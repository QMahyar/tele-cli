use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::require_chat_target;
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

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, default_value_t = 20, help = "max dialogs to list (1-10000)")]
    limit: u32,
    #[arg(long, help = "folder ID: 0=main, 1=archive")]
    folder: Option<i32>,
}

#[derive(Args)]
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

#[derive(Args)]
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

#[derive(Args)]
pub struct PinArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "unpin instead of pin")]
    unpin: bool,
}

#[derive(Args)]
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
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let folder_arg = args.folder;
    let folder = folder_arg.unwrap_or(0);
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();

        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_data(limit, folder_arg));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let mut iter = guard.client.iter_dialogs();
            let mut rows = Vec::new();
            let mut count = 0u32;
            let mut seen = 0usize;
            while count < limit {
                match iter.next().await.map_err(tele_invocation)? {
                    Some(dialog) => {
                        seen += 1;
                        guard.rate_limiter.acquire_for_items(seen).await;
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
                        let last_message_date =
                            dialog.last_message.as_ref().map(|m| m.date().to_rfc3339());
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
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
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
            Ok(serde_json::json!({"dialogs": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn drafts(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if args.folder.is_some() {
        return Err(TeleError::Usage(
            "--folder is not supported for dialog drafts".to_string(),
        ));
    }
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit as usize;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(drafts_dry_run_data(limit));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let updates: tl::enums::Updates = guard
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
                        if rows.len() >= limit {
                            break;
                        }
                    }
                }
            }
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
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
            Ok(serde_json::json!({"drafts": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
    let unarchive = args.unarchive;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "archive": !unarchive,
                    "would": format!("{} chat {target}", if unarchive { "unarchive" } else { "archive" }),
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let folder_id = if unarchive { 0 } else { 1 };
            let _: tl::enums::Updates = guard
                .client
                .invoke(&tl::functions::folders::EditPeerFolders {
                    folder_peers: vec![tl::enums::InputFolderPeer::Peer(
                        tl::types::InputFolderPeer { peer, folder_id },
                    )],
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "archive": !unarchive,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
    let target = args.chat.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = target.clone();
        let action = action.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(draft_dry_run_data(&target, cleared));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let _: bool = guard
                .client
                .invoke(&draft_request(peer, &action))
                .await
                .map_err(tele_invocation)?;
            Ok(draft_result(&target, &action))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn pin(args: PinArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let pinned = !args.unpin;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let target = args.chat.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(pin_dry_run_data(&target, pinned));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let _: bool = guard
                .client
                .invoke(&pin_request(peer, pinned))
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "pinned": pinned,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
    let revoke = args.revoke;
    let target = args.chat.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(delete_dry_run_data(&target, revoke));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            match delete_route(&peer) {
                DeleteRoute::HistoryClear => {
                    let _: tl::enums::messages::AffectedHistory = guard
                        .client
                        .invoke(&tl::functions::messages::DeleteHistory {
                            just_clear: false,
                            revoke,
                            peer,
                            max_id: 0,
                            min_date: None,
                            max_date: None,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    Ok(delete_result(&target, false, true))
                }
                DeleteRoute::Leave => {
                    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
                    guard
                        .client
                        .delete_dialog(chat_ref)
                        .await
                        .map_err(tele_invocation)?;
                    Ok(delete_result(&target, true, false))
                }
            }
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) fn dialog_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TeleError;

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

