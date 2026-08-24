use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::{require_chat_target, validate_limit};
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum DialogCmd {
    List(ListArgs),
    Drafts(ListArgs),
    #[command(about = "save or clear a message draft for a chat (--text / --clear)")]
    Draft(DraftArgs),
    Archive(ArchiveArgs),
    #[command(about = "pin or unpin a dialog in the chat list (--unpin)")]
    Pin(PinArgs),
    #[command(
        about = "remove a dialog from the list: leaves channels and groups; private chats keep history on both sides unless --revoke also deletes it"
    )]
    Delete(DeleteArgs),
}

#[derive(Args, Clone)]
pub struct ListArgs {
    #[arg(long, default_value_t = 20, help = "max dialogs to list (1-10000)")]
    limit: u32,
    #[arg(long, help = "folder ID: 0=main, 1=archive")]
    folder: Option<i32>,
}

#[derive(Args, Clone)]
pub struct ArchiveArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "unarchive (restore) instead of archive")]
    unarchive: bool,
}

#[derive(Args)]
pub struct ChatArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
}

#[derive(Args, Clone)]
pub struct DraftArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "draft text to save; mutually exclusive with --clear")]
    text: Option<String>,
    #[arg(
        long,
        help = "remove the saved draft instead of saving text; mutually exclusive with --text"
    )]
    clear: bool,
}

#[derive(Args, Clone)]
pub struct PinArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "unpin instead of pin")]
    unpin: bool,
}

#[derive(Args, Clone)]
pub struct DeleteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "private chats only: also delete the history for both sides (channels/groups are left regardless; their history is untouched)"
    )]
    revoke: bool,
}

pub async fn run(cmd: DialogCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        DialogCmd::List(a) => list(a, flags).await,
        DialogCmd::Drafts(a) => drafts(a, flags).await,
        DialogCmd::Draft(a) => draft(a, flags).await,
        DialogCmd::Archive(a) => archive(a, flags).await,
        DialogCmd::Pin(a) => pin(a, flags).await,
        DialogCmd::Delete(a) => delete(a, flags).await,
    }
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();

        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_data(args.limit, args.folder));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = dialog_list_core(&guard.shares(), ListParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let table_rows: Vec<Vec<String>> = result["dialogs"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|r| {
                        vec![
                            r["chat"]["name"].as_str().unwrap_or_default().to_string(),
                            r["unread"].to_string(),
                            r["draft"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(60)
                                .collect(),
                        ]
                    })
                    .collect();
                output::print_account_table(
                    &name,
                    multi,
                    &["chat", "unread", "draft"],
                    &table_rows,
                )?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn dialog_list_core(
    shares: &crate::client::ServeShares,
    params: ListParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let folder = params.folder.unwrap_or(0);
    let mut iter = shares.client.iter_dialogs();
    let mut rows = Vec::new();
    let mut count = 0u32;
    let mut seen = 0usize;
    while count < params.limit {
        match iter.next().await.map_err(tele_invocation)? {
            Some(dialog) => {
                seen += 1;
                shares.rate_limiter.acquire_for_items(seen).await;
                if !is_dialog_row(&dialog.raw) {
                    continue;
                }
                let d = match &dialog.raw {
                    tl::enums::Dialog::Dialog(d) => d,
                    tl::enums::Dialog::Folder(_) => continue,
                };
                let draft = match &d.draft {
                    Some(tl::enums::DraftMessage::Message(dm)) => dm.message.clone(),
                    _ => String::new(),
                };
                if !matches_folder(&dialog.raw, folder) {
                    continue;
                }
                let last = dialog
                    .last_message
                    .as_ref()
                    .map(|m| m.text().to_string())
                    .unwrap_or_default();
                let last_message_date = dialog.last_message.as_ref().map(|m| m.date().to_rfc3339());
                rows.push(dialog_row(
                    d,
                    crate::serialize::peer_key(&dialog.peer),
                    draft,
                    last,
                    last_message_date,
                ));
                count += 1;
            }
            None => break,
        }
    }
    Ok(serde_json::json!({"dialogs": rows}))
}

async fn drafts(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if args.folder.is_some() {
        return Err(TeleError::Usage(
            "--folder is not supported for dialog drafts".to_string(),
        ));
    }
    validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(drafts_dry_run_data(args.limit as usize));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = dialog_drafts_core(&guard.shares(), DraftsParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let table_rows: Vec<Vec<String>> = result["drafts"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["draft"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(60)
                                .collect(),
                        ]
                    })
                    .collect();
                output::print_account_table(&name, multi, &["id", "draft"], &table_rows)?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn dialog_drafts_core(
    shares: &crate::client::ServeShares,
    params: DraftsParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let updates: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::messages::GetAllDrafts {})
        .await
        .map_err(tele_invocation)?;
    let mut rows = Vec::new();
    for update in collect_updates(&updates) {
        if let tl::enums::Update::DraftMessage(u) = update {
            if let tl::enums::DraftMessage::Message(d) = &u.draft {
                let id = match &u.peer {
                    tl::enums::Peer::User(p) => p.user_id,
                    tl::enums::Peer::Chat(p) => -p.chat_id,
                    tl::enums::Peer::Channel(p) => -p.channel_id,
                };
                rows.push(serde_json::json!({
                    "id": id,
                    "draft": d.message.clone(),
                }));
                if rows.len() >= params.limit as usize {
                    break;
                }
            }
        }
    }
    Ok(serde_json::json!({"drafts": rows}))
}

fn list_dry_run_data(limit: u32, folder: Option<i32>) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "limit": limit,
        "folder": folder,
        "would": "list dialogs"
    })
}

