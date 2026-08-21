use clap::{Args, Subcommand};
use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::require_chat_target;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::fs_util::{path_under_guard, resolve_for_guard};
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

#[derive(Args, Clone)]
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
    #[arg(long, action = clap::ArgAction::SetTrue, help = "disable link preview")]
    no_preview: bool,
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
    #[arg(
        long,
        help = "delete only for yourself (no revoke; private chats and basic groups only, not channels)"
    )]
    self_only: bool,
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
}

#[derive(Args)]
pub struct PinArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "message ID to pin or unpin")]
    id: i32,
    #[arg(long, help = "remove pin instead of adding")]
    unpin: bool,
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
    #[arg(long, help = "overwrite existing files")]
    force: bool,
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

fn effective_preview(args: &SendArgs) -> bool {
    args.preview && !args.no_preview
}

fn validate_send(args: &SendArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
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
    if let Some(text) = &args.text {
        if text.trim().is_empty() {
            return Err(TeleError::Usage("--text must not be empty".to_string()));
        }
    }
    if args.caption.is_some() && args.file.is_none() {
        return Err(TeleError::Usage("--caption requires --file".to_string()));
    }
    if let Some(path) = &args.file {
        if !effective_preview(args) {
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
        if let Some(caption) = &args.caption {
            validate_markdown(caption)?;
        }
    }
    Ok(())
}

fn validate_markdown(text: &str) -> TeleResult<()> {
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("tg://user?id=") {
        let abs = search_from + pos;
        let prev = text[..abs].chars().last();
        let genuine = match prev {
            None => true,
            Some(c) => c == '(' || c == '<' || c.is_whitespace(),
        };
        if !genuine {
            search_from = abs + "tg://user?id=".len();
            continue;
        }
        let after = &text[abs + "tg://user?id=".len()..];
        let id = mention_id(after, prev == Some('<'));
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

const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn check_upload_size(bytes: u64) -> TeleResult<()> {
    if bytes > MAX_UPLOAD_BYTES {
        return Err(TeleError::Usage(format!(
            "refusing to upload file larger than 2 GiB (got {bytes} bytes)"
        )));
    }
    Ok(())
}

pub fn validate_upload_path(path: &str) -> TeleResult<()> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    validate_filename(base)?;
    let app_dir = canonical_guard_path(&crate::config::app_data_dir().to_string_lossy());
    let canonical = canonical_guard_path(path);
    if path_under_guard(&canonical, &app_dir) {
        return Err(TeleError::Usage(
            "refusing to upload a file from the telecli app data directory".to_string(),
        ));
    }
    let lower = base.to_lowercase();
    if is_sensitive_basename(&lower) {
        return Err(TeleError::Usage(format!(
            "refusing to upload sensitive file {base}"
        )));
    }
    let path = std::path::Path::new(path);
    if !path.is_file() {
        return Err(TeleError::Usage(format!("upload file not found: {path:?}")));
    }
    check_upload_size(std::fs::metadata(path)?.len())
}

pub fn is_sensitive_basename(lower: &str) -> bool {
    const SUFFIXES: [&str; 7] = [
        ".session",
        ".session-journal",
        ".pem",
        ".key",
        ".p12",
        ".pfx",
        ".kdbx",
    ];
    const PREFIXES: [&str; 6] = [
        ".env",
        "config.toml",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "id_dsa",
    ];
    const EXACT: [&str; 3] = [".netrc", ".git-credentials", "credentials"];
    SUFFIXES.iter().any(|s| lower.ends_with(s))
        || PREFIXES.iter().any(|s| lower.starts_with(s))
        || EXACT.contains(&lower)
}

fn validate_download_dir(dir: &str) -> TeleResult<()> {
    let app_dir = canonical_guard_path(&crate::config::app_data_dir().to_string_lossy());
    let sessions_dir = canonical_guard_path(&crate::session::session_dir().to_string_lossy());
    let canonical = canonical_guard_path(dir);
    if path_under_guard(&canonical, &app_dir) || path_under_guard(&canonical, &sessions_dir) {
        return Err(TeleError::Usage(
            "refusing to download into the telecli app data directory".to_string(),
        ));
    }
    Ok(())
}

fn canonical_guard_path(path: &str) -> std::path::PathBuf {
    resolve_for_guard(std::path::Path::new(path))
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

fn send_dry_run_payload(args: &SendArgs, schedule: Option<u64>) -> serde_json::Value {
    let would = format!("send message to chat {}", args.chat);
    serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "text": args.text,
        "file": args.file,
        "caption": args.caption,
        "format": args.format,
        "schedule": schedule,
        "reply": args.reply,
        "preview": effective_preview(args),
        "silent": args.silent,
        "would": would,
    })
}

async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let schedule = parse_schedule(args.schedule.as_deref())?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        let chat_target = args.chat.clone();
        let text = args.text.clone();
        let caption = args.caption.clone();
        let file = args.file.clone();
        let format = args.format.clone();
        let schedule = schedule.map(|s| s as u64);
        let reply = args.reply;
        let preview = effective_preview(&args);
        let silent = args.silent;
        Box::pin(async move {
            if dry_run {
                return Ok(send_dry_run_payload(&args, schedule));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut msg = if let Some(path) = &file {
                let uploaded = guard.client.upload_file(path).await.map_err(|e| {
                    match std::error::Error::source(&e)
                        .and_then(|s| s.downcast_ref::<grammers_client::InvocationError>())
                    {
                        Some(grammers_client::InvocationError::Rpc(rpc)) if rpc.code == 420 => {
                            TeleError::Invocation(rpc.to_string(), rpc.value)
                        }
                        _ => TeleError::Other(e.to_string()),
                    }
                })?;
                let base = match format.as_str() {
                    "markdown" => InputMessage::new().markdown(caption.unwrap_or_default()),
                    _ => InputMessage::new().text(caption.unwrap_or_default()),
                };
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

fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    if args.text.trim().is_empty() {
        return Err(TeleError::Usage("--text must not be empty".to_string()));
    }
    Ok(())
}

fn edit_dry_run_payload(chat: &str, text: &str, id: i32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "id": id,
        "chat": chat,
        "text": text,
        "would": format!("edit message {id}"),
    })
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_edit(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let text = args.text.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(edit_dry_run_payload(&chat_target, &text, id));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
    require_chat_target(&args.chat, "chat")?;
    if args.all && !args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--all and --ids are mutually exclusive".to_string(),
        ));
    }
    if !args.all && args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids required unless --all is used".to_string(),
        ));
    }
    if args.all && args.self_only {
        return Err(TeleError::Usage(
            "--all and --self-only are mutually exclusive".to_string(),
        ));
    }
    Ok(())
}

