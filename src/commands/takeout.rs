use std::collections::HashMap;
use std::io::Write;

use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use grammers_client::media::Media;
use grammers_client::peer::{Peer, User};
use grammers_client::tl;
use grammers_session::types::{PeerId, PeerKind};

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum TakeoutCmd {
    Start(StartArgs),
    Export(ExportArgs),
    Finish(FinishArgs),
}

#[derive(Args)]
pub struct StartArgs {
    #[arg(long, help = "include contacts in export")]
    contacts: bool,
    #[arg(long, help = "include messages in export")]
    messages: bool,
    #[arg(long, help = "include photos in export")]
    photos: bool,
}

#[derive(Args)]
pub struct ExportArgs {
    #[arg(
        long,
        default_value_t = 1000,
        help = "max messages per dialog to export"
    )]
    message_limit: u32,
}

#[derive(Args)]
pub struct FinishArgs {
    #[arg(
        long,
        help = "end the session as abandoned (success:false) instead of completed"
    )]
    abandon: bool,
}

pub async fn run(cmd: TakeoutCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        TakeoutCmd::Start(a) => start(a, flags).await,
        TakeoutCmd::Export(a) => export(a, flags).await,
        TakeoutCmd::Finish(a) => finish(a, flags).await,
    }
}

fn start_dry_run_payload(contacts: bool, messages: bool, photos: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "takeout": true,
        "contacts": contacts,
        "messages": messages,
        "photos": photos,
        "would": format!(
            "start takeout session (contacts: {contacts}, messages: {messages}, photos: {photos})"
        ),
    })
}

