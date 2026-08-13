use clap::Args;
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};

#[derive(Args)]
pub struct RawArgs {
    name: String,
    #[arg(long, default_value = "{}")]
    args: String,
}

pub const REGISTERED: &[&str] = &[
    "account.UpdateProfile",
    "contacts.Search",
    "messages.ExportChatInvite",
    "messages.GetAllDrafts",
    "stats.GetBroadcastStats",
    "stats.GetMegagroupStats",
];

pub async fn run(args: &RawArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let params: serde_json::Value = serde_json::from_str(&args.args)
        .map_err(|e| TeleError::Usage(format!("invalid --args JSON: {e}")))?;
    let name = args.name.clone();
    if !REGISTERED.contains(&name.as_str()) {
        return Err(TeleError::Usage(format!(
            "raw method not in registry; add an arm in src/commands/raw.rs (registered: {REGISTERED:?})"
        )));
    }
    let envelope = run_fanout(flags, move |account| {
        let config_path = config_path.clone();
        let name = name.clone();
        let params = params.clone();
        Box::pin(async move {
            let guard =
                ClientGuard::connect(&account, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            dispatch(&guard.client, &name, &params)
                .await
                .map_err(tele_invocation)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn dispatch(
    client: &grammers_client::Client,
    name: &str,
    p: &serde_json::Value,
) -> Result<serde_json::Value, grammers_client::InvocationError> {
    match name {
        "messages.ExportChatInvite" => {
            let chat = crate::entities::resolve_peer(client, &str_field(p, "chat")?).await?;
            let peer = crate::entities::input_peer(&chat).await?;
            let r: tl::enums::ExportedChatInvite = client
                .invoke(&tl::functions::messages::ExportChatInvite {
                    legacy_revoke_permanent: false,
                    request_needed: bool_field(p, "request_needed")?,
                    peer,
                    expire_date: opt_int_field(p, "expire_date")?,
                    usage_limit: opt_int_field(p, "usage_limit")?,
                    title: opt_str_field(p, "title")?,
                    subscription_pricing: None,
                })
                .await?;
            match r {
                tl::enums::ExportedChatInvite::ChatInviteExported(invite) => {
                    Ok(serde_json::json!({
                        "link": invite.link,
                        "usage_limit": invite.usage_limit,
                        "expire_date": invite.expire_date,
                    }))
                }
                _ => Ok(serde_json::json!({"result": "public_join_requests"})),
            }
        }
        "contacts.Search" => {
            let r: tl::enums::contacts::Found = client
                .invoke(&tl::functions::contacts::Search {
                    broadcasts: bool_field(p, "broadcasts")?,
                    bots: bool_field(p, "bots")?,
                    q: str_field(p, "q")?,
                    limit: int_field(p, "limit")?,
                })
                .await?;
            Ok(serde_json::json!({"result": format!("{r:?}")}))
        }
        "messages.GetAllDrafts" => {
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::GetAllDrafts {})
                .await?;
            Ok(serde_json::json!({"result": format!("{r:?}")}))
        }
        "stats.GetBroadcastStats" => {
            let chat = crate::entities::resolve_peer(client, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat).await?;
            let r: tl::enums::stats::BroadcastStats = client
                .invoke(&tl::functions::stats::GetBroadcastStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await?;
            Ok(serde_json::json!({"result": format!("{r:?}")}))
        }
        "stats.GetMegagroupStats" => {
            let chat = crate::entities::resolve_peer(client, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat).await?;
            let r: tl::enums::stats::MegagroupStats = client
                .invoke(&tl::functions::stats::GetMegagroupStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await?;
            Ok(serde_json::json!({"result": format!("{r:?}")}))
        }
        "account.UpdateProfile" => {
            let r: tl::enums::User = client
                .invoke(&tl::functions::account::UpdateProfile {
                    first_name: opt_str_field(p, "first_name")?,
                    last_name: opt_str_field(p, "last_name")?,
                    about: opt_str_field(p, "about")?,
                })
                .await?;
            Ok(serde_json::json!({"result": format!("{r:?}")}))
        }
        _ => Err(grammers_client::InvocationError::Rpc(
            grammers_client::sender::RpcError {
                code: 400,
                name: "RAW_NOT_REGISTERED".to_string(),
                value: None,
                caused_by: None,
            },
        )),
    }
}

fn str_field(p: &serde_json::Value, key: &str) -> Result<String, grammers_client::InvocationError> {
    Ok(p.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

fn opt_str_field(
    p: &serde_json::Value,
    key: &str,
) -> Result<Option<String>, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn int_field(p: &serde_json::Value, key: &str) -> Result<i32, grammers_client::InvocationError> {
    Ok(p.get(key)
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(10))
}

fn opt_int_field(
    p: &serde_json::Value,
    key: &str,
) -> Result<Option<i32>, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_i64()).map(|v| v as i32))
}

fn bool_field(p: &serde_json::Value, key: &str) -> Result<bool, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_bool()).unwrap_or(false))
}

fn creds() -> crate::TeleResult<crate::config::Credentials> {
    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))
}

fn creds_api_id() -> crate::TeleResult<i32> {
    Ok(creds()?.api_id)
}