fn self_only_supported(kind: grammers_session::types::PeerKind) -> bool {
    !matches!(kind, grammers_session::types::PeerKind::Channel)
}

fn delete_report(requested: usize, deleted: usize) -> (serde_json::Value, bool) {
    let partial = requested > 0 && deleted < requested;
    let mut value = serde_json::json!({"requested": requested, "deleted": deleted});
    if partial {
        value["partial"] = serde_json::json!(true);
    }
    (value, partial)
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
        let self_only = args.self_only;
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "ids": ids,
                    "self_only": self_only,
                    "would": if all {
                        format!("delete all messages in chat {chat_target}")
                    } else {
                        format!("delete {} message(s) by id", ids.len())
                    },
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            if all {
                let mut iter = guard.client.iter_messages(chat_ref);
                let mut count = 0usize;
                let mut requested = 0usize;
                let mut batch: Vec<i32> = Vec::with_capacity(100);
                while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                    requested += 1;
                    batch.push(msg.id());
                    if batch.len() >= 100 {
                        guard
                            .client
                            .delete_messages(chat_ref, &batch)
                            .await
                            .map_err(tele_invocation)?;
                        count += batch.len();
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    guard
                        .client
                        .delete_messages(chat_ref, &batch)
                        .await
                        .map_err(tele_invocation)?;
                    count += batch.len();
                }
                let (report, partial) = delete_report(requested, count);
                if partial {
                    crate::output::log_line(
                        "warn",
                        &format!("delete removed {count} of {requested} requested message(s)"),
                    );
                }
                return Ok(report);
            }
            let mut count = 0usize;
            if self_only {
                if !self_only_supported(chat.id().kind()) {
                    return Err(TeleError::Usage(
                        "--self-only is not supported in channels".to_string(),
                    ));
                }
                for chunk in batches(&ids) {
                    guard
                        .client
                        .invoke(&grammers_client::tl::functions::messages::DeleteMessages {
                            revoke: false,
                            id: chunk.to_vec(),
                        })
                        .await
                        .map_err(tele_invocation)?;
                    count += chunk.len();
                }
            } else {
                for chunk in batches(&ids) {
                    guard
                        .client
                        .delete_messages(chat_ref, chunk)
                        .await
                        .map_err(tele_invocation)?;
                    count += chunk.len();
                }
            }
            let (report, partial) = delete_report(ids.len(), count);
            if partial {
                crate::output::log_line(
                    "warn",
                    &format!("delete removed {count} of {} requested id(s)", ids.len()),
                );
            }
            Ok(report)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_forward(args: &ForwardArgs) -> TeleResult<()> {
    require_chat_target(&args.from, "from")?;
    require_chat_target(&args.to, "to")?;
    if args.ids.is_empty() {
        return Err(TeleError::Usage("--ids required".to_string()));
    }
    Ok(())
}

async fn forward(args: ForwardArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_forward(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let from_target = args.from.clone();
        let to_target = args.to.clone();
        let ids = args.ids.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "ids": ids,
                    "would": format!("forward {} message(s) to chat {to_target}", ids.len())
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let from =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &from_target).await?;
            let to =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &to_target).await?;
            let from_ref = entities::peer_ref(&from).await.map_err(tele_invocation)?;
            let to_ref = entities::peer_ref(&to).await.map_err(tele_invocation)?;
            let mut forwarded: Vec<serde_json::Value> = Vec::new();
            let mut dropped: Vec<i32> = Vec::new();
            let mut failed: Vec<i32> = Vec::new();
            for chunk in batches(&ids) {
                let sent = guard
                    .client
                    .forward_messages(to_ref, chunk, from_ref)
                    .await
                    .map_err(tele_invocation);
                match sent {
                    Ok(results) => {
                        push_forward_results(&mut forwarded, &mut dropped, chunk, results)?
                    }
                    Err(e) => {
                        crate::output::log_line(
                            "warn",
                            &format!("forward failed for {} id(s): {e}", chunk.len()),
                        );
                        failed.extend_from_slice(chunk);
                    }
                }
            }
            let (report, should_warn) = forward_report(ids.len(), &forwarded, &dropped, &failed);
            if should_warn {
                crate::output::log_line(
                    "warn",
                    &format!("forward confirmed 0 of {} requested id(s)", ids.len()),
                );
            }
            Ok(report)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn batches(ids: &[i32]) -> Vec<&[i32]> {
    ids.chunks(100).collect()
}

fn push_forward_results(
    forwarded: &mut Vec<serde_json::Value>,
    dropped: &mut Vec<i32>,
    chunk: &[i32],
    results: Vec<Option<grammers_client::message::Message>>,
) -> TeleResult<()> {
    for (i, m) in results.into_iter().enumerate() {
        match m {
            Some(m) => forwarded.push(crate::serialize::message_to_json(&m)?),
            None => {
                if let Some(id) = chunk.get(i) {
                    dropped.push(*id);
                }
            }
        }
    }
    Ok(())
}

fn forward_report(
    requested: usize,
    forwarded: &[serde_json::Value],
    dropped: &[i32],
    failed: &[i32],
) -> (serde_json::Value, bool) {
    let mut value = serde_json::json!({"requested": requested, "forwarded": forwarded});
    if !dropped.is_empty() {
        value["dropped"] = serde_json::json!({"count": dropped.len(), "ids": dropped});
    }
    if !failed.is_empty() {
        value["failed"] = serde_json::json!({"count": failed.len(), "ids": failed});
    }
    let partial = requested > 0 && forwarded.len() < requested;
    if partial {
        value["partial"] = serde_json::json!(true);
    }
    let should_warn = requested > 0 && forwarded.is_empty();
    (value, should_warn)
}

async fn pin(args: PinArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let unpin = args.unpin;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "id": id,
                    "unpin": unpin,
                    "would": format!("{} message {id}", if unpin { "unpin" } else { "pin" })
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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

fn validate_get(args: &GetArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    if args.last && args.offset_id.is_some() {
        return Err(TeleError::Usage(
            "--last and --offset-id are mutually exclusive".to_string(),
        ));
    }
    Ok(())
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_get(&args)?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let limit = args.limit as usize;
    let offset_id = args.offset_id;
    let last = args.last;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
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
                    "would": format!("get messages from chat {chat_target}"),
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "date", "sender", "text"],
                    &table_rows,
                )?;
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
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let mark_unread = args.mark_unread;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "unread": mark_unread,
                    "would": format!(
                        "mark chat {chat_target} as {}",
                        if mark_unread { "unread" } else { "read" }
                    ),
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
    require_chat_target(&args.chat, "chat")?;
    if args.reaction.is_some() && args.remove {
        return Err(TeleError::Usage(
            "use --reaction or --remove, not both".to_string(),
        ));
    }
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
                let would = if remove {
                    format!("remove reaction from message {id}")
                } else if let Some(r) = &reaction {
                    format!("react {r} to message {id}")
                } else {
                    format!("react to message {id}")
                };
                return Ok(serde_json::json!({"dry_run": true, "id": id, "would": would}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
    require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let limit = args.limit as usize;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
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
                    "would": format!("search messages in chat {chat_target}"),
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "date", "sender", "text"],
                    &table_rows,
                )?;
            }
            Ok(serde_json::json!({"messages": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn download(args: DownloadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
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
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "id": id,
                    "would": format!("download message {id}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
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
            validate_download_dir(&out_dir)?;
            let path = std::path::Path::new(&out_dir).join(name);
            if !args.force {
                refuse_existing_download_target(&path)?;
            }
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
    use grammers_session::types::PeerKind;

    fn send_args(format: &str) -> SendArgs {
        SendArgs {
            chat: "me".to_string(),
            text: Some("hi".to_string()),
            schedule: None,
            file: None,
            caption: None,
            reply: None,
            preview: true,
            no_preview: false,
            format: format.to_string(),
            silent: false,
        }
    }

    fn upload_fixture(tag: &str, names: &[&str]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telecli-msg-upload-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        dir
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("telecli-msg-{tag}-{}", std::process::id()))
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
    fn check_upload_size_accepts_boundary() {
        assert!(check_upload_size(MAX_UPLOAD_BYTES).is_ok());
    }

    #[test]
    fn check_upload_size_rejects_over_cap() {
        let err = check_upload_size(MAX_UPLOAD_BYTES + 1).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("2 GiB"));
    }

    #[test]
    fn validate_upload_path_rejects_config_toml_basename() {
        let dir = temp_path("uploadcfg");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["config.toml", "config.toml.tmp-123", "CONFIG.TOML"] {
            let path = dir.join(name);
            std::fs::write(&path, b"x").unwrap();
            let err = validate_upload_path(path.to_str().unwrap()).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "{name}: {err:?}");
            assert!(err.message().contains("sensitive"), "{name}");
        }
        let ok_path = dir.join("notes.txt");
        std::fs::write(&ok_path, b"x").unwrap();
        validate_upload_path(ok_path.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_sensitive_basename_covers_private_key_families() {
        for name in [
            "id_rsa",
            "id_rsa.old",
            "ID_ED25519",
            "id_ecdsa",
            "id_dsa",
            "server.pem",
            "CERT.KEY",
            "keystore.p12",
            "backup.pfx",
            "vault.kdbx",
            ".netrc",
            ".git-credentials",
            ".env.local",
            "work.session",
            "work.session-journal",
        ] {
            assert!(
                is_sensitive_basename(&name.to_lowercase()),
                "{name} must be blocked"
            );
        }
        for name in ["notes.txt", "report.pdf", "archive.tar.gz"] {
            assert!(
                !is_sensitive_basename(&name.to_lowercase()),
                "{name} must be allowed"
            );
        }
    }

    #[test]
    fn validate_upload_path_rejects_aws_credentials_basename() {
        let dir = temp_path("uploadaws");
        let aws = dir.join(".aws");
        std::fs::create_dir_all(&aws).unwrap();
        let path = aws.join("credentials");
        std::fs::write(&path, b"x").unwrap();
        let err = validate_upload_path(path.to_str().unwrap()).unwrap_err();
        assert!(err.message().contains("sensitive"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upload_allows_regular_files() {
        let dir = upload_fixture("regular", &["report.pdf"]);
        let file = dir.join("report.pdf");
        assert!(validate_upload_path(&file.to_string_lossy()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn send_rejects_empty_text() {
        let mut args = send_args("plain");
        args.text = Some("".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_rejects_whitespace_text() {
        let mut args = send_args("plain");
        args.text = Some("   \t ".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_allows_whitespace_inside_text() {
        let mut args = send_args("plain");
        args.text = Some("  hello world  ".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn edit_rejects_empty_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "".to_string(),
        };
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn edit_rejects_whitespace_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "   \t ".to_string(),
        };
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn edit_allows_normal_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "hello".to_string(),
        };
        assert!(validate_edit(&args).is_ok());
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
        let dir = upload_fixture("caption", &["a.pdf"]);
        let mut args = send_args("plain");
        args.text = None;
        args.file = Some(dir.join("a.pdf").to_string_lossy().into_owned());
        args.caption = Some("cap".to_string());
        assert!(validate_send(&args).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn markdown_accepts_url_containing_tg_user_substring() {
        assert!(validate_markdown("[x](https://example.com/tg://user?id=abc)").is_ok());
    }

    #[test]
    fn markdown_accepts_https_tg_substring() {
        assert!(validate_markdown("https://tg://user?id=abc").is_ok());
    }

    #[test]
    fn markdown_rejects_genuine_invalid_mention() {
        for bad in [
            "[x](tg://user?id=abc)",
            "<tg://user?id=abc>",
            "plain text with tg://user?id=abc",
            "[x](tg://user?id=12abc)",
        ] {
            assert!(
                matches!(validate_markdown(bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_genuine_valid_mention() {
        for good in ["[x](tg://user?id=12345678)", "<tg://user?id=999>"] {
            assert!(validate_markdown(good).is_ok(), "{good}");
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
    fn forward_batch_indexing_does_not_panic_on_short_chunk() {
        let chunk: [i32; 3] = [1, 2, 3];
        let mut forwarded: Vec<serde_json::Value> = Vec::new();
        let mut dropped: Vec<i32> = Vec::new();
        let results = vec![None; 5];
        push_forward_results(&mut forwarded, &mut dropped, &chunk, results).unwrap();
        assert_eq!(dropped, vec![1, 2, 3]);
        assert!(forwarded.is_empty());
    }

    #[test]
    fn delete_requires_ids_unless_all() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: Vec::new(),
            all: false,
            self_only: false,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
        let mut with_all = args;
        with_all.all = true;
        assert!(validate_delete(&with_all).is_ok());
        let with_ids = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1, 2],
            all: false,
            self_only: false,
        };
        assert!(validate_delete(&with_ids).is_ok());
    }

    #[test]
    fn delete_rejects_all_with_ids() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1, 2],
            all: true,
            self_only: false,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn delete_report_flags_partial_deletions() {
        let (report, partial) = delete_report(3, 3);
        assert_eq!(report["requested"], serde_json::json!(3));
        assert_eq!(report["deleted"], serde_json::json!(3));
        assert!(!partial);
        assert!(report.get("partial").is_none());
        let (report, partial) = delete_report(3, 1);
        assert_eq!(report["requested"], serde_json::json!(3));
        assert_eq!(report["deleted"], serde_json::json!(1));
        assert!(partial);
        assert_eq!(report["partial"], serde_json::json!(true));
        let (report, partial) = delete_report(3, 0);
        assert_eq!(report["deleted"], serde_json::json!(0));
        assert!(partial);
        assert_eq!(report["partial"], serde_json::json!(true));
        let (_, partial) = delete_report(0, 0);
        assert!(!partial);
    }

    #[test]
    fn validate_delete_rejects_self_only_with_all() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: Vec::new(),
            all: true,
            self_only: true,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
        let with_ids = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1],
            all: false,
            self_only: true,
        };
        assert!(validate_delete(&with_ids).is_ok());
    }

    #[test]
    fn self_only_supported_kinds() {
        assert!(self_only_supported(PeerKind::User));
        assert!(self_only_supported(PeerKind::Chat));
        assert!(!self_only_supported(PeerKind::Channel));
    }

    #[test]
    fn forward_report_tracks_dropped_and_failed() {
        let forwarded = vec![serde_json::json!({"id": 1})];
        let (report, warn) = forward_report(4, &forwarded, &[2], &[3, 4]);
        assert_eq!(report["requested"], serde_json::json!(4));
        assert_eq!(report["forwarded"], serde_json::json!([{"id": 1}]));
        assert_eq!(report["dropped"]["count"], serde_json::json!(1));
        assert_eq!(report["dropped"]["ids"], serde_json::json!([2]));
        assert_eq!(report["failed"]["count"], serde_json::json!(2));
        assert_eq!(report["failed"]["ids"], serde_json::json!([3, 4]));
        assert!(!warn);
    }

    #[test]
    fn forward_report_warns_when_nothing_confirmed() {
        let (report, warn) = forward_report(2, &[], &[1, 2], &[]);
        assert!(warn);
        assert!(report.get("dropped").is_some());
        assert!(report.get("failed").is_none());
        assert_eq!(report["partial"], serde_json::json!(true));
        let (report, warn) = forward_report(1, &[serde_json::json!({"id": 9})], &[], &[]);
        assert!(!warn);
        assert!(report.get("dropped").is_none());
        assert!(report.get("failed").is_none());
        assert!(report.get("partial").is_none());
    }

    #[test]
    fn forward_report_marks_partial_when_all_chunks_fail() {
        let (value, should_warn) = forward_report(3, &[], &[], &[1, 2, 3]);
        assert_eq!(value["partial"], serde_json::json!(true));
        assert_eq!(value["failed"]["count"], serde_json::json!(3));
        assert!(should_warn);
    }

    #[test]
    fn forward_report_marks_partial_when_some_dropped() {
        let (value, _) = forward_report(2, &[serde_json::json!({"id": 1})], &[2], &[]);
        assert_eq!(value["partial"], serde_json::json!(true));
    }

    #[test]
    fn forward_report_omits_partial_when_all_forwarded() {
        let (value, _) = forward_report(1, &[serde_json::json!({"id": 1})], &[], &[]);
        assert!(value.get("partial").is_none(), "value: {value}");
    }

    #[test]
    fn forward_requires_ids() {
        let args = ForwardArgs {
            from: "a".to_string(),
            ids: Vec::new(),
            to: "b".to_string(),
        };
        assert!(matches!(validate_forward(&args), Err(TeleError::Usage(_))));
        let mut with_ids = args;
        with_ids.ids = vec![3];
        assert!(validate_forward(&with_ids).is_ok());
    }

    #[test]
    fn get_rejects_last_with_offset_id() {
        let args = GetArgs {
            chat: "me".to_string(),
            limit: 10,
            offset_id: Some(5),
            last: true,
        };
        assert!(matches!(validate_get(&args), Err(TeleError::Usage(_))));
        let no_offset = GetArgs {
            chat: "me".to_string(),
            limit: 10,
            offset_id: None,
            last: true,
        };
        assert!(validate_get(&no_offset).is_ok());
        let no_last = GetArgs {
            chat: "me".to_string(),
            limit: 10,
            offset_id: Some(5),
            last: false,
        };
        assert!(validate_get(&no_last).is_ok());
    }

    #[test]
    fn msg_validators_reject_empty_or_whitespace_chat() {
        for chat in ["", "   ", "\t"] {
            let mut send = send_args("plain");
            send.chat = chat.to_string();
            assert!(
                matches!(validate_send(&send), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let edit = EditArgs {
                chat: chat.to_string(),
                id: 1,
                text: "x".to_string(),
            };
            assert!(
                matches!(validate_edit(&edit), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let delete = DeleteArgs {
                chat: chat.to_string(),
                ids: vec![1],
                all: false,
                self_only: false,
            };
            assert!(
                matches!(validate_delete(&delete), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let fwd_from = ForwardArgs {
                from: chat.to_string(),
                ids: vec![1],
                to: "b".to_string(),
            };
            assert!(
                matches!(validate_forward(&fwd_from), Err(TeleError::Usage(_))),
                "{chat:?}"
            );
            let fwd_to = ForwardArgs {
                from: "a".to_string(),
                ids: vec![1],
                to: chat.to_string(),
            };
            assert!(
                matches!(validate_forward(&fwd_to), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let get = GetArgs {
                chat: chat.to_string(),
                limit: 10,
                offset_id: None,
                last: false,
            };
            assert!(
                matches!(validate_get(&get), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let react = ReactArgs {
                chat: chat.to_string(),
                id: 1,
                reaction: Some("+1".to_string()),
                remove: false,
            };
            assert!(
                matches!(validate_react(&react), Err(TeleError::Usage(_))),
                "{chat:?}"
            );
        }
    }

    #[tokio::test]
    async fn pin_read_search_download_reject_empty_chat_before_connect() {
        let err = pin(
            PinArgs {
                chat: String::new(),
                id: 1,
                unpin: false,
            },
            &dryrun_flags("msg pin", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let err = read(
            ReadArgs {
                chat: "   ".to_string(),
                mark_unread: false,
            },
            &dryrun_flags("msg read", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let err = search(
            SearchArgs {
                chat: String::new(),
                query: "x".to_string(),
                limit: 10,
            },
            &dryrun_flags("msg search", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let out =
            std::env::temp_dir().join(format!("telecli-msg-emptychat-{}", std::process::id()));
        let err = download(
            DownloadArgs {
                chat: "\t".to_string(),
                id: 1,
                dir: out.to_string_lossy().into_owned(),
                force: false,
            },
            &dryrun_flags("msg download", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));
    }

    #[test]
    fn react_rejects_reaction_with_remove() {
        let both = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: Some("+1".to_string()),
            remove: true,
        };
        let err = validate_react(&both).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("not both"), "{}", err.message());
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
        let dir = upload_fixture("lookalike", &["session", "env", "x.env"]);
        for good in ["session", "env", "x.env"] {
            assert!(
                validate_upload_path(&dir.join(good).to_string_lossy()).is_ok(),
                "{good}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
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
        let dir = upload_fixture(
            "wlookalike",
            &["file.txt", "CONs", "COM10", "report.txt", "com0", "lpt10"],
        );
        for good in ["file.txt", "CONs", "COM10", "report.txt", "com0", "lpt10"] {
            assert!(
                validate_upload_path(&dir.join(good).to_string_lossy()).is_ok(),
                "{good}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn download_dir_resolves_nonexisting_tail_outside_app_data() {
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
        let outside = sibling.join("deep").join("not-yet-created");
        assert!(validate_download_dir(&outside.to_string_lossy()).is_ok());
        let inside = base.join("not-yet-created").join("nested");
        assert!(
            matches!(
                validate_download_dir(&inside.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{inside:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[cfg(windows)]
    #[test]
    fn download_dir_rejects_case_variant_of_app_data() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let lower = base.to_string_lossy().to_lowercase();
        std::env::set_var("TELE_APP_DIR", &lower);
        let candidate = base.join("sessions").join("new");
        assert!(
            matches!(
                validate_download_dir(&candidate.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{candidate:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn upload_rejects_case_variant_app_data_path() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-upload-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let lower = base.to_string_lossy().to_lowercase();
        std::env::set_var("TELE_APP_DIR", &lower);
        let candidate = base.join("secret.txt");
        assert!(
            matches!(
                validate_upload_path(&candidate.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{candidate:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn upload_rejects_nonexistent_file() {
        let dir = upload_fixture("missing", &[]);
        let missing = dir.join("definitely-missing.txt");
        assert!(matches!(
            validate_upload_path(&missing.to_string_lossy()),
            Err(TeleError::Usage(msg)) if msg.contains("not found")
        ));
        let _ = std::fs::remove_dir_all(&dir);
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
                force: false,
            },
            &dryrun_flags("msg download", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[test]
    fn download_force_flag_allows_overwrite() {
        let dir = temp_path("dl-force");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("existing.txt");
        std::fs::write(&target, b"old").unwrap();
        assert!(refuse_existing_download_target(&target).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_dry_run_carries_argument_keys() {
        let mut args = send_args("plain");
        args.chat = "@x".to_string();
        let value = send_dry_run_payload(&args, None);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["chat"], serde_json::json!("@x"));
        assert_eq!(value["text"], serde_json::json!("hi"));
        assert_eq!(value["file"], serde_json::Value::Null);
        assert_eq!(value["caption"], serde_json::Value::Null);
        assert_eq!(value["format"], serde_json::json!("plain"));
        assert_eq!(value["schedule"], serde_json::Value::Null);
        assert_eq!(value["reply"], serde_json::Value::Null);
        assert_eq!(value["preview"], serde_json::json!(true));
        assert_eq!(value["silent"], serde_json::json!(false));
        assert_eq!(value["would"], serde_json::json!("send message to chat @x"));
    }

    #[test]
    fn send_rejects_no_preview_with_file() {
        let dir = upload_fixture("nopreview", &["a.pdf"]);
        let mut args = send_args("plain");
        args.text = None;
        args.file = Some(dir.join("a.pdf").to_string_lossy().into_owned());
        args.no_preview = true;
        let err = validate_send(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--no-preview"), "{}", err.message());
        args.no_preview = false;
        assert!(validate_send(&args).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_dry_run_payload_carries_effective_preview() {
        let args = send_args("plain");
        assert_eq!(
            send_dry_run_payload(&args, None)["preview"],
            serde_json::json!(true)
        );
        let mut disabled = send_args("plain");
        disabled.no_preview = true;
        assert_eq!(
            send_dry_run_payload(&disabled, None)["preview"],
            serde_json::json!(false)
        );
        let mut both = send_args("plain");
        both.preview = false;
        both.no_preview = true;
        assert_eq!(
            send_dry_run_payload(&both, None)["preview"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn edit_dry_run_carries_argument_keys() {
        let args = EditArgs {
            chat: "@x".to_string(),
            id: 5,
            text: "new text".to_string(),
        };
        let value = edit_dry_run_payload(&args.chat, &args.text, args.id);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["id"], serde_json::json!(5));
        assert_eq!(value["chat"], serde_json::json!("@x"));
        assert_eq!(value["text"], serde_json::json!("new text"));
        assert_eq!(value["would"], serde_json::json!("edit message 5"));
    }
}