async fn start(args: StartArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_explicit_selection("takeout start", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let contacts = args.contacts;
    let messages = args.messages;
    let photos = args.photos;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();

        Box::pin(async move {
            if dry_run {
                return Ok(start_dry_run_payload(contacts, messages, photos));
            }
            let dir = export_dir(&name);
            ensure_no_active_takeout(&dir)?;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let info: tl::enums::account::Takeout = guard
                .client
                .invoke(&tl::functions::account::InitTakeoutSession {
                    contacts,
                    message_users: messages,
                    message_chats: messages,
                    message_megagroups: messages,
                    message_channels: messages,
                    files: photos,
                    file_max_size: Some(5_242_880_000),
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::account::Takeout::Takeout(info) = info;
            crate::fs_util::create_dir_private(&dir)?;
            write_takeout_state(
                &dir,
                &TakeoutStateFile {
                    takeout_id: info.id,
                    checkpoints: HashMap::new(),
                },
            )?;
            Ok(serde_json::json!({
                "takeout_id": info.id,
                "dir": dir.to_string_lossy(),
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_export(args: &ExportArgs) -> TeleResult<()> {
    if args.message_limit == 0 {
        return Err(TeleError::Usage("--message-limit must be >= 1".to_string()));
    }
    Ok(())
}

fn export_dir(name: &str) -> std::path::PathBuf {
    crate::config::app_data_dir().join("export").join(name)
}

const TAKEOUT_STATE_FILE: &str = "takeout.json";

const CHECKPOINT_DONE: i64 = -1;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TakeoutStateFile {
    takeout_id: i64,
    #[serde(default)]
    checkpoints: HashMap<String, i64>,
}

fn takeout_state_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(TAKEOUT_STATE_FILE)
}

fn write_takeout_state(dir: &std::path::Path, state: &TakeoutStateFile) -> TeleResult<()> {
    crate::fs_util::create_dir_private(dir)?;
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(takeout_state_path(dir), json)?;
    Ok(())
}

fn read_takeout_state(dir: &std::path::Path) -> TeleResult<TakeoutStateFile> {
    let path = takeout_state_path(dir);
    if !path.exists() {
        return Err(TeleError::Other(
            "no takeout session started (run takeout start first)".to_string(),
        ));
    }
    let json = std::fs::read_to_string(&path)?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    let takeout_id = value
        .get("takeout_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            TeleError::Other(format!(
                "invalid takeout state at {} (run takeout start to renew)",
                path.to_string_lossy()
            ))
        })?;
    let checkpoints = match value.get("checkpoints") {
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n)))
            .collect(),
        _ => HashMap::new(),
    };
    Ok(TakeoutStateFile {
        takeout_id,
        checkpoints,
    })
}

fn ensure_no_active_takeout(dir: &std::path::Path) -> TeleResult<()> {
    if read_takeout_state(dir).is_ok() {
        return Err(TeleError::Other(
            "active takeout exists; finish it first".to_string(),
        ));
    }
    Ok(())
}

fn delete_takeout_state(dir: &std::path::Path) {
    let _ = std::fs::remove_file(takeout_state_path(dir));
}

fn export_state(dir: &std::path::Path) -> String {
    let contacts = if dir.join("contacts.json").exists() {
        "written"
    } else {
        "missing"
    };
    let messages = if dir.join("dialogs.json").exists() {
        "written"
    } else if dir.join("messages.jsonl").exists() {
        "partial"
    } else {
        "missing"
    };
    let dialogs = if dir.join("dialogs.json").exists() {
        "written"
    } else {
        "missing"
    };
    format!("contacts.json: {contacts}, messages.jsonl: {messages}, dialogs.json: {dialogs}")
}

fn export_error_message(dir: &std::path::Path, cause: &str) -> String {
    format!(
        "takeout export failed; export dir {}: {}; server-side takeout session kept alive for resume: re-run `tele takeout export` to resume automatically from the saved per-dialog checkpoints (completed dialogs are skipped, messages.jsonl is appended, not truncated), or `tele takeout start` if the session expired, then `tele takeout finish`; cause: {cause}",
        dir.to_string_lossy(),
        export_state(dir),
    )
}

#[derive(Debug)]
enum ResumeCursor {
    Fresh,
    From(i32),
    Skip,
}

fn resume_cursor(checkpoints: &HashMap<String, i64>, dialog_key: &str) -> ResumeCursor {
    match checkpoints.get(dialog_key) {
        Some(min_id) if *min_id == CHECKPOINT_DONE => ResumeCursor::Skip,
        Some(min_id) if *min_id > 0 && *min_id <= i32::MAX as i64 => {
            ResumeCursor::From(*min_id as i32)
        }
        _ => ResumeCursor::Fresh,
    }
}

fn persist_checkpoints(
    dir: &std::path::Path,
    takeout_id: i64,
    checkpoints: &HashMap<String, i64>,
) -> TeleResult<()> {
    write_takeout_state(
        dir,
        &TakeoutStateFile {
            takeout_id,
            checkpoints: checkpoints.clone(),
        },
    )
}

fn progress_enabled(machine_mode: bool) -> bool {
    !machine_mode
}

fn report_progress(human: bool, message: &str) {
    if human {
        crate::output::log_line("info", message);
    }
}

fn dialog_page_message(dialog_index: usize, total: Option<i32>, name: &str, msgs: u32) -> String {
    match total {
        Some(n) => format!("dialog {dialog_index}/{n} {name} msgs={msgs}"),
        None => format!("dialog {dialog_index} {name} msgs={msgs}"),
    }
}

fn dialog_skipped_message(dialog_index: usize, total: Option<i32>, name: &str) -> String {
    match total {
        Some(n) => format!("dialog {dialog_index}/{n} {name} complete (skipped)"),
        None => format!("dialog {dialog_index} {name} complete (skipped)"),
    }
}

async fn export(args: ExportArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_explicit_selection("takeout export", flags)?;
    validate_export(&args)?;
    crate::commands::validate_limit(args.message_limit, 1_000_000, "message-limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let human = progress_enabled(output::machine_mode(flags.json, flags.jsonl));
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let limit = args.message_limit;
        Box::pin(async move {
            let dir = export_dir(&name);
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "dir": dir.to_string_lossy(),
                    "message_limit": limit,
                    "would": format!("export takeout data to {}", dir.to_string_lossy()),
                }));
            }
            let state_file = read_takeout_state(&dir)?;
            let takeout_id = state_file.takeout_id;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            run_export(&guard, &dir, limit, takeout_id, human)
                .await
                .map_err(|e| TeleError::Other(export_error_message(&dir, &e.to_string())))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn run_export(
    guard: &ClientGuard,
    dir: &std::path::Path,
    limit: u32,
    takeout_id: i64,
    human_progress: bool,
) -> TeleResult<serde_json::Value> {
    crate::fs_util::create_dir_private(dir)?;
    let mut checkpoints: HashMap<String, i64> = read_takeout_state(dir).map(|s| s.checkpoints)?;

    let mut contacts = Vec::new();
    let raw: tl::enums::contacts::Contacts = guard
        .client
        .invoke(&tl::functions::InvokeWithTakeout {
            takeout_id,
            query: tl::functions::contacts::GetContacts { hash: 0 },
        })
        .await
        .map_err(tele_invocation)?;
    if let tl::enums::contacts::Contacts::Contacts(c) = raw {
        for user in c.users.iter().filter_map(|u| match u {
            tl::enums::User::User(u) => Some(u),
            _ => None,
        }) {
            contacts.push(serde_json::json!({
                "id": user.id,
                "name": format!(
                    "{} {}",
                    user.first_name.clone().unwrap_or_default(),
                    user.last_name.clone().unwrap_or_default()
                ).trim().to_string(),
                "phone": user.phone.as_deref().unwrap_or_default(),
            }));
        }
    }
    let contacts_path = dir.join("contacts.json");
    let contacts_json = serde_json::to_string_pretty(&contacts)?;
    tokio::task::spawn_blocking(move || std::fs::write(&contacts_path, &contacts_json))
        .await
        .map_err(|e| TeleError::Other(e.to_string()))??;

    let mut dialogs = Vec::new();
    let messages_path = dir.join("messages.jsonl");
    let mut messages_file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&messages_path)
            .map(std::io::BufWriter::new)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    let mut exclude_pinned = false;
    let mut offset_date = 0i32;
    let mut offset_id = 0i32;
    let mut offset_peer = tl::enums::InputPeer::Empty;
    let mut page_no = 0usize;
    let mut dialog_index = 0usize;
    loop {
        let res: tl::enums::messages::Dialogs = guard
            .client
            .invoke(&tl::functions::InvokeWithTakeout {
                takeout_id,
                query: tl::functions::messages::GetDialogs {
                    exclude_pinned,
                    folder_id: None,
                    offset_date,
                    offset_id,
                    offset_peer,
                    limit: PAGE_LIMIT,
                    hash: 0,
                },
            })
            .await
            .map_err(tele_invocation)?;
        let (page_dialogs, page_messages, page_users, page_chats, last_chunk, total_dialogs) =
            match res {
                tl::enums::messages::Dialogs::Dialogs(d) => {
                    (d.dialogs, d.messages, d.users, d.chats, true, None)
                }
                tl::enums::messages::Dialogs::Slice(d) => {
                    let last_chunk = d.dialogs.len() < PAGE_LIMIT as usize;
                    (
                        d.dialogs,
                        d.messages,
                        d.users,
                        d.chats,
                        last_chunk,
                        Some(d.count),
                    )
                }
                tl::enums::messages::Dialogs::NotModified(_) => break,
            };
        let peers = build_peers(&guard.client, page_users, page_chats);
        let entries = page_dialogs
            .into_iter()
            .map(|dlg| {
                let peer_id = PeerId::from(dlg.peer());
                let peer = peers
                    .get(&peer_id)
                    .ok_or_else(|| {
                        TeleError::Other(format!(
                            "dialog references peer not in response: {peer_id}"
                        ))
                    })
                    .cloned()?;
                let last_message = page_messages
                    .iter()
                    .find(|m| raw_peer_from_message(m).map(PeerId::from).as_ref() == Some(&peer_id))
                    .cloned();
                Ok((dlg, peer, last_message))
            })
            .collect::<TeleResult<Vec<_>>>()?;
        page_no += 1;
        report_progress(
            human_progress,
            &format!("dialogs page {page_no}: +{} dialogs", entries.len()),
        );
        for (dlg, peer, _) in &entries {
            dialog_index += 1;
            let chat_name = crate::serialize::peer_name(peer);
            let unread = match dlg {
                tl::enums::Dialog::Dialog(d) => d.unread_count,
                tl::enums::Dialog::Folder(_) => 0,
            };
            dialogs.push(serde_json::json!({
                "chat": chat_name,
                "unread": unread,
            }));
            let chat_ref = crate::entities::peer_ref(peer)
                .await
                .map_err(tele_invocation)?;
            let chat_id = chat_ref.id;
            let dialog_key = chat_id.to_string();
            let mut count = 0u32;
            let mut msg_offset_date = 0i32;
            let mut msg_offset_id = 0i32;
            match resume_cursor(&checkpoints, &dialog_key) {
                ResumeCursor::Skip => {
                    report_progress(
                        human_progress,
                        &dialog_skipped_message(dialog_index, total_dialogs, &chat_name),
                    );
                    continue;
                }
                ResumeCursor::From(min_id) => {
                    msg_offset_id = min_id;
                    report_progress(
                        human_progress,
                        &format!("dialog {dialog_index} {chat_name} resuming from min_id={min_id}"),
                    );
                }
                ResumeCursor::Fresh => {}
            }
            loop {
                if count >= limit {
                    break;
                }
                let mres: tl::enums::messages::Messages = guard
                    .client
                    .invoke(&tl::functions::InvokeWithTakeout {
                        takeout_id,
                        query: tl::functions::messages::GetHistory {
                            peer: chat_ref.into(),
                            offset_id: msg_offset_id,
                            offset_date: msg_offset_date,
                            add_offset: 0,
                            limit: PAGE_LIMIT,
                            max_id: 0,
                            min_id: 0,
                            hash: 0,
                        },
                    })
                    .await
                    .map_err(tele_invocation)?;
                let (msgs, m_users, m_chats, m_last_chunk) = match mres {
                    tl::enums::messages::Messages::Messages(m) => {
                        (m.messages, m.users, m.chats, true)
                    }
                    tl::enums::messages::Messages::Slice(m) => {
                        let last_chunk = m.messages.len() < PAGE_LIMIT as usize;
                        (m.messages, m.users, m.chats, last_chunk)
                    }
                    tl::enums::messages::Messages::ChannelMessages(m) => {
                        let last_chunk = m.messages.len() < PAGE_LIMIT as usize;
                        (m.messages, m.users, m.chats, last_chunk)
                    }
                    tl::enums::messages::Messages::NotModified(_) => break,
                };
                if msgs.is_empty() {
                    checkpoints.insert(dialog_key.clone(), CHECKPOINT_DONE);
                    persist_checkpoints(dir, takeout_id, &checkpoints)?;
                    break;
                }
                let m_peers = build_peers(&guard.client, m_users, m_chats);
                let mut lines = Vec::new();
                for raw in &msgs {
                    if count >= limit {
                        break;
                    }
                    let row = raw_message_to_json(raw, &m_peers, Some(chat_id))?;
                    lines.push(serde_json::to_string(&row)?);
                    count += 1;
                }
                if !lines.is_empty() {
                    let written = lines.len();
                    messages_file = tokio::task::spawn_blocking(move || {
                        let mut file = messages_file;
                        for line in &lines {
                            writeln!(file, "{line}")?;
                        }
                        file.flush()?;
                        file.get_ref().sync_all()?;
                        Ok::<_, std::io::Error>(file)
                    })
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))??;
                    let oldest_written =
                        msgs.iter().take(written).map(|m| m.id()).min().unwrap_or(0);
                    checkpoints.insert(dialog_key.clone(), i64::from(oldest_written));
                    persist_checkpoints(dir, takeout_id, &checkpoints)?;
                }
                report_progress(
                    human_progress,
                    &dialog_page_message(dialog_index, total_dialogs, &chat_name, count),
                );
                if m_last_chunk || count >= limit {
                    checkpoints.insert(dialog_key.clone(), CHECKPOINT_DONE);
                    persist_checkpoints(dir, takeout_id, &checkpoints)?;
                    break;
                }
                let last = msgs
                    .last()
                    .ok_or_else(|| TeleError::Other("expected non-empty page".to_string()))?;
                msg_offset_id = last.id();
                msg_offset_date = raw_message_date(last);
            }
        }
        if last_chunk || entries.is_empty() {
            break;
        }
        exclude_pinned = true;
        if let Some((_, _, Some(last_message))) = entries
            .iter()
            .rev()
            .find(|(_, _, last_message)| last_message.is_some())
        {
            offset_date = raw_message_date(last_message);
            offset_id = last_message.id();
        }
        let last_peer_ref = crate::entities::peer_ref(&entries[entries.len() - 1].1)
            .await
            .map_err(tele_invocation)?;
        offset_peer = last_peer_ref.into();
    }
    tokio::task::spawn_blocking(move || messages_file.flush())
        .await
        .map_err(|e| TeleError::Other(e.to_string()))??;
    let dialogs_path = dir.join("dialogs.json");
    let dialogs_json = serde_json::to_string_pretty(&dialogs)?;
    tokio::task::spawn_blocking(move || std::fs::write(&dialogs_path, &dialogs_json))
        .await
        .map_err(|e| TeleError::Other(e.to_string()))??;
    Ok(serde_json::json!({
        "dir": dir.to_string_lossy(),
        "contacts": contacts.len(),
        "dialogs": dialogs.len(),
    }))
}

