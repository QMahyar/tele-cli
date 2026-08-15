use std::io::Write;

use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::{creds, creds_api_id};
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
                return Ok(serde_json::json!({"dry_run": true, "takeout": true}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
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
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            run_export(&guard, &dir, limit)
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
) -> TeleResult<serde_json::Value> {
    crate::fs_util::create_dir_private(dir)?;

    let mut contacts = Vec::new();
    let raw: tl::enums::contacts::Contacts = guard
        .client
        .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
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
    std::fs::write(
        dir.join("contacts.json"),
        serde_json::to_string_pretty(&contacts)?,
    )?;

    let mut dialogs = Vec::new();
    let mut messages_file =
        std::io::BufWriter::new(std::fs::File::create(dir.join("messages.jsonl"))?);
    let mut dialog_iter = guard.client.iter_dialogs();
    while let Some(dialog) = dialog_iter.next().await.map_err(tele_invocation)? {
        let chat_name = crate::serialize::peer_name(&dialog.peer);
        let unread = match &dialog.raw {
            tl::enums::Dialog::Dialog(d) => d.unread_count,
            tl::enums::Dialog::Folder(_) => 0,
        };
        dialogs.push(serde_json::json!({
            "chat": chat_name,
            "unread": unread,
        }));
        let chat_ref = crate::entities::peer_ref(&dialog.peer)
            .await
            .map_err(tele_invocation)?;
        let mut msg_iter = guard.client.iter_messages(chat_ref);
        let mut count = 0u32;
        while count < limit {
            match msg_iter.next().await.map_err(tele_invocation)? {
                Some(msg) => {
                    let row = crate::serialize::message_to_json(&msg)?;
                    writeln!(messages_file, "{}", serde_json::to_string(&row)?)?;
                    count += 1;
                }
                None => break,
            }
        }
    }
    messages_file.flush()?;
    std::fs::write(
        dir.join("dialogs.json"),
        serde_json::to_string_pretty(&dialogs)?,
    )?;
    Ok(serde_json::json!({
        "dir": dir.to_string_lossy(),
        "contacts": contacts.len(),
        "dialogs": dialogs.len(),
    }))
}

async fn finish(flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "finished": true}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let result = guard
                .client
                .invoke(&tl::functions::account::FinishTakeoutSession { success: true })
                .await;
            let data = match result {
                Ok(success) => serde_json::json!({"finished": success}),
                Err(grammers_client::InvocationError::Rpc(e)) if e.name == "TAKEOUT_REQUIRED" => {
                    serde_json::json!({
                        "finished": false,
                        "reason": "no active takeout session (run takeout start first)"
                    })
                }
                Err(e) => return Err(tele_invocation(e)),
            };
            Ok(data)
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
}
