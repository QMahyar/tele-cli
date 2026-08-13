use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum ContactCmd {
    List(ListArgs),
    Add(AddArgs),
    Block(BlockArgs),
    Unblock(BlockArgs),
}

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, default_value_t = 100)]
    limit: u32,
}

#[derive(Args)]
pub struct AddArgs {
    #[arg(long)]
    user: String,
    #[arg(long)]
    first: Option<String>,
    #[arg(long)]
    last: Option<String>,
    #[arg(long)]
    phone: Option<String>,
}

#[derive(Args)]
pub struct BlockArgs {
    #[arg(long)]
    user: String,
}

pub async fn run(cmd: ContactCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ContactCmd::List(a) => list(a, flags).await,
        ContactCmd::Add(a) => add(a, flags).await,
        ContactCmd::Block(a) => block(a, flags).await,
        ContactCmd::Unblock(a) => unblock(a, flags).await,
    }
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();

        Box::pin(async move {
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let raw: tl::enums::contacts::Contacts = guard
                .client
                .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
                .await
                .map_err(tele_invocation)?;
            let users = match raw {
                tl::enums::contacts::Contacts::NotModified => Vec::new(),
                tl::enums::contacts::Contacts::Contacts(contacts) => contacts.users,
            };
            let mut rows = Vec::new();
            for user in users.into_iter().take(limit as usize) {
                if let tl::enums::User::User(user) = user {
                    rows.push(serde_json::json!({
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
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["name"].as_str().unwrap_or_default().to_string(),
                            r["phone"].as_str().unwrap_or_default().to_string(),
                        ]
                    })
                    .collect();
                output::print_table(&["id", "name", "phone"], &table_rows);
            }
            Ok(serde_json::json!({"contacts": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn add(args: AddArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let user_target = args.user.clone();
        let first = args.first.clone();
        let last = args.last.clone();
        let phone = args.phone.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "user": user_target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let peer = entities::resolve_peer(&guard.client, &user_target)
                .await
                .map_err(tele_invocation)?;
            let user_input = entities::input_user(&peer).await.map_err(tele_invocation)?;
            let name = crate::serialize::peer_name(&peer);
            let (f, l) = match (first, last) {
                (Some(f), Some(l)) => (f, l),
                _ => {
                    let mut parts = name.splitn(2, ' ');
                    (
                        parts.next().unwrap_or(&name).to_string(),
                        parts.next().unwrap_or("").to_string(),
                    )
                }
            };
            let _: tl::enums::Updates = guard
                .client
                .invoke(&tl::functions::contacts::AddContact {
                    add_phone_privacy_exception: false,
                    id: user_input,
                    first_name: f,
                    last_name: l,
                    phone: phone.unwrap_or_default(),
                    note: None,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"user": user_target, "added": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn block(args: BlockArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let user_target = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "user": user_target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let peer = entities::resolve_peer(&guard.client, &user_target)
                .await
                .map_err(tele_invocation)?;
            let input = entities::input_peer(&peer).await.map_err(tele_invocation)?;
            let _: bool = guard
                .client
                .invoke(&tl::functions::contacts::Block {
                    my_stories_from: false,
                    id: input,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"user": user_target, "blocked": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn unblock(args: BlockArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let user_target = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "user": user_target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let peer = entities::resolve_peer(&guard.client, &user_target)
                .await
                .map_err(tele_invocation)?;
            let input = entities::input_peer(&peer).await.map_err(tele_invocation)?;
            let _: bool = guard
                .client
                .invoke(&tl::functions::contacts::Unblock {
                    my_stories_from: false,
                    id: input,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"user": user_target, "blocked": false}))
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