const PAGE_LIMIT: i32 = 100;

fn build_peers(
    client: &grammers_client::Client,
    users: Vec<tl::enums::User>,
    chats: Vec<tl::enums::Chat>,
) -> HashMap<PeerId, Peer> {
    users
        .into_iter()
        .map(|user| Peer::User(User::from_raw(client, user)))
        .chain(chats.into_iter().map(|chat| Peer::from_raw(client, chat)))
        .map(|peer| (peer.id(), peer))
        .collect()
}

fn raw_peer_from_message(raw: &tl::enums::Message) -> Option<tl::enums::Peer> {
    match raw {
        tl::enums::Message::Empty(e) => e.peer_id.clone(),
        tl::enums::Message::Message(m) => Some(m.peer_id.clone()),
        tl::enums::Message::Service(s) => Some(s.peer_id.clone()),
    }
}

fn raw_message_date(raw: &tl::enums::Message) -> i32 {
    match raw {
        tl::enums::Message::Empty(_) => 0,
        tl::enums::Message::Message(m) => m.date,
        tl::enums::Message::Service(s) => s.date,
    }
}

fn raw_message_out(raw: &tl::enums::Message) -> bool {
    match raw {
        tl::enums::Message::Empty(_) => false,
        tl::enums::Message::Message(m) => m.out,
        tl::enums::Message::Service(s) => s.out,
    }
}