fn is_dialog_row(raw: &tl::enums::Dialog) -> bool {
    matches!(raw, tl::enums::Dialog::Dialog(_))
}

fn dialog_row(
    d: &tl::types::Dialog,
    chat: serde_json::Value,
    draft: String,
    last_message: String,
    last_message_date: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "chat": chat,
        "unread": d.unread_count,
        "pinned": d.pinned,
        "unread_mark": d.unread_mark,
        "unread_mentions": d.unread_mentions_count,
        "unread_reactions": d.unread_reactions_count,
        "draft": draft,
        "last_message": last_message,
        "last_message_date": last_message_date,
    })
}

fn matches_folder(raw: &tl::enums::Dialog, folder: i32) -> bool {
    match raw {
        tl::enums::Dialog::Dialog(d) => d.folder_id.unwrap_or(0) == folder,
        tl::enums::Dialog::Folder(_) => false,
    }
}

fn drafts_dry_run_data(limit: usize) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "limit": limit,
        "would": "list drafts"
    })
}

fn collect_updates(updates: &tl::enums::Updates) -> Vec<&tl::enums::Update> {
    match updates {
        tl::enums::Updates::Updates(u) => u.updates.iter().collect(),
        tl::enums::Updates::Combined(u) => u.updates.iter().collect(),
        tl::enums::Updates::UpdateShort(u) => vec![&u.update],
        tl::enums::Updates::UpdateShortMessage(_)
        | tl::enums::Updates::UpdateShortChatMessage(_)
        | tl::enums::Updates::UpdateShortSentMessage(_) => Vec::new(),
        tl::enums::Updates::TooLong => Vec::new(),
    }
}

async fn archive(args: ArchiveArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(archive_dry_run_data(&args.chat, args.unarchive));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            dialog_archive_core(&guard.shares(), ArchiveParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn archive_dry_run_data(target: &str, unarchive: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": target,
        "archive": !unarchive,
        "would": format!("{} chat {target}", if unarchive { "unarchive" } else { "archive" }),
    })
}

pub(crate) async fn dialog_archive_core(
    shares: &crate::client::ServeShares,
    params: ArchiveParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let folder_id = if params.unarchive { 0 } else { 1 };
    let _: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::folders::EditPeerFolders {
            folder_peers: vec![tl::enums::InputFolderPeer::Peer(
                tl::types::InputFolderPeer { peer, folder_id },
            )],
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "chat": params.chat,
        "archive": !params.unarchive,
    }))
}

#[derive(Clone, Debug)]
enum DraftAction {
    Set(String),
    Clear,
}

fn draft_action(text: &Option<String>, clear: bool) -> TeleResult<DraftAction> {
    if clear {
        if text.is_some() {
            return Err(TeleError::Usage(
                "--text and --clear are mutually exclusive".to_string(),
            ));
        }
        return Ok(DraftAction::Clear);
    }
    match text {
        Some(t) => Ok(DraftAction::Set(t.clone())),
        None => Err(TeleError::Usage(
            "nothing to do: pass --text <t> to save a draft or --clear to remove it".to_string(),
        )),
    }
}

fn draft_request(
    peer: tl::enums::InputPeer,
    action: &DraftAction,
) -> tl::functions::messages::SaveDraft {
    let message = match action {
        DraftAction::Set(text) => text.clone(),
        DraftAction::Clear => String::new(),
    };
    tl::functions::messages::SaveDraft {
        no_webpage: false,
        invert_media: false,
        reply_to: None,
        peer,
        message,
        entities: None,
        media: None,
        effect: None,
        suggested_post: None,
        rich_message: None,
    }
}

fn pin_request(
    peer: tl::enums::InputPeer,
    pinned: bool,
) -> tl::functions::messages::ToggleDialogPin {
    tl::functions::messages::ToggleDialogPin {
        pinned,
        peer: tl::enums::InputDialogPeer::Peer(tl::types::InputDialogPeer { peer }),
    }
}

enum DeleteRoute {
    Leave,
    HistoryClear,
}

fn delete_route(peer: &tl::enums::InputPeer) -> DeleteRoute {
    match peer {
        tl::enums::InputPeer::User(_) => DeleteRoute::HistoryClear,
        _ => DeleteRoute::Leave,
    }
}

