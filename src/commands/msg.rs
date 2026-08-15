use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

use clap::{Args, Subcommand};
use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum MsgCmd {
    Send(SendArgs),
    Edit(EditArgs),
    Delete(DeleteArgs),
    Forward(ForwardArgs),
    Pin(PinArgs),
    Get(GetArgs),
    Read(ReadArgs),
    React(ReactArgs),
    Search(SearchArgs),
    Download(DownloadArgs),
}

#[derive(Args)]
pub struct SendArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message text (mutually exclusive with --file)")]
    text: Option<String>,
    #[arg(
        long,
        help = "send time: Unix timestamp or RFC3339 datetime (must be in the future)"
    )]
    schedule: Option<String>,
    #[arg(long, help = "file path to upload (mutually exclusive with --text)")]
    file: Option<String>,
    #[arg(long, help = "caption for uploaded file (requires --file)")]
    caption: Option<String>,
    #[arg(long, help = "message ID to reply to")]
    reply: Option<i32>,
    #[arg(long, default_value_t = true, help = "show link preview")]
    preview: bool,
    #[arg(long, default_value = "plain", help = "text format: plain or markdown")]
    format: String,
    #[arg(long, help = "send without notification sound")]
    silent: bool,
}

#[derive(Args)]
pub struct EditArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message ID to edit")]
    id: i32,
    #[arg(long, help = "new message text")]
    text: String,
}

#[derive(Args)]
pub struct DeleteArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to delete"
    )]
    ids: Vec<i32>,
    #[arg(long, help = "delete all messages in chat")]
    all: bool,
}

#[derive(Args)]
pub struct ForwardArgs {
    #[arg(long, help = "source chat to forward from")]
    from: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to forward"
    )]
    ids: Vec<i32>,
    #[arg(long, help = "destination chat to forward to")]
    to: String,
    #[arg(long, help = "forward without notification sound")]
    silent: bool,
}

#[derive(Args)]
pub struct PinArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message ID to pin or unpin")]
    id: i32,
    #[arg(long, help = "remove pin instead of adding")]
    unpin: bool,
    #[arg(long, help = "pin without notification sound")]
    silent: bool,
}

#[derive(Args)]
pub struct GetArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    limit: u32,
    #[arg(long, help = "fetch messages before this ID")]
    offset_id: Option<i32>,
    #[arg(long, help = "fetch only the most recent message")]
    last: bool,
}

#[derive(Args)]
pub struct ReadArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "mark as unread instead of read")]
    mark_unread: bool,
}

#[derive(Args)]
pub struct ReactArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message ID to react to")]
    id: i32,
    #[arg(long, help = "emoji reaction to add")]
    reaction: Option<String>,
    #[arg(long, help = "remove reaction instead of adding")]
    remove: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "search query text")]
    query: String,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct DownloadArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message ID to download media from")]
    id: i32,
    #[arg(long, help = "output directory for downloaded media")]
    dir: String,
}

pub async fn run(cmd: MsgCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        MsgCmd::Send(a) => send(a, flags).await,
        MsgCmd::Edit(a) => edit(a, flags).await,
        MsgCmd::Delete(a) => delete(a, flags).await,
        MsgCmd::Forward(a) => forward(a, flags).await,
        MsgCmd::Pin(a) => pin(a, flags).await,
        MsgCmd::Get(a) => get(a, flags).await,
        MsgCmd::Read(a) => read(a, flags).await,
        MsgCmd::React(a) => react(a, flags).await,
        MsgCmd::Search(a) => search(a, flags).await,
        MsgCmd::Download(a) => download(a, flags).await,
    }
}

fn validate_send(args: &SendArgs) -> TeleResult<()> {
    match args.format.as_str() {
        "plain" | "markdown" => Ok(()),
        other => Err(TeleError::Usage(format!(
            "unknown --format {other} (use plain or markdown)"
        ))),
    }?;
    match (&args.text, &args.file) {
        (None, None) => {
            return Err(TeleError::Usage(
                "msg send requires --text or --file".to_string(),
            ))
        }
        (Some(_), Some(_)) => {
            return Err(TeleError::Usage(
                "--text and --file are mutually exclusive".to_string(),
            ))
        }
        _ => {}
    }
    if args.caption.is_some() && args.file.is_none() {
        return Err(TeleError::Usage("--caption requires --file".to_string()));
    }
    if let Some(path) = &args.file {
        if !args.preview {
            return Err(TeleError::Usage(
                "--no-preview is not supported with --file".to_string(),
            ));
        }
        validate_upload_path(path)?;
    }
    if args.format == "markdown" {
        if let Some(text) = &args.text {
            validate_markdown(text)?;
        }
    }
    Ok(())
}

fn validate_markdown(text: &str) -> TeleResult<()> {
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("tg://user?id=") {
        let abs = search_from + pos;
        let after = &text[abs + "tg://user?id=".len()..];
        let raw_form = abs > 0 && bytes[abs - 1] == b'<';
        let id = mention_id(after, raw_form);
        if !is_valid_mention_id(id) {
            return Err(TeleError::Usage(format!(
                "invalid tg://user?id= mention in --text: id must be a positive number (got {id:?})"
            )));
        }
        search_from = abs + "tg://user?id=".len();
    }
    Ok(())
}