fn raw_message_sender_id(raw: &tl::enums::Message, peer_id: PeerId) -> Option<PeerId> {
    let from_id = match raw {
        tl::enums::Message::Empty(_) => None,
        tl::enums::Message::Message(m) => m.from_id.clone().map(PeerId::from),
        tl::enums::Message::Service(s) => s.from_id.clone().map(PeerId::from),
    };
    from_id.or_else(|| {
        if matches!(peer_id.kind(), PeerKind::User) {
            if raw_message_out(raw) {
                Some(PeerId::self_user())
            } else {
                Some(peer_id)
            }
        } else {
            None
        }
    })
}

fn raw_message_to_json(
    raw: &tl::enums::Message,
    peers: &HashMap<PeerId, Peer>,
    fetched_in: Option<PeerId>,
) -> TeleResult<serde_json::Value> {
    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::json!(raw.id()));
    out.insert(
        "date".into(),
        serde_json::json!(
            DateTime::<Utc>::from_timestamp(i64::from(raw_message_date(raw)), 0)
                .ok_or_else(|| TeleError::Other("message date out of range".to_string()))?
                .to_rfc3339()
        ),
    );
    out.insert("out".into(), serde_json::json!(raw_message_out(raw)));
    let peer_id = raw_peer_from_message(raw)
        .map(PeerId::from)
        .or(fetched_in)
        .ok_or_else(|| TeleError::Other("message has no peer".to_string()))?;
    out.insert(
        "peer".into(),
        match peers.get(&peer_id) {
            Some(peer) => serde_json::json!(crate::serialize::peer_key(peer)),
            None => serde_json::Value::Null,
        },
    );
    out.insert(
        "sender".into(),
        match raw_message_sender_id(raw, peer_id).and_then(|id| peers.get(&id)) {
            Some(sender) => serde_json::json!(crate::serialize::peer_key(sender)),
            None => serde_json::Value::Null,
        },
    );
    out.insert(
        "text".into(),
        serde_json::json!(match raw {
            tl::enums::Message::Empty(_) => "",
            tl::enums::Message::Message(m) => m.message.as_str(),
            tl::enums::Message::Service(_) => "",
        }),
    );
    if let Some(media) = match raw {
        tl::enums::Message::Empty(_) => None,
        tl::enums::Message::Message(m) => m.media.clone().and_then(Media::from_raw),
        tl::enums::Message::Service(_) => None,
    } {
        out.insert(
            "media".into(),
            serde_json::json!(crate::serialize::media_name(&media)),
        );
    }
    Ok(serde_json::Value::Object(out))
}