fn delete_result(target: &str, left: bool, cleared: bool) -> serde_json::Value {
    serde_json::json!({
        "chat": target,
        "deleted": true,
        "left": left,
        "cleared": cleared,
    })
}

async fn draft(args: DraftArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let action = draft_action(&args.text, args.clear)?;
    let cleared = matches!(action, DraftAction::Clear);
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(draft_dry_run_data(&args.chat, cleared));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            dialog_draft_core(&guard.shares(), DraftParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn dialog_draft_core(
    shares: &crate::client::ServeShares,
    params: DraftParams,
) -> TeleResult<serde_json::Value> {
    let action = draft_action(&params.text, params.clear)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let _: bool = shares
        .client
        .invoke(&draft_request(peer, &action))
        .await
        .map_err(tele_invocation)?;
    Ok(draft_result(&params.chat, &action))
}

async fn pin(args: PinArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let pinned = !args.unpin;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(pin_dry_run_data(&args.chat, pinned));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            dialog_pin_core(&guard.shares(), PinParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn dialog_pin_core(
    shares: &crate::client::ServeShares,
    params: PinParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let _: bool = shares
        .client
        .invoke(&pin_request(peer, !params.unpin))
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "chat": params.chat,
        "pinned": !params.unpin,
    }))
}

fn draft_result(target: &str, action: &DraftAction) -> serde_json::Value {
    match action {
        DraftAction::Set(text) => serde_json::json!({
            "chat": target,
            "cleared": false,
            "draft": text,
        }),
        DraftAction::Clear => serde_json::json!({
            "chat": target,
            "cleared": true,
            "draft": "",
        }),
    }
}

fn draft_dry_run_data(target: &str, cleared: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": target,
        "cleared": cleared,
        "would": format!(
            "{} draft for chat {target}",
            if cleared { "clear" } else { "save" }
        ),
    })
}

fn pin_dry_run_data(target: &str, pinned: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": target,
        "pinned": pinned,
        "would": format!(
            "{} dialog with chat {target}",
            if pinned { "pin" } else { "unpin" }
        ),
    })
}

fn delete_dry_run_data(target: &str, revoke: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": target,
        "revoke": revoke,
        "would": format!(
            "delete dialog with chat {target} (leaves channels/groups; clears private-chat history{})",
            if revoke { " for both sides" } else { "" }
        ),
    })
}

