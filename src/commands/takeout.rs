use std::collections::HashMap;
use std::io::Write;

use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use grammers_client::media::Media;
use grammers_client::peer::{Peer, User};
use grammers_client::tl;
use grammers_session::types::{PeerId, PeerKind};

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};

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
pub struct FinishArgs {}

pub async fn run(cmd: TakeoutCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        TakeoutCmd::Start(a) => start(a, flags).await,
        TakeoutCmd::Export(a) => export(a, flags).await,
        TakeoutCmd::Finish(_) => finish(flags).await,
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
            let dir = export_dir(&name);
            crate::fs_util::create_dir_private(&dir)?;
            write_takeout_state(&dir, info.id)?;
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

fn takeout_state_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(TAKEOUT_STATE_FILE)
}

fn write_takeout_state(dir: &std::path::Path, takeout_id: i64) -> TeleResult<()> {
    crate::fs_util::create_dir_private(dir)?;
    let json = serde_json::to_string_pretty(&serde_json::json!({ "takeout_id": takeout_id }))?;
    std::fs::write(takeout_state_path(dir), json)?;
    Ok(())
}

fn read_takeout_state(dir: &std::path::Path) -> TeleResult<i64> {
    let path = takeout_state_path(dir);
    if !path.exists() {
        return Err(TeleError::Other(
            "no takeout session started (run takeout start first)".to_string(),
        ));
    }
    let json = std::fs::read_to_string(&path)?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    value
        .get("takeout_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            TeleError::Other(format!(
                "invalid takeout state at {} (run takeout start to renew)",
                path.to_string_lossy()
            ))
        })
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
        "takeout export failed; export dir {}: {}; server-side takeout session kept alive for resume: re-run `tele takeout export` (or `tele takeout start` if the session expired), then `tele takeout finish`; cause: {cause}",
        dir.to_string_lossy(),
        export_state(dir),
    )
}

async fn export(args: ExportArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_export(&args)?;
    crate::commands::validate_limit(args.message_limit, 1_000_000, "message-limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
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
            let takeout_id = read_takeout_state(&dir)?;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            run_export(&guard, &dir, limit, takeout_id)
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
) -> TeleResult<serde_json::Value> {
    crate::fs_util::create_dir_private(dir)?;

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
        std::fs::File::create(&messages_path).map(std::io::BufWriter::new)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    let mut exclude_pinned = false;
    let mut offset_date = 0i32;
    let mut offset_id = 0i32;
    let mut offset_peer = tl::enums::InputPeer::Empty;
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
        let (page_dialogs, page_messages, page_users, page_chats, last_chunk) = match res {
            tl::enums::messages::Dialogs::Dialogs(d) => {
                (d.dialogs, d.messages, d.users, d.chats, true)
            }
            tl::enums::messages::Dialogs::Slice(d) => {
                let last_chunk = d.dialogs.len() < PAGE_LIMIT as usize;
                (d.dialogs, d.messages, d.users, d.chats, last_chunk)
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
        for (dlg, peer, _) in &entries {
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
            let mut count = 0u32;
            let mut msg_offset_date = 0i32;
            let mut msg_offset_id = 0i32;
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
                    messages_file = tokio::task::spawn_blocking(move || {
                        let mut file = messages_file;
                        for line in &lines {
                            writeln!(file, "{line}")?;
                        }
                        Ok::<_, std::io::Error>(file)
                    })
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))??;
                }
                if m_last_chunk {
                    break;
                }
                let last = msgs.last().expect("non-empty page");
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
            DateTime::<Utc>::from_timestamp(raw_message_date(raw) as i64, 0)
                .expect("date out of range")
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

async fn finish(flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "finished": true,
                    "would": "finish takeout session"
                }));
            }
            let dir = export_dir(&name);
            let takeout_id = read_takeout_state(&dir)?;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let result = guard
                .client
                .invoke(&tl::functions::InvokeWithTakeout {
                    takeout_id,
                    query: tl::functions::account::FinishTakeoutSession { success: true },
                })
                .await;
            match result {
                Ok(success) => {
                    delete_takeout_state(&dir);
                    Ok(serde_json::json!({"finished": success}))
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
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
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
        write_takeout_state(&dir, 123456).unwrap();
        assert_eq!(read_takeout_state(&dir).unwrap(), 123456);
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
}
