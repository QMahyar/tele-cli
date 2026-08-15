use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::TeleResult;
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum DialogCmd {
    List(ListArgs),
    Drafts(ListArgs),
    Archive(ArchiveArgs),
    Delete(ChatArgs),
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
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
    #[arg(long, help = "unarchive (restore) instead of archive")]
    unarchive: bool,
}

#[derive(Args)]
pub struct ChatArgs {
    #[arg(long, help = "target chat: @username, t.me link, numeric ID, or me")]
    chat: String,
}

pub async fn run(cmd: DialogCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        DialogCmd::List(a) => list(a, flags).await,
        DialogCmd::Drafts(a) => drafts(a, flags).await,
        DialogCmd::Archive(a) => archive(a, flags).await,
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
    let folder = args.folder;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();

        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_data(limit, folder));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let mut iter = guard.client.iter_dialogs();
            let mut rows = Vec::new();
            let mut count = 0u32;
            while count < limit {
                match iter.next().await.map_err(tele_invocation)? {
                    Some(dialog) => {
                        let (unread, draft, dialog_folder) = match &dialog.raw {
                            tl::enums::Dialog::Dialog(d) => (
                                d.unread_count,
                                match &d.draft {
                                    Some(tl::enums::DraftMessage::Message(dm)) => {
                                        dm.message.clone()
                                    }
                                    _ => String::new(),
                                },
                                d.folder_id,
                            ),
                            tl::enums::Dialog::Folder(_) => (0, String::new(), None),
                        };
                        if let Some(f) = folder {
                            if dialog_folder != Some(f) {
                                continue;
                            }
                        }
                        let last = dialog
                            .last_message
                            .as_ref()
                            .map(|m| m.text().to_string())
                            .unwrap_or_default();
                        rows.push(serde_json::json!({
                            "chat": crate::serialize::peer_key(&dialog.peer),
                            "unread": unread,
                            "draft": draft,
                            "last_message": last,
                        }));
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
                output::print_table(&["chat", "unread", "draft"], &table_rows);
            }
            Ok(serde_json::json!({"dialogs": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn drafts(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit as usize;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(drafts_dry_run_data(limit));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
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
                output::print_table(&["id", "draft"], &table_rows);
            }
            Ok(serde_json::json!({"drafts": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn list_dry_run_data(limit: u32, folder: Option<i32>) -> serde_json::Value {
    serde_json::json!({"dry_run": true, "limit": limit, "folder": folder})
}

fn drafts_dry_run_data(limit: usize) -> serde_json::Value {
    serde_json::json!({"dry_run": true, "limit": limit})
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
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
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

async fn delete(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            guard
                .client
                .delete_dialog(chat_ref)
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"chat": target, "deleted": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