async fn delete(args: DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(delete_dry_run_data(&args.chat, args.revoke));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            dialog_delete_core(&guard.shares(), DeleteParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn dialog_delete_core(
    shares: &crate::client::ServeShares,
    params: DeleteParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    match delete_route(&peer) {
        DeleteRoute::HistoryClear => {
            let _: tl::enums::messages::AffectedHistory = shares
                .client
                .invoke(&tl::functions::messages::DeleteHistory {
                    just_clear: false,
                    revoke: params.revoke,
                    peer,
                    max_id: 0,
                    min_date: None,
                    max_date: None,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(delete_result(&params.chat, false, true))
        }
        DeleteRoute::Leave => {
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            shares
                .client
                .delete_dialog(chat_ref)
                .await
                .map_err(tele_invocation)?;
            Ok(delete_result(&params.chat, true, false))
        }
    }
}

fn default_dialog_limit() -> u32 {
    20
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    #[serde(default = "default_dialog_limit")]
    pub(crate) limit: u32,
    pub(crate) folder: Option<i32>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ListArgs> for ListParams {
    fn from(a: &ListArgs) -> Self {
        Self {
            limit: a.limit,
            folder: a.folder,
            dry_run: false,
        }
    }
}

impl From<&ListParams> for ListArgs {
    fn from(p: &ListParams) -> Self {
        Self {
            limit: p.limit,
            folder: p.folder,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftsParams {
    #[serde(default = "default_dialog_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ListArgs> for DraftsParams {
    fn from(a: &ListArgs) -> Self {
        Self {
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&DraftsParams> for ListArgs {
    fn from(p: &DraftsParams) -> Self {
        Self {
            limit: p.limit,
            folder: None,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) clear: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&DraftArgs> for DraftParams {
    fn from(a: &DraftArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            text: a.text.clone(),
            clear: a.clear,
            dry_run: false,
        }
    }
}

impl From<&DraftParams> for DraftArgs {
    fn from(p: &DraftParams) -> Self {
        Self {
            chat: p.chat.clone(),
            text: p.text.clone(),
            clear: p.clear,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) unarchive: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ArchiveArgs> for ArchiveParams {
    fn from(a: &ArchiveArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            unarchive: a.unarchive,
            dry_run: false,
        }
    }
}

impl From<&ArchiveParams> for ArchiveArgs {
    fn from(p: &ArchiveParams) -> Self {
        Self {
            chat: p.chat.clone(),
            unarchive: p.unarchive,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PinParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) unpin: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&PinArgs> for PinParams {
    fn from(a: &PinArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            unpin: a.unpin,
            dry_run: false,
        }
    }
}

impl From<&PinParams> for PinArgs {
    fn from(p: &PinParams) -> Self {
        Self {
            chat: p.chat.clone(),
            unpin: p.unpin,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) revoke: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&DeleteArgs> for DeleteParams {
    fn from(a: &DeleteArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            revoke: a.revoke,
            dry_run: false,
        }
    }
}

impl From<&DeleteParams> for DeleteArgs {
    fn from(p: &DeleteParams) -> Self {
        Self {
            chat: p.chat.clone(),
            revoke: p.revoke,
        }
    }
}

pub(crate) fn validate_list(args: &ListArgs) -> TeleResult<()> {
    validate_limit(args.limit, 10_000, "limit")?;
    Ok(())
}

pub(crate) fn validate_drafts(args: &ListArgs) -> TeleResult<()> {
    if args.folder.is_some() {
        return Err(TeleError::Usage(
            "--folder is not supported for dialog drafts".to_string(),
        ));
    }
    validate_limit(args.limit, 10_000, "limit")?;
    Ok(())
}

pub(crate) fn validate_draft(args: &DraftArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    draft_action(&args.text, args.clear)?;
    Ok(())
}

pub(crate) fn validate_archive(args: &ArchiveArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")
}

pub(crate) fn validate_pin(args: &PinArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")
}

pub(crate) fn validate_delete(args: &DeleteArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")
}

pub(crate) fn list_serve_dry_run(args: &ListArgs) -> TeleResult<serde_json::Value> {
    Ok(list_dry_run_data(args.limit, args.folder))
}

pub(crate) fn drafts_serve_dry_run(args: &ListArgs) -> TeleResult<serde_json::Value> {
    Ok(drafts_dry_run_data(args.limit as usize))
}

pub(crate) fn draft_serve_dry_run(args: &DraftArgs) -> TeleResult<serde_json::Value> {
    Ok(draft_dry_run_data(&args.chat, args.clear))
}

pub(crate) fn archive_serve_dry_run(args: &ArchiveArgs) -> TeleResult<serde_json::Value> {
    Ok(archive_dry_run_data(&args.chat, args.unarchive))
}

pub(crate) fn pin_serve_dry_run(args: &PinArgs) -> TeleResult<serde_json::Value> {
    Ok(pin_dry_run_data(&args.chat, !args.unpin))
}

pub(crate) fn delete_serve_dry_run(args: &DeleteArgs) -> TeleResult<serde_json::Value> {
    Ok(delete_dry_run_data(&args.chat, args.revoke))
}

pub(crate) fn dialog_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "dialog archive",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            ArchiveParams,
            ArchiveArgs,
            validate_archive,
            archive_serve_dry_run,
            run_archive
        ),
        crate::serve_route!(
            "dialog delete",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            DeleteParams,
            DeleteArgs,
            validate_delete,
            delete_serve_dry_run,
            run_delete
        ),
        crate::serve_route!(
            "dialog draft",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            DraftParams,
            DraftArgs,
            validate_draft,
            draft_serve_dry_run,
            run_draft
        ),
        crate::serve_route!(
            "dialog drafts",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            DraftsParams,
            ListArgs,
            validate_drafts,
            drafts_serve_dry_run,
            run_drafts
        ),
        crate::serve_route!(
            "dialog list",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            ListParams,
            ListArgs,
            validate_list,
            list_serve_dry_run,
            run_list
        ),
        crate::serve_route!(
            "dialog pin",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            PinParams,
            PinArgs,
            validate_pin,
            pin_serve_dry_run,
            run_pin
        ),
    ]
}

crate::serve_runner!(run_archive, dialog_archive_core, ArchiveParams);
crate::serve_runner!(run_delete, dialog_delete_core, DeleteParams);
crate::serve_runner!(run_draft, dialog_draft_core, DraftParams);
crate::serve_runner!(run_drafts, dialog_drafts_core, DraftsParams);
crate::serve_runner!(run_list, dialog_list_core, ListParams);
crate::serve_runner!(run_pin, dialog_pin_core, PinParams);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::serve::{Lane, Plan};
    use crate::error::TeleError;

    fn plan_dialog_op(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
        let route = dialog_serve_routes()
            .into_iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    fn serve_error_message(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "ServeError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn usage_error_message(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "UsageError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn expect_execute(plan: Plan, raw: &serde_json::Value) {
        match plan {
            Plan::Execute(passed) => assert_eq!(&passed, raw),
            other => panic!("expected execute plan, got {other:?}"),
        }
    }

    #[test]
    fn serve_dialog_list_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op("dialog list", serde_json::json!({"limit": "many"})).unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog list", serde_json::json!({"limit": 10_001})).unwrap_err(),
        );
        assert!(msg.contains("--limit"), "{msg}");

        let plan = plan_dialog_op(
            "dialog list",
            serde_json::json!({"limit": 7, "folder": 1, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, list_dry_run_data(7, Some(1))),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"folder": 0});
        let plan = plan_dialog_op("dialog list", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_dialog_drafts_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op("dialog drafts", serde_json::json!({"limit": true})).unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = serve_error_message(
            plan_dialog_op("dialog drafts", serde_json::json!({"folder": 1})).unwrap_err(),
        );
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("folder"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog drafts", serde_json::json!({"limit": 10_001})).unwrap_err(),
        );
        assert!(msg.contains("--limit"), "{msg}");

        let plan = plan_dialog_op(
            "dialog drafts",
            serde_json::json!({"limit": 100, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, drafts_dry_run_data(100)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({});
        let plan = plan_dialog_op("dialog drafts", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_dialog_draft_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op(
                "dialog draft",
                serde_json::json!({"chat": "@x", "clear": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog draft", serde_json::json!({"chat": "@x"})).unwrap_err(),
        );
        assert!(msg.contains("nothing to do"), "{msg}");
        let msg = usage_error_message(
            plan_dialog_op(
                "dialog draft",
                serde_json::json!({"chat": "@x", "text": "a", "clear": true}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("mutually exclusive"), "{msg}");

        let plan = plan_dialog_op(
            "dialog draft",
            serde_json::json!({"chat": "@x", "text": "hi", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, draft_dry_run_data("@x", false)),
            other => panic!("expected dry run plan, got {other:?}"),
        }
        let plan = plan_dialog_op(
            "dialog draft",
            serde_json::json!({"chat": "@x", "clear": true, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, draft_dry_run_data("@x", true)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "@x", "text": "hi"});
        let plan = plan_dialog_op("dialog draft", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_dialog_pin_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op(
                "dialog pin",
                serde_json::json!({"chat": "@x", "unpin": "y"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog pin", serde_json::json!({"chat": " "})).unwrap_err(),
        );
        assert!(msg.contains("--chat"), "{msg}");

        let plan = plan_dialog_op(
            "dialog pin",
            serde_json::json!({"chat": "@x", "unpin": true, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, pin_dry_run_data("@x", false)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "@x", "unpin": true});
        let plan = plan_dialog_op("dialog pin", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_dialog_archive_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op(
                "dialog archive",
                serde_json::json!({"chat": "@x", "unarchive": 1}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog archive", serde_json::json!({"chat": ""})).unwrap_err(),
        );
        assert!(msg.contains("--chat"), "{msg}");

        let plan = plan_dialog_op(
            "dialog archive",
            serde_json::json!({"chat": "@x", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, archive_dry_run_data("@x", false)),
            other => panic!("expected dry run plan, got {other:?}"),
        }
        let plan = plan_dialog_op(
            "dialog archive",
            serde_json::json!({"chat": "@x", "unarchive": true, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, archive_dry_run_data("@x", true)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "@x", "unarchive": true});
        let plan = plan_dialog_op("dialog archive", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_dialog_delete_plan_matrix() {
        let msg = serve_error_message(
            plan_dialog_op(
                "dialog delete",
                serde_json::json!({"chat": "@x", "revoke": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error_message(
            plan_dialog_op("dialog delete", serde_json::json!({"chat": ""})).unwrap_err(),
        );
        assert!(msg.contains("--chat"), "{msg}");

        let plan = plan_dialog_op(
            "dialog delete",
            serde_json::json!({"chat": "@x", "revoke": true, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, delete_dry_run_data("@x", true)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "@x", "revoke": true});
        let plan = plan_dialog_op("dialog delete", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn dialog_serve_lane_and_timeout_table_is_locked() {
        let expected: &[(&str, Lane, Option<u64>)] = &[
            ("dialog archive", Lane::Mutate, Some(30)),
            ("dialog delete", Lane::Mutate, Some(30)),
            ("dialog draft", Lane::Mutate, Some(30)),
            ("dialog drafts", Lane::Read, Some(120)),
            ("dialog list", Lane::Read, Some(120)),
            ("dialog pin", Lane::Mutate, Some(30)),
        ];
        let routes = dialog_serve_routes();
        assert_eq!(routes.len(), expected.len());
        for (op, lane, secs) in expected {
            let route = routes
                .iter()
                .find(|r| r.op == *op)
                .unwrap_or_else(|| panic!("route missing for {op}"));
            assert_eq!(route.lane, *lane, "lane for {op}");
            assert_eq!(
                route.timeout,
                secs.map(std::time::Duration::from_secs),
                "timeout for {op}"
            );
        }
    }

    #[tokio::test]
    async fn drafts_rejects_over_limit() {
        let flags = GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: false,
            quiet: false,
            config_path: None,
            command: "dialog drafts".to_string(),
        };
        let err = drafts(
            ListArgs {
                limit: 10_001,
                folder: None,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("too large"));
    }

    #[tokio::test]
    async fn drafts_accepts_max_limit_with_dry_run() {
        let dir = temp_app("drafts-max");
        let flags = dialog_flags("dialog drafts", &dir.join("config.toml"));
        let code = drafts(
            ListArgs {
                limit: 10_000,
                folder: None,
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_dry_run_exits_ok_before_connect() {
        let dir = temp_app("list-dry");
        let flags = dialog_flags("dialog list", &dir.join("config.toml"));
        let code = list(
            ListArgs {
                limit: 20,
                folder: Some(1),
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn drafts_dry_run_still_validates_limit() {
        let flags = GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: true,
            quiet: false,
            config_path: None,
            command: "dialog drafts".to_string(),
        };
        let err = drafts(
            ListArgs {
                limit: 10_001,
                folder: None,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("too large"));
    }

    #[tokio::test]
    async fn list_dry_run_still_validates_limit() {
        let flags = GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: true,
            quiet: false,
            config_path: None,
            command: "dialog list".to_string(),
        };
        let err = list(
            ListArgs {
                limit: 10_001,
                folder: None,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("too large"));
    }

    #[test]
    fn list_dry_run_data_marks_dry_run_and_echoes_args() {
        let v = list_dry_run_data(7, Some(1));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["limit"], serde_json::json!(7));
        assert_eq!(v["folder"], serde_json::json!(1));
    }

    #[test]
    fn drafts_dry_run_data_marks_dry_run_and_echoes_limit() {
        let v = drafts_dry_run_data(100);
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["limit"], serde_json::json!(100));
    }

    fn notify_settings() -> tl::enums::PeerNotifySettings {
        tl::enums::PeerNotifySettings::Settings(tl::types::PeerNotifySettings {
            show_previews: None,
            silent: None,
            mute_until: None,
            ios_sound: None,
            android_sound: None,
            other_sound: None,
            stories_muted: None,
            stories_hide_sender: None,
            stories_ios_sound: None,
            stories_android_sound: None,
            stories_other_sound: None,
        })
    }

    fn raw_dialog(folder_id: Option<i32>) -> tl::enums::Dialog {
        tl::enums::Dialog::Dialog(tl::types::Dialog {
            pinned: false,
            unread_mark: false,
            view_forum_as_messages: false,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 1 }),
            top_message: 0,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            unread_count: 0,
            unread_mentions_count: 0,
            unread_reactions_count: 0,
            unread_poll_votes_count: 0,
            notify_settings: notify_settings(),
            pts: None,
            draft: None,
            folder_id,
            ttl_period: None,
        })
    }

    fn rich_dialog() -> tl::types::Dialog {
        tl::types::Dialog {
            pinned: true,
            unread_mark: true,
            view_forum_as_messages: false,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 1 }),
            top_message: 10,
            read_inbox_max_id: 5,
            read_outbox_max_id: 8,
            unread_count: 3,
            unread_mentions_count: 2,
            unread_reactions_count: 4,
            unread_poll_votes_count: 0,
            notify_settings: notify_settings(),
            pts: None,
            draft: None,
            folder_id: Some(0),
            ttl_period: None,
        }
    }

    #[test]
    fn dialog_row_locks_every_key_and_value() {
        let row = dialog_row(
            &rich_dialog(),
            serde_json::json!({"id": 1, "kind": "user"}),
            "draft text".to_string(),
            "last message".to_string(),
            Some("2026-08-21T00:00:00+00:00".to_string()),
        );
        let obj = row.as_object().expect("row must be an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "chat",
                "draft",
                "last_message",
                "last_message_date",
                "pinned",
                "unread",
                "unread_mark",
                "unread_mentions",
                "unread_reactions",
            ]
        );
        assert_eq!(row["unread"], 3);
        assert_eq!(row["pinned"], serde_json::json!(true));
        assert_eq!(row["unread_mark"], serde_json::json!(true));
        assert_eq!(row["unread_mentions"], 2);
        assert_eq!(row["unread_reactions"], 4);
        assert_eq!(row["draft"], "draft text");
        assert_eq!(row["last_message"], "last message");
        assert_eq!(row["last_message_date"], "2026-08-21T00:00:00+00:00");
    }

    #[test]
    fn dialog_row_last_message_date_is_null_without_last_message() {
        let mut d = rich_dialog();
        d.pinned = false;
        d.unread_mark = false;
        d.unread_count = 0;
        d.unread_mentions_count = 0;
        d.unread_reactions_count = 0;
        let row = dialog_row(
            &d,
            serde_json::json!({"id": 1}),
            String::new(),
            String::new(),
            None,
        );
        assert_eq!(row["pinned"], serde_json::json!(false));
        assert_eq!(row["unread_mark"], serde_json::json!(false));
        assert_eq!(row["unread"], 0);
        assert_eq!(row["unread_mentions"], 0);
        assert_eq!(row["unread_reactions"], 0);
        assert!(row["last_message_date"].is_null());
    }

    fn phantom_dialog() -> tl::enums::Dialog {
        tl::enums::Dialog::Folder(tl::types::DialogFolder {
            pinned: false,
            folder: tl::enums::Folder::Folder(tl::types::Folder {
                autofill_new_broadcasts: false,
                autofill_public_groups: false,
                autofill_new_correspondents: false,
                id: 1,
                title: "archive".to_string(),
                photo: None,
            }),
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 99 }),
            top_message: 0,
            unread_muted_peers_count: 0,
            unread_unmuted_peers_count: 0,
            unread_muted_messages_count: 0,
            unread_unmuted_messages_count: 0,
        })
    }

    #[test]
    fn phantom_folder_rows_are_not_dialogs() {
        assert!(is_dialog_row(&raw_dialog(Some(0))));
        assert!(is_dialog_row(&raw_dialog(None)));
        assert!(!is_dialog_row(&phantom_dialog()));
    }

    #[test]
    fn folder_zero_matches_main_rows_only() {
        assert!(matches_folder(&raw_dialog(Some(0)), 0));
        assert!(matches_folder(&raw_dialog(None), 0));
        assert!(!matches_folder(&raw_dialog(Some(1)), 0));
        assert!(!matches_folder(&phantom_dialog(), 0));
    }

    #[test]
    fn folder_one_matches_only_archive_rows() {
        assert!(matches_folder(&raw_dialog(Some(1)), 1));
        assert!(!matches_folder(&raw_dialog(Some(0)), 1));
        assert!(!matches_folder(&raw_dialog(None), 1));
        assert!(!matches_folder(&phantom_dialog(), 1));
    }

    #[tokio::test]
    async fn drafts_rejects_folder_flag() {
        let flags = GlobalFlags {
            account: vec!["work".to_string()],
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: true,
            quiet: false,
            config_path: None,
            command: "dialog drafts".to_string(),
        };
        let err = drafts(
            ListArgs {
                limit: 20,
                folder: Some(1),
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--folder"));
    }

    #[test]
    fn draft_action_requires_text_or_clear() {
        let err = draft_action(&None, false).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--text"));
        assert!(err.message().contains("--clear"));
    }

    #[test]
    fn draft_action_rejects_text_and_clear_together() {
        let text = Some("hi".to_string());
        let err = draft_action(&text, true).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("mutually exclusive"));
    }

    #[test]
    fn draft_action_set_carries_text_and_clear_when_only_clear() {
        assert!(matches!(
            draft_action(&Some("hello".to_string()), false).unwrap(),
            DraftAction::Set(t) if t == "hello"
        ));
        assert!(matches!(
            draft_action(&None, true).unwrap(),
            DraftAction::Clear
        ));
    }

    #[tokio::test]
    async fn draft_validates_target_flags_before_connect_even_in_dry_run() {
        let dir = temp_app("draft-validate");
        let flags = dialog_flags("dialog draft", &dir.join("config.toml"));
        let err = draft(
            DraftArgs {
                chat: "me".to_string(),
                text: None,
                clear: false,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        let err = draft(
            DraftArgs {
                chat: "me".to_string(),
                text: Some("a".to_string()),
                clear: true,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn peer_user(user_id: i64) -> tl::enums::InputPeer {
        tl::enums::InputPeer::User(tl::types::InputPeerUser {
            user_id,
            access_hash: 0,
        })
    }

    fn peer_chat(chat_id: i64) -> tl::enums::InputPeer {
        tl::enums::InputPeer::Chat(tl::types::InputPeerChat { chat_id })
    }

    fn peer_channel(channel_id: i64) -> tl::enums::InputPeer {
        tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
            channel_id,
            access_hash: 0,
        })
    }

    #[test]
    fn draft_request_set_shapes_save_draft_fields() {
        let req = draft_request(peer_user(7), &DraftAction::Set("hello".to_string()));
        assert_eq!(req.peer, peer_user(7));
        assert_eq!(req.message, "hello");
        assert!(!req.no_webpage);
        assert!(!req.invert_media);
        assert!(req.reply_to.is_none());
        assert!(req.entities.is_none());
        assert!(req.media.is_none());
    }

    #[test]
    fn draft_request_clear_saves_empty_message() {
        let req = draft_request(peer_user(7), &DraftAction::Clear);
        assert_eq!(req.peer, peer_user(7));
        assert!(req.message.is_empty());
    }

    #[test]
    fn pin_request_wraps_peer_in_input_dialog_peer() {
        let req = pin_request(peer_channel(11), true);
        assert!(req.pinned);
        match req.peer {
            tl::enums::InputDialogPeer::Peer(p) => assert_eq!(p.peer, peer_channel(11)),
            _ => panic!("peer must wrap InputDialogPeer::Peer"),
        }
        let req = pin_request(peer_user(7), false);
        assert!(!req.pinned);
    }

    #[test]
    fn delete_route_clears_history_for_users_only() {
        assert!(matches!(
            delete_route(&peer_user(7)),
            DeleteRoute::HistoryClear
        ));
        assert!(matches!(delete_route(&peer_chat(9)), DeleteRoute::Leave));
        assert!(matches!(
            delete_route(&peer_channel(11)),
            DeleteRoute::Leave
        ));
    }

    #[test]
    fn delete_result_reports_left_and_cleared_honestly() {
        let left = delete_result("@x", true, false);
        assert_eq!(left["deleted"], serde_json::json!(true));
        assert_eq!(left["left"], serde_json::json!(true));
        assert_eq!(left["cleared"], serde_json::json!(false));

        let cleared = delete_result("@x", false, true);
        assert_eq!(cleared["deleted"], serde_json::json!(true));
        assert_eq!(cleared["left"], serde_json::json!(false));
        assert_eq!(cleared["cleared"], serde_json::json!(true));
    }

    #[test]
    fn draft_result_echoes_text_on_set_and_empty_on_clear() {
        let set = draft_result("me", &DraftAction::Set("note".to_string()));
        assert_eq!(set["chat"], serde_json::json!("me"));
        assert_eq!(set["cleared"], serde_json::json!(false));
        assert_eq!(set["draft"], serde_json::json!("note"));

        let clear = draft_result("me", &DraftAction::Clear);
        assert_eq!(clear["cleared"], serde_json::json!(true));
        assert_eq!(clear["draft"], serde_json::json!(""));
    }

    #[test]
    fn draft_dry_run_data_echoes_cleared_flag() {
        let set = draft_dry_run_data("@x", false);
        assert_eq!(set["dry_run"], serde_json::json!(true));
        assert_eq!(set["chat"], serde_json::json!("@x"));
        assert_eq!(set["cleared"], serde_json::json!(false));
        assert_eq!(set["would"], serde_json::json!("save draft for chat @x"));

        let clear = draft_dry_run_data("@x", true);
        assert_eq!(clear["cleared"], serde_json::json!(true));
        assert_eq!(clear["would"], serde_json::json!("clear draft for chat @x"));
    }

    #[test]
    fn pin_dry_run_data_echoes_pinned_flag() {
        let pin = pin_dry_run_data("@x", true);
        assert_eq!(pin["dry_run"], serde_json::json!(true));
        assert_eq!(pin["pinned"], serde_json::json!(true));
        assert_eq!(pin["would"], serde_json::json!("pin dialog with chat @x"));

        let unpin = pin_dry_run_data("@x", false);
        assert_eq!(unpin["pinned"], serde_json::json!(false));
        assert_eq!(
            unpin["would"],
            serde_json::json!("unpin dialog with chat @x")
        );
    }

    #[test]
    fn delete_dry_run_data_echoes_revoke_flag_and_honest_would() {
        let plain = delete_dry_run_data("@x", false);
        assert_eq!(plain["dry_run"], serde_json::json!(true));
        assert_eq!(plain["revoke"], serde_json::json!(false));
        let would = plain["would"].as_str().unwrap();
        assert!(would.contains("leaves channels/groups"));
        assert!(would.contains("clears private-chat history"));
        assert!(!would.contains("both sides"));

        let revoked = delete_dry_run_data("@x", true);
        assert_eq!(revoked["revoke"], serde_json::json!(true));
        assert!(revoked["would"].as_str().unwrap().contains("both sides"));
    }

    #[tokio::test]
    async fn draft_dry_run_exits_ok_before_connect() {
        let dir = temp_app("draft-dry");
        let flags = dialog_flags("dialog draft", &dir.join("config.toml"));
        let code = draft(
            DraftArgs {
                chat: "@someone".to_string(),
                text: Some("hello".to_string()),
                clear: false,
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn draft_clear_dry_run_exits_ok_before_connect() {
        let dir = temp_app("draft-clear-dry");
        let flags = dialog_flags("dialog draft", &dir.join("config.toml"));
        let code = draft(
            DraftArgs {
                chat: "@someone".to_string(),
                text: None,
                clear: true,
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn pin_dry_run_exits_ok_before_connect() {
        let dir = temp_app("pin-dry");
        let flags = dialog_flags("dialog pin", &dir.join("config.toml"));
        let code = pin(
            PinArgs {
                chat: "@someone".to_string(),
                unpin: true,
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn delete_dry_run_exits_ok_before_connect() {
        let dir = temp_app("delete-dry");
        let flags = dialog_flags("dialog delete", &dir.join("config.toml"));
        let code = delete(
            DeleteArgs {
                chat: "@someone".to_string(),
                revoke: true,
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn dialog_flags(command: &str, config: &std::path::Path) -> GlobalFlags {
        GlobalFlags {
            account: vec!["work".to_string()],
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: true,
            quiet: false,
            config_path: Some(config.to_path_buf()),
            command: command.to_string(),
        }
    }

    fn temp_app(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-dialog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "[accounts.work]\ntags = []\n").unwrap();
        dir
    }
}