fn mention_id(after: &str, raw_form: bool) -> &str {
    if let Some(rest) = after.strip_prefix('<') {
        return rest.split('>').next().unwrap_or("");
    }
    if raw_form {
        return after.split('>').next().unwrap_or("");
    }
    let mut end = 0;
    let mut depth = 0i32;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => break,
            ')' => depth -= 1,
            c if c.is_whitespace() => break,
            _ => {}
        }
        end = i + c.len_utf8();
    }
    &after[..end]
}

fn is_valid_mention_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
        && matches!(id.parse::<i64>(), Ok(v) if v > 0)
}

fn is_reserved_device_name(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    if matches!(lower.as_str(), "con" | "prn" | "aux" | "nul") {
        return true;
    }
    lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower
            .as_bytes()
            .get(3)
            .is_some_and(|b| (b'1'..=b'9').contains(b))
}

fn validate_filename(name: &str) -> TeleResult<()> {
    if name.ends_with([' ', '.']) {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: name ends with a character Windows would strip"
        )));
    }
    if name.contains(':') {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: ':' is not allowed in a Windows file name"
        )));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved_device_name(stem) {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: reserved Windows device name"
        )));
    }
    Ok(())
}

pub fn validate_upload_path(path: &str) -> TeleResult<()> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    validate_filename(base)?;
    let app_dir = crate::config::app_data_dir();
    let canonical = canonical_guard_path(path);
    if canonical.starts_with(&app_dir) {
        return Err(TeleError::Usage(
            "refusing to upload a file from the telecli app data directory".to_string(),
        ));
    }
    let lower = base.to_ascii_lowercase();
    if lower == ".env" || lower.ends_with(".session") || lower.ends_with(".session-journal") {
        return Err(TeleError::Usage(format!(
            "refusing to upload sensitive file {base}"
        )));
    }
    Ok(())
}

fn validate_download_dir(dir: &str) -> TeleResult<()> {
    let app_dir = crate::config::app_data_dir();
    let sessions_dir = crate::session::session_dir();
    let canonical = canonical_guard_path(dir);
    if canonical.starts_with(&app_dir) || canonical.starts_with(&sessions_dir) {
        return Err(TeleError::Usage(
            "refusing to download into the telecli app data directory".to_string(),
        ));
    }
    Ok(())
}

fn canonical_guard_path(path: &str) -> std::path::PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    #[cfg(windows)]
    let canonical = strip_verbatim_prefix(canonical);
    canonical
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => std::path::PathBuf::from(rest),
        None => path,
    }
}

fn parse_schedule(value: Option<&str>) -> TeleResult<Option<i32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let dt = crate::commands::parse_unixtime(value)?;
    let ts = dt.timestamp();
    if ts <= chrono::Utc::now().timestamp() {
        return Err(TeleError::Usage(format!(
            "--schedule must be a future time (got {value})"
        )));
    }
    i32::try_from(ts).map(Some).map_err(|_| {
        TeleError::Usage(format!(
            "--schedule {value} is out of range (max 2038-01-19 03:14:07 UTC)"
        ))
    })
}

async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let schedule = parse_schedule(args.schedule.as_deref())?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let text = args.text.clone();
        let caption = args.caption.clone();
        let file = args.file.clone();
        let format = args.format.clone();
        let schedule = schedule.map(|s| s as u64);
        let reply = args.reply;
        let preview = args.preview;
        let silent = args.silent;
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": chat_target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut msg = if let Some(path) = &file {
                let uploaded = guard
                    .client
                    .upload_file(path)
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))?;
                let base = InputMessage::new().text(caption.unwrap_or_default());
                if looks_like_image(path) {
                    base.photo(uploaded)
                } else {
                    base.document(uploaded)
                }
            } else {
                let text = text.as_deref().unwrap_or_default();
                let base = match format.as_str() {
                    "markdown" => InputMessage::new().markdown(text),
                    _ => InputMessage::new().text(text),
                };
                base.link_preview(preview)
            };
            if let Some(s) = schedule {
                let ts = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s);
                msg = msg.schedule_date(Some(ts));
            }
            msg = msg.reply_to(reply);
            if silent {
                msg = msg.silent(true);
            }
            let sent = guard
                .client
                .send_message(chat_ref, msg)
                .await
                .map_err(tele_invocation)?;
            crate::serialize::message_to_json(&sent)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let text = args.text.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "id": id}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            guard
                .client
                .edit_message(chat_ref, id, InputMessage::new().text(text))
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"id": id, "edited": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_delete(args: &DeleteArgs) -> TeleResult<()> {
    if !args.all && args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids required unless --all is used".to_string(),
        ));
    }
    Ok(())
}