async fn finish(args: FinishArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_explicit_selection("takeout finish", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let success = !args.abandon;
    let abandon = args.abandon;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                let would = if abandon {
                    "abandon takeout session (success:false)"
                } else {
                    "finish takeout session"
                };
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "finished": true,
                    "abandon": abandon,
                    "would": would
                }));
            }
            let dir = export_dir(&name);
            let takeout_id = read_takeout_state(&dir)?.takeout_id;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let result = guard
                .client
                .invoke(&tl::functions::InvokeWithTakeout {
                    takeout_id,
                    query: tl::functions::account::FinishTakeoutSession { success },
                })
                .await;
            match result {
                Ok(server_success) => {
                    delete_takeout_state(&dir);
                    Ok(serde_json::json!({"finished": server_success}))
                }
                Err(grammers_client::InvocationError::Rpc(e)) if e.name == "TAKEOUT_REQUIRED" => {
                    delete_takeout_state(&dir);
                    Err(TeleError::Other(
                        "no active takeout session (run takeout start first)".to_string(),
                    ))
                }
                Err(e) => Err(tele_invocation(e)),
            }
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telecli-takeout-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn start_dry_run_carries_argument_keys() {
        let value = start_dry_run_payload(true, false, true);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["takeout"], serde_json::json!(true));
        assert_eq!(value["contacts"], serde_json::json!(true));
        assert_eq!(value["messages"], serde_json::json!(false));
        assert_eq!(value["photos"], serde_json::json!(true));
        assert_eq!(
            value["would"],
            serde_json::json!(
                "start takeout session (contacts: true, messages: false, photos: true)"
            )
        );
    }

    #[test]
    fn export_rejects_zero_message_limit() {
        let args = ExportArgs { message_limit: 0 };
        assert!(matches!(validate_export(&args), Err(TeleError::Usage(_))));
        let one = ExportArgs { message_limit: 1 };
        assert!(validate_export(&one).is_ok());
    }

    #[test]
    fn export_dir_lives_under_app_data_export() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let base = temp_dir("dir");
        std::env::set_var("TELE_APP_DIR", &base);
        assert_eq!(export_dir("work"), base.join("export").join("work"));
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn export_state_tracks_each_file() {
        let dir = temp_dir("state");
        assert_eq!(
            export_state(&dir),
            "contacts.json: missing, messages.jsonl: missing, dialogs.json: missing"
        );
        std::fs::write(dir.join("contacts.json"), "[]").unwrap();
        std::fs::write(dir.join("messages.jsonl"), "{}").unwrap();
        assert_eq!(
            export_state(&dir),
            "contacts.json: written, messages.jsonl: partial, dialogs.json: missing"
        );
        std::fs::write(dir.join("dialogs.json"), "[]").unwrap();
        assert_eq!(
            export_state(&dir),
            "contacts.json: written, messages.jsonl: written, dialogs.json: written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_state_reports_contacts_only() {
        let dir = temp_dir("state-contacts-only");
        std::fs::write(dir.join("contacts.json"), "[]").unwrap();
        assert_eq!(
            export_state(&dir),
            "contacts.json: written, messages.jsonl: missing, dialogs.json: missing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takeout_state_round_trips() {
        let dir = temp_dir("state-roundtrip");
        write_takeout_state(
            &dir,
            &TakeoutStateFile {
                takeout_id: 123456,
                checkpoints: HashMap::new(),
            },
        )
        .unwrap();
        let read = read_takeout_state(&dir).unwrap();
        assert_eq!(read.takeout_id, 123456);
        assert!(read.checkpoints.is_empty());
        delete_takeout_state(&dir);
        assert!(matches!(read_takeout_state(&dir), Err(TeleError::Other(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takeout_state_missing_mentions_start() {
        let dir = temp_dir("state-missing");
        let err = read_takeout_state(&dir).unwrap_err();
        assert!(err.message().contains("takeout start first"), "err: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takeout_state_checkpoints_round_trip() {
        let dir = temp_dir("checkpoints-roundtrip");
        let mut checkpoints = HashMap::new();
        checkpoints.insert("100".to_string(), CHECKPOINT_DONE);
        checkpoints.insert("200".to_string(), 4321i64);
        write_takeout_state(
            &dir,
            &TakeoutStateFile {
                takeout_id: 7,
                checkpoints: checkpoints.clone(),
            },
        )
        .unwrap();
        let read = read_takeout_state(&dir).unwrap();
        assert_eq!(read.takeout_id, 7);
        assert_eq!(read.checkpoints, checkpoints);
        assert_eq!(
            read.checkpoints.get("200"),
            Some(&4321),
            "cursor survives restart"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_state_without_checkpoints_reads_with_empty_map() {
        let dir = temp_dir("legacy-state");
        std::fs::write(
            takeout_state_path(&dir),
            serde_json::to_string_pretty(&serde_json::json!({"takeout_id": 42})).unwrap(),
        )
        .unwrap();
        let read = read_takeout_state(&dir).unwrap();
        assert_eq!(read.takeout_id, 42);
        assert!(read.checkpoints.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fixture_checkpoints() -> HashMap<String, i64> {
        HashMap::from([
            ("done".to_string(), CHECKPOINT_DONE),
            ("mid".to_string(), 555i64),
            ("zero".to_string(), 0),
        ])
    }

    #[test]
    fn resume_cursor_skips_completed_dialogs() {
        let cps = fixture_checkpoints();
        assert!(matches!(resume_cursor(&cps, "done"), ResumeCursor::Skip,));
    }

    #[test]
    fn resume_cursor_resumes_from_saved_min_id() {
        let cps = fixture_checkpoints();
        match resume_cursor(&cps, "mid") {
            ResumeCursor::From(min_id) => assert_eq!(min_id, 555),
            other => panic!("expected From cursor, got {other:?}"),
        }
    }

    #[test]
    fn resume_cursor_fresh_for_absent_zero_or_invalid() {
        let mut cps = fixture_checkpoints();
        assert!(matches!(resume_cursor(&cps, "absent"), ResumeCursor::Fresh));
        assert!(matches!(resume_cursor(&cps, "zero"), ResumeCursor::Fresh));
        cps.insert("huge".to_string(), i64::from(i32::MAX) + 1);
        assert!(matches!(resume_cursor(&cps, "huge"), ResumeCursor::Fresh));
        cps.insert("negative".to_string(), -9);
        assert!(matches!(
            resume_cursor(&cps, "negative"),
            ResumeCursor::Fresh
        ));
    }

    #[test]
    fn progress_only_enabled_in_human_mode() {
        assert!(progress_enabled(false));
        assert!(!progress_enabled(true), "machine mode stays silent");
    }

    #[test]
    fn dialog_page_message_matches_contract_style() {
        assert_eq!(
            dialog_page_message(3, Some(57), "Alice", 120),
            "dialog 3/57 Alice msgs=120"
        );
        assert_eq!(
            dialog_page_message(3, None, "Alice", 120),
            "dialog 3 Alice msgs=120"
        );
    }

    #[test]
    fn dialog_skipped_message_reports_completion_without_counts() {
        assert_eq!(
            dialog_skipped_message(2, Some(57), "Bob"),
            "dialog 2/57 Bob complete (skipped)"
        );
    }

    #[test]
    fn persist_checkpoints_writes_readable_state_file() {
        let dir = temp_dir("persist-checkpoints");
        crate::fs_util::create_dir_private(&dir).unwrap();
        let mut cps = HashMap::new();
        cps.insert("-1001234".to_string(), 99);
        persist_checkpoints(&dir, 5, &cps).unwrap();
        let read = read_takeout_state(&dir).unwrap();
        assert_eq!(read.takeout_id, 5);
        assert_eq!(read.checkpoints.get("-1001234"), Some(&99));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invoke_with_takeout_wrapper_serializes_with_constructor_and_fields() {
        use grammers_client::tl::Identifiable;
        use grammers_client::tl::Serializable;
        let req = tl::functions::InvokeWithTakeout {
            takeout_id: 42,
            query: tl::functions::messages::GetHistory {
                peer: tl::enums::InputPeer::Empty,
                offset_id: 7,
                offset_date: 0,
                add_offset: 0,
                limit: 100,
                max_id: 0,
                min_id: 0,
                hash: 0,
            },
        };
        let mut buf = Vec::new();
        req.serialize(&mut buf);
        assert_eq!(
            u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            <tl::functions::InvokeWithTakeout<
                tl::functions::messages::GetHistory,
            > as Identifiable>::CONSTRUCTOR_ID
        );
        assert_eq!(i64::from_le_bytes(buf[4..12].try_into().unwrap()), 42);
        assert_eq!(
            u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            <tl::functions::messages::GetHistory as Identifiable>::CONSTRUCTOR_ID
        );
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            <tl::types::InputPeerEmpty as Identifiable>::CONSTRUCTOR_ID
        );
        assert_eq!(i32::from_le_bytes(buf[20..24].try_into().unwrap()), 7);
        assert_eq!(i32::from_le_bytes(buf[28..32].try_into().unwrap()), 0);
        assert_eq!(i32::from_le_bytes(buf[32..36].try_into().unwrap()), 100);
        assert_eq!(&buf[36..], &[0u8; 16]);
    }

    #[test]
    fn invoke_with_takeout_wrapper_round_trips_with_bare_query() {
        use grammers_client::tl::{Deserializable, Serializable};
        let req = tl::functions::InvokeWithTakeout {
            takeout_id: 42,
            query: tl::types::InputPeerEmpty {},
        };
        let mut buf = Vec::new();
        req.serialize(&mut buf);
        let out =
            tl::functions::InvokeWithTakeout::<tl::types::InputPeerEmpty>::from_bytes(&buf[4..])
                .unwrap();
        assert_eq!(out, req);
    }

    #[test]
    fn invoke_with_takeout_wrapper_serializes_get_contacts_with_constructor_and_fields() {
        use grammers_client::tl::Identifiable;
        use grammers_client::tl::Serializable;
        let req = tl::functions::InvokeWithTakeout {
            takeout_id: 42,
            query: tl::functions::contacts::GetContacts { hash: 0 },
        };
        let mut buf = Vec::new();
        req.serialize(&mut buf);
        assert_eq!(
            u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            <tl::functions::InvokeWithTakeout<
                tl::functions::contacts::GetContacts,
            > as Identifiable>::CONSTRUCTOR_ID
        );
        assert_eq!(i64::from_le_bytes(buf[4..12].try_into().unwrap()), 42);
        assert_eq!(
            u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            <tl::functions::contacts::GetContacts as Identifiable>::CONSTRUCTOR_ID
        );
        assert_eq!(i64::from_le_bytes(buf[16..24].try_into().unwrap()), 0);
    }

    #[test]
    fn invoke_with_takeout_wrapper_round_trips_with_get_contacts_query() {
        use grammers_client::tl::{Deserializable, Serializable};
        let req = tl::functions::InvokeWithTakeout {
            takeout_id: 42,
            query: tl::functions::contacts::GetContacts { hash: 0 },
        };
        let mut buf = Vec::new();
        req.serialize(&mut buf);
        let mut body = Vec::new();
        body.extend_from_slice(&buf[4..12]);
        body.extend_from_slice(&buf[16..24]);
        let out =
            tl::functions::InvokeWithTakeout::<tl::functions::contacts::GetContacts>::from_bytes(
                &body,
            )
            .unwrap();
        assert_eq!(out, req);
    }

    #[test]
    fn export_error_message_names_dir_and_resume_commands() {
        let dir = temp_dir("err-resume");
        std::fs::write(dir.join("contacts.json"), "[]").unwrap();
        std::fs::write(dir.join("messages.jsonl"), "{}").unwrap();
        let msg = export_error_message(&dir, "FLOOD_WAIT");
        assert!(
            msg.contains(&dir.to_string_lossy().to_string()),
            "msg: {msg}"
        );
        assert!(msg.contains("messages.jsonl: partial"), "msg: {msg}");
        assert!(msg.contains("re-run `tele takeout export`"), "msg: {msg}");
        assert!(msg.contains("`tele takeout start`"), "msg: {msg}");
        assert!(msg.contains("`tele takeout finish`"), "msg: {msg}");
        assert!(msg.contains("FLOOD_WAIT"), "msg: {msg}");
        assert!(msg.contains("resume automatically"), "msg: {msg}");
        assert!(msg.contains("per-dialog checkpoints"), "msg: {msg}");
        assert!(msg.contains("appended, not truncated"), "msg: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_error_message_names_state_and_resume_path() {
        let dir = temp_dir("err");
        std::fs::write(dir.join("contacts.json"), "[]").unwrap();
        let msg = export_error_message(&dir, "FLOOD_WAIT");
        assert!(msg.contains("contacts.json: written"), "msg: {msg}");
        assert!(msg.contains("messages.jsonl: missing"), "msg: {msg}");
        assert!(msg.contains("dialogs.json: missing"), "msg: {msg}");
        assert!(msg.contains("kept alive"), "msg: {msg}");
        assert!(msg.contains("takeout export"), "msg: {msg}");
        assert!(msg.contains("FLOOD_WAIT"), "msg: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pagination_uses_message_count_not_id() {
        const HIGH_MESSAGE_ID: i32 = 5_000_000;

        const { assert!(50 < PAGE_LIMIT) };
        const { assert!(!(HIGH_MESSAGE_ID <= PAGE_LIMIT)) };

        const { assert!(!(100 < PAGE_LIMIT)) };
        const { assert!(!(150 < PAGE_LIMIT)) };
    }

    #[test]
    fn active_takeout_guard_blocks_second_start() {
        let dir = temp_dir("guard-active");
        write_takeout_state(
            &dir,
            &TakeoutStateFile {
                takeout_id: 99,
                checkpoints: HashMap::new(),
            },
        )
        .unwrap();
        let err = ensure_no_active_takeout(&dir).unwrap_err();
        assert!(
            err.message().contains("active takeout exists"),
            "err: {err}"
        );
        assert!(err.message().contains("finish it first"), "err: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn active_takeout_guard_allows_fresh_start() {
        let dir = temp_dir("guard-fresh");
        assert!(ensure_no_active_takeout(&dir).is_ok());
        write_takeout_state(
            &dir,
            &TakeoutStateFile {
                takeout_id: 1,
                checkpoints: HashMap::new(),
            },
        )
        .unwrap();
        assert!(ensure_no_active_takeout(&dir).is_err());
        delete_takeout_state(&dir);
        assert!(ensure_no_active_takeout(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
