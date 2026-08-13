use clap::{Args, Subcommand};
use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
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
    #[arg(long)]
    chat: String,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    schedule: Option<String>,
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    caption: Option<String>,
    #[arg(long)]
    reply: Option<i32>,
    #[arg(long, default_value_t = true)]
    preview: bool,
    #[arg(long, default_value = "plain")]
    format: String,
    #[arg(long)]
    silent: bool,
}

#[derive(Args)]
pub struct EditArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    id: i32,
    #[arg(long)]
    text: String,
}

#[derive(Args)]
pub struct DeleteArgs {
    #[arg(long)]
    chat: String,
    #[arg(long, value_delimiter = ',')]
    ids: Vec<i32>,
    #[arg(long)]
    all: bool,
}

#[derive(Args)]
pub struct ForwardArgs {
    #[arg(long)]
    from: String,
    #[arg(long, value_delimiter = ',')]
    ids: Vec<i32>,
    #[arg(long)]
    to: String,
    #[arg(long)]
    silent: bool,
}

#[derive(Args)]
pub struct PinArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    id: i32,
    #[arg(long)]
    unpin: bool,
    #[arg(long)]
    silent: bool,
}

#[derive(Args)]
pub struct GetArgs {
    #[arg(long)]
    chat: String,
    #[arg(long, default_value_t = 10)]
    limit: u32,
    #[arg(long)]
    offset_id: Option<i32>,
    #[arg(long)]
    last: bool,
}

#[derive(Args)]
pub struct ReadArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    mark_unread: bool,
}

#[derive(Args)]
pub struct ReactArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    id: i32,
    #[arg(long)]
    reaction: Option<String>,
    #[arg(long)]
    remove: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: u32,
}

#[derive(Args)]
pub struct DownloadArgs {
    #[arg(long)]
    chat: String,
    #[arg(long)]
    id: i32,
    #[arg(long)]
    out: String,
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
    if args.text.is_none() && args.file.is_none() {
        return Err(TeleError::Usage(
            "msg send requires --text or --file".to_string(),
        ));
    }
    if let Some(path) = &args.file {
        validate_upload_path(path)?;
    }
    Ok(())
}

fn validate_upload_path(path: &str) -> TeleResult<()> {
    let app_dir = crate::config::app_data_dir();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    if canonical.starts_with(&app_dir) {
        return Err(TeleError::Usage(
            "refusing to upload a file from the telecli app data directory".to_string(),
        ));
    }
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == ".env" || base.ends_with(".session") || base.ends_with(".session-journal") {
        return Err(TeleError::Usage(format!(
            "refusing to upload sensitive file {base}"
        )));
    }
    Ok(())
}

async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let schedule = args
        .schedule
        .as_deref()
        .map(crate::commands::parse_unixtime)
        .transpose()?
        .and_then(|s| {
            let ts = s.timestamp();
            u32::try_from(ts)
                .ok()
                .or_else(|| ts.is_negative().then_some(0))
        });
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
            client::authorize(&guard.client, &creds()?).await?;
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
            client::authorize(&guard.client, &creds()?).await?;
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
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let ids = if all {
                let mut iter = guard.client.iter_messages(chat_ref);
                let mut collected = Vec::new();
                while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                    collected.push(msg.id());
                }
                collected
            } else {
                ids
            };
            if ids.is_empty() {
                return Ok(serde_json::json!({"deleted": 0}));
            }
            let count = guard
                .client
                .delete_messages(chat_ref, &ids)
                .await
                .map_err(tele_invocation)?;
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
            client::authorize(&guard.client, &creds()?).await?;
            let from = entities::resolve_peer(&guard.client, guard.session.as_ref(), &from_target)
                .await
                .map_err(tele_invocation)?;
            let to = entities::resolve_peer(&guard.client, guard.session.as_ref(), &to_target)
                .await
                .map_err(tele_invocation)?;
            let from_ref = entities::peer_ref(&from).await.map_err(tele_invocation)?;
            let to_ref = entities::peer_ref(&to).await.map_err(tele_invocation)?;
            let sent = guard
                .client
                .forward_messages(to_ref, &ids, from_ref)
                .await
                .map_err(tele_invocation)?;
            let msgs: Vec<serde_json::Value> = sent
                .iter()
                .filter_map(|m| m.as_ref())
                .map(crate::serialize::message_to_json)
                .collect::<TeleResult<_>>()?;
            Ok(serde_json::json!({"forwarded": msgs}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
            client::authorize(&guard.client, &creds()?).await?;
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
    let limit = args.limit as usize;
    let offset_id = args.offset_id;
    let last = args.last;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        Box::pin(async move {
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
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
                            r["text"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(80)
                                .collect(),
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
            client::authorize(&guard.client, &creds()?).await?;
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
            client::authorize(&guard.client, &creds()?).await?;
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
    let limit = args.limit as usize;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let query = args.query.clone();
        Box::pin(async move {
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
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
                            r["text"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(80)
                                .collect(),
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
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let id = args.id;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = args.chat.clone();
        let out_dir = args.out.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "id": id}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
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
            std::fs::create_dir_all(&out_dir)?;
            let path = std::path::Path::new(&out_dir).join(name);
            let ok = msg.download_media(&path).await.map_err(tele_invocation)?;
            if !ok {
                return Err(TeleError::Usage("message has no media".to_string()));
            }
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Ok(serde_json::json!({"path": path.to_string_lossy(), "bytes": bytes}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn download_name(msg: &grammers_client::message::Message) -> String {
    use grammers_client::media::Media;
    let name = match msg.media() {
        Some(Media::Document(d)) => d
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| "document.bin".to_string()),
        Some(Media::Photo(_)) => "photo.jpg".to_string(),
        Some(Media::Sticker(_)) => "sticker.webp".to_string(),
        Some(_) => "media.bin".to_string(),
        None => "media.bin".to_string(),
    };
    let base = name.rsplit('/').next().unwrap_or(&name);
    let base = base.rsplit('\\').next().unwrap_or(base);
    base.replace(['\0', ':', '*', '?', '"', '<', '>', '|'], "_")
}

fn looks_like_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic"
    )
}

fn creds() -> crate::TeleResult<crate::config::Credentials> {
    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))
}

fn creds_api_id() -> crate::TeleResult<i32> {
    Ok(creds()?.api_id)
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
}