async fn delete(args: DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_delete(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let all = args.all;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let ids = args.ids.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "ids": ids}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            if all {
                let mut iter = guard.client.iter_messages(chat_ref);
                let mut count = 0usize;
                let mut batch: Vec<i32> = Vec::with_capacity(100);
                while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                    batch.push(msg.id());
                    if batch.len() >= 100 {
                        count += guard
                            .client
                            .delete_messages(chat_ref, &batch)
                            .await
                            .map_err(tele_invocation)?;
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    count += guard
                        .client
                        .delete_messages(chat_ref, &batch)
                        .await
                        .map_err(tele_invocation)?;
                }
                return Ok(serde_json::json!({"deleted": count}));
            }
            if ids.is_empty() {
                return Ok(serde_json::json!({"deleted": 0}));
            }
            let mut count = 0usize;
            for chunk in batches(&ids) {
                count += guard
                    .client
                    .delete_messages(chat_ref, chunk)
                    .await
                    .map_err(tele_invocation)?;
            }
            Ok(serde_json::json!({"deleted": count}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_forward(args: &ForwardArgs) -> TeleResult<()> {
    if args.ids.is_empty() {
        return Err(TeleError::Usage("--ids required".to_string()));
    }
    Ok(())
}

async fn forward(args: ForwardArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_forward(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let silent = args.silent;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let from_target = args.from.clone();
        let to_target = args.to.clone();
        let ids = args.ids.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "ids": ids}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let from = entities::resolve_peer(&guard.client, guard.session.as_ref(), &from_target)
                .await
                .map_err(tele_invocation)?;
            let to = entities::resolve_peer(&guard.client, guard.session.as_ref(), &to_target)
                .await
                .map_err(tele_invocation)?;
            let from_ref = entities::peer_ref(&from).await.map_err(tele_invocation)?;
            let to_ref = entities::peer_ref(&to).await.map_err(tele_invocation)?;
            let mut msgs: Vec<serde_json::Value> = Vec::new();
            for chunk in batches(&ids) {
                let sent = if silent {
                    let from_peer = entities::input_peer(&from).await.map_err(tele_invocation)?;
                    let to_peer = entities::input_peer(&to).await.map_err(tele_invocation)?;
                    forward_silent(&guard.client, to_ref, from_peer, to_peer, chunk).await?
                } else {
                    guard
                        .client
                        .forward_messages(to_ref, chunk, from_ref)
                        .await
                        .map_err(tele_invocation)?
                };
                msgs.extend(
                    sent.iter()
                        .filter_map(|m| m.as_ref())
                        .map(crate::serialize::message_to_json)
                        .collect::<TeleResult<Vec<_>>>()?,
                );
            }
            Ok(serde_json::json!({"forwarded": msgs}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn batches(ids: &[i32]) -> Vec<&[i32]> {
    ids.chunks(100).collect()
}

static RANDOM_ID: AtomicI64 = AtomicI64::new(0);

fn random_ids(count: usize) -> Vec<i64> {
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
        .max(1);
    RANDOM_ID.fetch_max(seed, Ordering::SeqCst);
    (0..count)
        .map(|_| RANDOM_ID.fetch_add(1, Ordering::SeqCst))
        .collect()
}

fn forwarded_ids(
    updates: &grammers_client::tl::enums::Updates,
    random_ids: &[i64],
) -> TeleResult<Vec<i32>> {
    let response_updates: &[grammers_client::tl::enums::Update] = match updates {
        grammers_client::tl::enums::Updates::Updates(u) => &u.updates,
        grammers_client::tl::enums::Updates::Combined(u) => &u.updates,
        _ => &[],
    };
    let mut new_ids = Vec::new();
    for update in response_updates {
        if let grammers_client::tl::enums::Update::MessageId(m) = update {
            if random_ids.contains(&m.random_id) {
                new_ids.push(m.id);
            }
        }
    }
    if new_ids.is_empty() {
        return Err(TeleError::Other(
            "forward succeeded but no new message ids were reported".to_string(),
        ));
    }
    Ok(new_ids)
}

async fn forward_silent(
    client: &grammers_client::Client,
    to_ref: grammers_session::types::PeerRef,
    from_peer: grammers_client::tl::enums::InputPeer,
    to_peer: grammers_client::tl::enums::InputPeer,
    ids: &[i32],
) -> TeleResult<Vec<Option<grammers_client::message::Message>>> {
    let random_ids = random_ids(ids.len());
    let request = grammers_client::tl::functions::messages::ForwardMessages {
        silent: true,
        background: false,
        with_my_score: false,
        drop_author: false,
        drop_media_captions: false,
        from_peer,
        id: ids.to_vec(),
        random_id: random_ids.clone(),
        to_peer,
        top_msg_id: None,
        reply_to: None,
        schedule_date: None,
        schedule_repeat_period: None,
        send_as: None,
        noforwards: false,
        quick_reply_shortcut: None,
        allow_paid_floodskip: false,
        effect: None,
        video_timestamp: None,
        allow_paid_stars: None,
        suggested_post: None,
    };
    let updates = client.invoke(&request).await.map_err(tele_invocation)?;
    let new_ids = forwarded_ids(&updates, &random_ids)?;
    client
        .get_messages_by_id(to_ref, &new_ids)
        .await
        .map_err(tele_invocation)
}

async fn pin(args: PinArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let unpin = args.unpin;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "id": id, "unpin": unpin}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let result = if unpin {
                guard.client.unpin_message(chat_ref, id).await
            } else {
                guard.client.pin_message(chat_ref, id).await
            };
            result.map_err(tele_invocation)?;
            Ok(serde_json::json!({"id": id, "pinned": !unpin}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let limit = args.limit as usize;
    let offset_id = args.offset_id;
    let last = args.last;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": chat_target,
                    "limit": limit,
                    "offset_id": offset_id,
                    "last": last,
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut iter = guard.client.iter_messages(chat_ref);
            if let Some(offset) = offset_id {
                iter = iter.offset_id(offset);
            }
            if last {
                iter = iter.limit(1);
            } else {
                iter = iter.limit(limit);
            }
            let mut rows: Vec<serde_json::Value> = Vec::new();
            while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                rows.push(crate::serialize::message_to_json(&msg)?);
            }
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["date"].as_str().unwrap_or_default().to_string(),
                            r["sender"]["name"].as_str().unwrap_or_default().to_string(),
                            truncate_text(r["text"].as_str().unwrap_or_default(), 80),
                        ]
                    })
                    .collect();
                output::print_table(&["id", "date", "sender", "text"], &table_rows);
            }
            Ok(serde_json::json!({"messages": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

async fn read(args: ReadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let mark_unread = args.mark_unread;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "unread": mark_unread}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            if mark_unread {
                let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                let dialog = grammers_client::tl::enums::InputDialogPeer::Peer(
                    grammers_client::tl::types::InputDialogPeer { peer },
                );
                guard
                    .client
                    .invoke(
                        &grammers_client::tl::functions::messages::MarkDialogUnread {
                            unread: true,
                            parent_peer: None,
                            peer: dialog,
                        },
                    )
                    .await
                    .map_err(tele_invocation)?;
            } else {
                let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
                guard
                    .client
                    .mark_as_read(chat_ref)
                    .await
                    .map_err(tele_invocation)?;
            }
            Ok(serde_json::json!({"unread": mark_unread}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_react(args: &ReactArgs) -> TeleResult<()> {
    if args.reaction.is_none() && !args.remove {
        return Err(TeleError::Usage(
            "--reaction <emoji> or --remove required".to_string(),
        ));
    }
    Ok(())
}

async fn react(args: ReactArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_react(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let remove = args.remove;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let reaction = args.reaction.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "id": id}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            use grammers_client::message::InputReactions;
            let input = if remove {
                InputReactions::remove()
            } else if let Some(r) = &reaction {
                InputReactions::emoticon(r)
            } else {
                return Err(TeleError::Usage(
                    "--reaction <emoji> or --remove required".to_string(),
                ));
            };
            guard
                .client
                .send_reactions(chat_ref, id, input)
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"id": id, "reaction": reaction}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn search(args: SearchArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let limit = args.limit as usize;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let query = args.query.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": chat_target,
                    "query": query,
                    "limit": limit,
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut iter = guard
                .client
                .search_messages(chat_ref)
                .query(&query)
                .limit(limit);
            let mut rows: Vec<serde_json::Value> = Vec::new();
            while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                rows.push(crate::serialize::message_to_json(&msg)?);
            }
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["date"].as_str().unwrap_or_default().to_string(),
                            r["sender"]["name"].as_str().unwrap_or_default().to_string(),
                            truncate_text(r["text"].as_str().unwrap_or_default(), 80),
                        ]
                    })
                    .collect();
                output::print_table(&["id", "date", "sender", "text"], &table_rows);
            }
            Ok(serde_json::json!({"messages": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn download(args: DownloadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_download_dir(&args.dir)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let out_dir = args.dir.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "id": id}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let found = guard
                .client
                .get_messages_by_id(chat_ref, &[id])
                .await
                .map_err(tele_invocation)?;
            let msg = found
                .into_iter()
                .flatten()
                .next()
                .ok_or_else(|| TeleError::Usage(format!("message {id} not found")))?;
            let name = download_name(&msg);
            tokio::task::spawn_blocking({
                let out_dir = out_dir.clone();
                move || std::fs::create_dir_all(&out_dir)
            })
            .await
            .map_err(|e| TeleError::Other(e.to_string()))??;
            let path = std::path::Path::new(&out_dir).join(name);
            refuse_existing_download_target(&path)?;
            let temp = download_temp_path(&path);
            tokio::task::spawn_blocking({
                let temp = temp.clone();
                move || create_download_temp(&temp)
            })
            .await
            .map_err(|e| TeleError::Other(e.to_string()))??;
            let ok = msg.download_media(&temp).await.map_err(|e| {
                let _ = std::fs::remove_file(&temp);
                tele_invocation(e)
            })?;
            if !ok {
                let _ = std::fs::remove_file(&temp);
                return Err(TeleError::Usage("message has no media".to_string()));
            }
            commit_download(&temp, &path)?;
            let bytes = tokio::task::spawn_blocking({
                let path = path.clone();
                move || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            })
            .await
            .map_err(|e| TeleError::Other(e.to_string()))?;
            Ok(serde_json::json!({"path": path.to_string_lossy(), "bytes": bytes}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn download_name(msg: &grammers_client::message::Message) -> String {
    use grammers_client::media::Media;
    let name = match msg.media() {
        Some(Media::Document(d)) => d.name().map(str::to_string).unwrap_or_default(),
        Some(Media::Photo(_)) => "photo.jpg".to_string(),
        Some(Media::Sticker(_)) => "sticker.webp".to_string(),
        Some(_) => "media.bin".to_string(),
        None => "media.bin".to_string(),
    };
    sanitize_download_name(&name)
}

fn sanitize_download_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = String::with_capacity(base.len());
    for c in base.chars() {
        out.push(match c {
            '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        });
    }
    let trimmed = out.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        "document.bin".to_string()
    } else if is_reserved_device_name(trimmed.split('.').next().unwrap_or(trimmed)) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn download_temp_path(final_path: &std::path::Path) -> std::path::PathBuf {
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    final_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".{name}.part-{}", std::process::id()))
}

fn refuse_existing_download_target(path: &std::path::Path) -> TeleResult<()> {
    if path.exists() {
        return Err(TeleError::Usage(format!(
            "download target exists: {}",
            path.display()
        )));
    }
    Ok(())
}

fn create_download_temp(temp: &std::path::Path) -> TeleResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map(|_| ())
        .map_err(|e| TeleError::Other(format!("cannot create temp file {}: {e}", temp.display())))
}

fn commit_download(temp: &std::path::Path, final_path: &std::path::Path) -> TeleResult<()> {
    match std::fs::rename(temp, final_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(temp);
            Err(TeleError::Other(format!(
                "cannot move download into place at {}: {e}",
                final_path.display()
            )))
        }
    }
}

fn looks_like_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_args(format: &str) -> SendArgs {
        SendArgs {
            chat: "me".to_string(),
            text: Some("hi".to_string()),
            schedule: None,
            file: None,
            caption: None,
            reply: None,
            preview: true,
            format: format.to_string(),
            silent: false,
        }
    }

    #[test]
    fn upload_rejects_sensitive_basenames() {
        assert!(matches!(
            validate_upload_path("C:/secrets/.env"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("C:/telecli-data/1.session"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("2.session-journal"),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn upload_allows_regular_files() {
        assert!(validate_upload_path("C:/tmp/report.pdf").is_ok());
    }

    #[test]
    fn send_rejects_unknown_format() {
        assert!(matches!(
            validate_send(&send_args("html")),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_send(&send_args("plain")).is_ok());
        assert!(validate_send(&send_args("markdown")).is_ok());
    }

    #[test]
    fn send_rejects_text_with_file() {
        let mut args = send_args("plain");
        args.file = Some("C:/tmp/a.pdf".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_rejects_caption_without_file() {
        let mut args = send_args("plain");
        args.caption = Some("cap".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_accepts_file_with_caption() {
        let mut args = send_args("plain");
        args.text = None;
        args.file = Some("C:/tmp/a.pdf".to_string());
        args.caption = Some("cap".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_rejects_non_numeric_mention_ids() {
        let mut args = send_args("markdown");
        for bad in [
            "[x](tg://user?id=abc)",
            "[x](tg://user?id=12abc)",
            "[x](tg://user?id=)",
            "plain text with tg://user?id=abc",
            "<tg://user?id=-1>",
        ] {
            args.text = Some(bad.to_string());
            assert!(
                matches!(validate_send(&args), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_numeric_mention_ids() {
        let mut args = send_args("markdown");
        for good in [
            "[x](tg://user?id=12345678)",
            "<tg://user?id=999>",
            "[a](https://example.com) **bold**",
            "plain text",
        ] {
            args.text = Some(good.to_string());
            assert!(validate_send(&args).is_ok(), "{good}");
        }
    }

    #[test]
    fn upload_rejects_sensitive_basenames_case_insensitive() {
        assert!(matches!(
            validate_upload_path("C:/secrets/.ENV"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("C:/telecli-data/1.SESSION"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("2.Session-Journal"),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn schedule_absent_is_none() {
        assert_eq!(parse_schedule(None).unwrap(), None);
    }

    #[test]
    fn schedule_rejects_past_timestamps() {
        for v in ["0", "-5", "1000000", "1970-01-01T00:00:01Z"] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_rejects_beyond_i32_range() {
        for v in [
            "2147483648",
            "9999999999",
            "2038-01-19T03:14:08Z",
            "2100-01-01T00:00:00Z",
        ] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_rejects_garbage() {
        assert!(matches!(
            parse_schedule(Some("not a date")),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn schedule_accepts_future_values() {
        let future_ts = chrono::Utc::now().timestamp() + 3600;
        let ts = parse_schedule(Some(&future_ts.to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(ts, future_ts as i32);
        let future_rfc = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        assert!(parse_schedule(Some(&future_rfc)).unwrap().is_some());
    }

    #[test]
    fn delete_batches_ids_by_100() {
        let ids: Vec<i32> = (1..=250).collect();
        let parts = batches(&ids);
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts.iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![100, 100, 50]
        );
        let exactly_100: Vec<i32> = (1..=100).collect();
        assert_eq!(batches(&exactly_100).len(), 1);
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn random_ids_are_unique_positive_and_increasing() {
        let a = random_ids(5);
        let b = random_ids(5);
        assert_eq!(a.len(), 5);
        assert_eq!(b.len(), 5);
        assert!(a.iter().all(|&x| x > 0));
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        assert!(b.windows(2).all(|w| w[0] < w[1]));
        let mut all = a;
        all.extend_from_slice(&b);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 10);
    }

    fn message_id_update(random_id: i64, id: i32) -> grammers_client::tl::enums::Update {
        grammers_client::tl::enums::Update::MessageId(grammers_client::tl::types::UpdateMessageId {
            id,
            random_id,
        })
    }

    fn plain_updates(
        updates: Vec<grammers_client::tl::enums::Update>,
    ) -> grammers_client::tl::enums::Updates {
        grammers_client::tl::enums::Updates::Updates(grammers_client::tl::types::Updates {
            updates,
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq: 0,
        })
    }

    fn combined_updates(
        updates: Vec<grammers_client::tl::enums::Update>,
    ) -> grammers_client::tl::enums::Updates {
        grammers_client::tl::enums::Updates::Combined(grammers_client::tl::types::UpdatesCombined {
            updates,
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq_start: 0,
            seq: 0,
        })
    }

    #[test]
    fn forwarded_ids_collect_matching_ids_from_plain_updates() {
        let updates = plain_updates(vec![
            message_id_update(101, 7),
            message_id_update(999, 9),
            message_id_update(102, 8),
        ]);
        assert_eq!(forwarded_ids(&updates, &[101, 102]).unwrap(), vec![7, 8]);
    }

    #[test]
    fn forwarded_ids_collect_matching_ids_from_combined_updates() {
        let updates = combined_updates(vec![message_id_update(101, 7), message_id_update(999, 9)]);
        assert_eq!(forwarded_ids(&updates, &[101]).unwrap(), vec![7]);
    }

    #[test]
    fn forwarded_ids_error_when_no_matching_ids_extracted() {
        let updates = plain_updates(vec![message_id_update(999, 9)]);
        assert!(matches!(
            forwarded_ids(&updates, &[101]),
            Err(TeleError::Other(_))
        ));
        let empty = combined_updates(Vec::new());
        assert!(forwarded_ids(&empty, &[101]).is_err());
    }

    #[test]
    fn delete_requires_ids_unless_all() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: Vec::new(),
            all: false,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
        let mut with_all = args;
        with_all.all = true;
        assert!(validate_delete(&with_all).is_ok());
        let with_ids = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1, 2],
            all: false,
        };
        assert!(validate_delete(&with_ids).is_ok());
    }

    #[test]
    fn forward_requires_ids() {
        let args = ForwardArgs {
            from: "a".to_string(),
            ids: Vec::new(),
            to: "b".to_string(),
            silent: false,
        };
        assert!(matches!(validate_forward(&args), Err(TeleError::Usage(_))));
        let mut with_ids = args;
        with_ids.ids = vec![3];
        assert!(validate_forward(&with_ids).is_ok());
    }

    #[test]
    fn react_requires_reaction_or_remove() {
        let none = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: None,
            remove: false,
        };
        assert!(matches!(validate_react(&none), Err(TeleError::Usage(_))));
        let with_remove = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: None,
            remove: true,
        };
        assert!(validate_react(&with_remove).is_ok());
        let with_reaction = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: Some("+1".to_string()),
            remove: false,
        };
        assert!(validate_react(&with_reaction).is_ok());
    }

    #[test]
    fn markdown_accepts_angle_bracket_link_dest() {
        let mut args = send_args("markdown");
        args.text = Some("[x](<tg://user?id=12345678>)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_rejects_junk_embedded_in_bare_link_dest() {
        let mut args = send_args("markdown");
        for bad in [
            "[x](tg://user?id=123\"456)",
            "[x](tg://user?id=123>456)",
            "[x](tg://user?id=123<456)",
            "[x](tg://user?id=123+456)",
        ] {
            args.text = Some(bad.to_string());
            assert!(
                matches!(validate_send(&args), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_id_terminated_by_space_like_grammers() {
        let mut args = send_args("markdown");
        args.text = Some("[x](tg://user?id=123 456)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_validates_every_link_in_text() {
        let mut args = send_args("markdown");
        args.text = Some("[a](tg://user?id=1)[b](tg://user?id=abc)".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.text = Some("[a](tg://user?id=1)[b](tg://user?id=2)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_rejects_overflowing_and_accepts_max_mention_ids() {
        let mut args = send_args("markdown");
        args.text = Some("[x](tg://user?id=99999999999999999999999)".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.text = Some("[x](tg://user?id=9223372036854775807)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn schedule_rejects_now_and_recent_past() {
        let now = chrono::Utc::now().timestamp();
        for v in [(now - 1).to_string(), now.to_string(), "+3600".to_string()] {
            assert!(
                matches!(parse_schedule(Some(&v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_accepts_max_i32_boundary() {
        assert_eq!(
            parse_schedule(Some("2147483647")).unwrap().unwrap(),
            i32::MAX
        );
    }

    #[test]
    fn schedule_rejects_empty_and_leap_second_strings() {
        for v in ["", "2016-12-31T23:59:60Z"] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v:?}"
            );
        }
    }

    #[test]
    fn schedule_normalizes_rfc3339_offsets_before_comparing() {
        let future = chrono::Utc::now() + chrono::Duration::hours(2);
        let with_offset = future
            .with_timezone(&chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap())
            .to_rfc3339();
        assert!(parse_schedule(Some(&with_offset)).unwrap().is_some());
        let past = chrono::Utc::now() - chrono::Duration::hours(2);
        let with_offset = past
            .with_timezone(&chrono::FixedOffset::west_opt(8 * 3600).unwrap())
            .to_rfc3339();
        assert!(matches!(
            parse_schedule(Some(&with_offset)),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn batches_splits_101_into_two_and_1000_into_ten() {
        let ids: Vec<i32> = (1..=101).collect();
        assert_eq!(
            batches(&ids).iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![100, 1]
        );
        let ids: Vec<i32> = (1..=1000).collect();
        let parts = batches(&ids);
        assert_eq!(parts.len(), 10);
        assert!(parts.iter().all(|b| b.len() == 100));
    }

    #[test]
    fn upload_rejects_any_sensitive_basename_and_allows_lookalikes() {
        for bad in [
            "a.session",
            "a.SESSION",
            "a.session-journal",
            ".ENV",
            ".env",
        ] {
            assert!(
                matches!(
                    validate_upload_path(&format!("C:/tmp/{bad}")),
                    Err(TeleError::Usage(_))
                ),
                "{bad}"
            );
        }
        for good in ["C:/tmp/session", "C:/tmp/env", "C:/tmp/x.env"] {
            assert!(validate_upload_path(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn upload_rejects_windows_alias_names() {
        for bad in [
            "file.txt.",
            "file.txt ",
            "file.txt:stream",
            "CON",
            "con.txt",
            "LPT1",
            "COM9",
            "NUL",
            "AUX",
            "PRN",
            "COM1",
            "LPT9",
        ] {
            assert!(
                matches!(
                    validate_upload_path(&format!("C:/tmp/{bad}")),
                    Err(TeleError::Usage(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn upload_allows_windows_lookalike_names() {
        for good in ["file.txt", "CONs", "COM10", "report.txt", "com0", "lpt10"] {
            assert!(
                validate_upload_path(&format!("C:/tmp/{good}")).is_ok(),
                "{good}"
            );
        }
    }

    #[test]
    fn validate_filename_rejects_windows_aliases() {
        for bad in [
            "file.txt.",
            "file.txt ",
            "file.txt:stream",
            "CON",
            "con.txt",
            "LPT1",
            "COM9",
            "NUL",
            "aux",
            "prn",
            "com1",
            "lpt9",
        ] {
            assert!(
                matches!(validate_filename(bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn validate_filename_accepts_safe_names() {
        for good in [
            "file.txt",
            "CONs",
            "COM10",
            "report.txt",
            "com0",
            "lpt10",
            "a.b.c",
        ] {
            assert!(validate_filename(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn reserved_device_names_match_exact_base_names() {
        for stem in [
            "con", "CON", "prn", "aux", "nul", "com1", "COM9", "lpt1", "LPT9",
        ] {
            assert!(is_reserved_device_name(stem), "{stem}");
        }
        for stem in [
            "cons", "console", "com0", "com10", "lpt0", "lpt10", "aux2", "",
        ] {
            assert!(!is_reserved_device_name(stem), "{stem}");
        }
    }

    #[test]
    fn sanitize_download_name_strips_parent_traversal() {
        assert_eq!(sanitize_download_name("../x"), "x");
        assert_eq!(sanitize_download_name("a/../b"), "b");
        assert_eq!(sanitize_download_name("a\\..\\b"), "b");
        assert_eq!(sanitize_download_name(".."), "document.bin");
        assert_eq!(sanitize_download_name("."), "document.bin");
    }

    #[test]
    fn sanitize_download_name_strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_download_name("name.."), "name");
        assert_eq!(sanitize_download_name("name "), "name");
        assert_eq!(sanitize_download_name("name. "), "name");
        assert_eq!(sanitize_download_name("..."), "document.bin");
    }

    #[test]
    fn sanitize_download_name_keeps_valid_names_unchanged() {
        assert_eq!(sanitize_download_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_download_name("عکس.png"), "عکس.png");
        assert_eq!(sanitize_download_name("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn sanitize_download_name_falls_back_on_empty() {
        assert_eq!(sanitize_download_name(""), "document.bin");
    }

    #[test]
    fn sanitize_download_name_maps_reserved_device_names() {
        for (input, expected) in [
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("NUL", "_NUL"),
            ("nul.log", "_nul.log"),
            ("PRN", "_PRN"),
            ("AUX", "_AUX"),
            ("COM1", "_COM1"),
            ("com3.bin", "_com3.bin"),
            ("LPT9", "_LPT9"),
            ("lpt5.txt", "_lpt5.txt"),
            ("CON.", "_CON"),
            ("COM10", "COM10"),
            ("LPT10", "LPT10"),
            ("conference.pdf", "conference.pdf"),
            ("company.1", "company.1"),
        ] {
            assert_eq!(sanitize_download_name(input), expected, "{input}");
        }
    }

    #[test]
    fn download_temp_path_is_dot_prefixed_in_same_dir() {
        let dir = std::env::temp_dir().join(format!("telecli-dl-tmp-{}", std::process::id()));
        let final_path = dir.join("report.pdf");
        let temp = download_temp_path(&final_path);
        assert_eq!(temp.parent().unwrap(), dir);
        let fname = temp.file_name().unwrap().to_string_lossy().to_string();
        assert!(fname.starts_with(".report.pdf.part-"), "{fname}");
        assert!(fname
            .strip_prefix(".report.pdf.part-")
            .unwrap()
            .parse::<u32>()
            .is_ok());
    }

    #[test]
    fn refuse_existing_download_target_rejects_existing_file() {
        let base = std::env::temp_dir().join(format!("telecli-dl-exists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let existing = base.join("x.bin");
        std::fs::write(&existing, b"old").unwrap();
        assert!(matches!(
            refuse_existing_download_target(&existing),
            Err(TeleError::Usage(_))
        ));
        let fresh = base.join("y.bin");
        assert!(refuse_existing_download_target(&fresh).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_download_temp_refuses_existing_temp() {
        let base = std::env::temp_dir().join(format!("telecli-dl-ct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".a.part-1");
        assert!(create_download_temp(&temp).is_ok());
        assert!(matches!(
            create_download_temp(&temp),
            Err(TeleError::Other(_))
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_download_moves_temp_into_place() {
        let base = std::env::temp_dir().join(format!("telecli-dl-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".x.bin.part-1");
        let final_path = base.join("x.bin");
        std::fs::write(&temp, b"payload").unwrap();
        commit_download(&temp, &final_path).unwrap();
        assert_eq!(std::fs::read(&final_path).unwrap(), b"payload");
        assert!(!temp.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_download_cleans_temp_on_failure() {
        let base = std::env::temp_dir().join(format!("telecli-dl-cfail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".x.bin.part-1");
        let occupied = base.join("occupied");
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(&temp, b"payload").unwrap();
        assert!(commit_download(&temp, &occupied).is_err());
        assert!(!temp.exists());
        assert!(occupied.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn download_dir_rejects_app_data_and_sessions() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sessions")).unwrap();
        std::env::set_var("TELE_APP_DIR", &base);
        for bad in [
            base.to_string_lossy().to_string(),
            base.join("sessions").to_string_lossy().to_string(),
            base.join("sessions")
                .join("new")
                .to_string_lossy()
                .to_string(),
            base.join("export").to_string_lossy().to_string(),
            base.join("not-yet-created").to_string_lossy().to_string(),
        ] {
            assert!(
                matches!(validate_download_dir(&bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn download_dir_allows_dirs_outside_app_data() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("TELE_APP_DIR", &base);
        let sibling = std::env::temp_dir().join(format!(
            "telecli-msg-dl-guard-{}-outside",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&sibling);
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(validate_download_dir(&sibling.to_string_lossy()).is_ok());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK.lock().await
    }

    fn dryrun_flags(command: &str, dry_run: bool) -> GlobalFlags {
        GlobalFlags {
            account: vec!["me".to_string()],
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run,
            quiet: true,
            config_path: None,
            command: command.to_string(),
        }
    }

    fn fake_app_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-msg-dryrun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions").join("me.session"), b"").unwrap();
        dir
    }

    #[tokio::test]
    async fn get_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = get(
            GetArgs {
                chat: "me".to_string(),
                limit: 10,
                offset_id: None,
                last: false,
            },
            &dryrun_flags("msg get", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn get_without_dry_run_requires_a_real_session() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = get(
            GetArgs {
                chat: "me".to_string(),
                limit: 10,
                offset_id: None,
                last: false,
            },
            &dryrun_flags("msg get", false),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(code, 0);
    }

    #[tokio::test]
    async fn search_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = search(
            SearchArgs {
                chat: "me".to_string(),
                query: "hello".to_string(),
                limit: 10,
            },
            &dryrun_flags("msg search", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn search_without_dry_run_requires_a_real_session() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = search(
            SearchArgs {
                chat: "me".to_string(),
                query: "hello".to_string(),
                limit: 10,
            },
            &dryrun_flags("msg search", false),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(code, 0);
    }

    #[tokio::test]
    async fn download_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let out =
            std::env::temp_dir().join(format!("telecli-msg-dl-dryrun-{}", std::process::id()));
        let code = download(
            DownloadArgs {
                chat: "me".to_string(),
                id: 1,
                dir: out.to_string_lossy().to_string(),
            },
            &dryrun_flags("msg download", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }
}
