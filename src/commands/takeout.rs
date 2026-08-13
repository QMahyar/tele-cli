use std::io::Write;

use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
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
    #[arg(long)]
    contacts: bool,
    #[arg(long)]
    messages: bool,
    #[arg(long)]
    photos: bool,
}

#[derive(Args)]
pub struct ExportArgs {
    #[arg(long, default_value_t = 1000)]
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
            let dir = crate::config::app_data_dir().join("export").join(&name);
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

async fn export(args: ExportArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_export(&args)?;
    crate::commands::validate_limit(args.message_limit, 1_000_000, "message-limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let limit = args.message_limit;
        Box::pin(async move {
            let dir = crate::config::app_data_dir().join("export").join(&name);
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
            crate::fs_util::create_dir_private(&dir)?;

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
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
            let success: bool = guard
                .client
                .invoke(&tl::functions::account::FinishTakeoutSession { success: true })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"finished": success}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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

    #[test]
    fn export_rejects_zero_message_limit() {
        let args = ExportArgs { message_limit: 0 };
        assert!(matches!(validate_export(&args), Err(TeleError::Usage(_))));
        let one = ExportArgs { message_limit: 1 };
        assert!(validate_export(&one).is_ok());
    }
}
