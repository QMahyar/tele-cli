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
    validate_params(&name, &params)?;
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |account| {
        let config_path = config_path.clone();
        let name = name.clone();
        let params = params.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "method": name,
                }));
            }
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

fn validate_params(name: &str, p: &serde_json::Value) -> TeleResult<()> {
    fn req_str(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if p.get(key).and_then(|v| v.as_str()).is_none() {
            return Err(TeleError::Usage(format!(
                "--args field {key} is required (string)"
            )));
        }
        Ok(())
    }
    fn opt_str(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            if !v.is_string() {
                return Err(TeleError::Usage(format!(
                    "--args field {key} must be a string"
                )));
            }
        }
        Ok(())
    }
    fn opt_i32(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            let n = v.as_i64().ok_or_else(|| {
                TeleError::Usage(format!("--args field {key} must be an integer"))
            })?;
            i32::try_from(n)
                .map_err(|_| TeleError::Usage(format!("--args field {key} is out of range")))?;
        }
        Ok(())
    }
    fn opt_bool(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            if !v.is_boolean() {
                return Err(TeleError::Usage(format!(
                    "--args field {key} must be a boolean"
                )));
            }
        }
        Ok(())
    }
    if !p.is_object() {
        return Err(TeleError::Usage(
            "--args must be a JSON object of constructor kwargs".to_string(),
        ));
    }
    match name {
        "contacts.Search" => {
            req_str(p, "q")?;
            opt_i32(p, "limit")?;
            opt_bool(p, "broadcasts")?;
            opt_bool(p, "bots")?;
        }
        "messages.ExportChatInvite" => {
            req_str(p, "chat")?;
            opt_bool(p, "request_needed")?;
            opt_i32(p, "expire_date")?;
            opt_i32(p, "usage_limit")?;
            opt_str(p, "title")?;
        }
        "stats.GetBroadcastStats" | "stats.GetMegagroupStats" => {
            req_str(p, "channel")?;
            opt_bool(p, "dark")?;
        }
        "account.UpdateProfile" => {
            opt_str(p, "first_name")?;
            opt_str(p, "last_name")?;
            opt_str(p, "about")?;
        }
        _ => {}
    }
    Ok(())
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
            let tl::enums::contacts::Found::Found(found) = r;
            let my_results = found.my_results.iter().map(peer_id).collect::<Vec<_>>();
            let results = found.results.iter().map(peer_id).collect::<Vec<_>>();
            let users = found
                .users
                .iter()
                .map(|u| match u {
                    tl::enums::User::User(u) => serde_json::json!({
                        "id": u.id,
                        "first_name": u.first_name,
                        "last_name": u.last_name,
                        "username": u.username,
                    }),
                    _ => serde_json::json!({"id": 0}),
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "my_results": my_results,
                "results": results,
                "users": users,
            }))
        }
        "messages.GetAllDrafts" => {
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::GetAllDrafts {})
                .await?;
            let (updates, users, chats) = match r {
                tl::enums::Updates::Updates(u) => (u.updates, u.users.len(), u.chats.len()),
                other => {
                    return Ok(serde_json::json!({
                        "updates": [],
                        "users": 0,
                        "chats": 0,
                        "kind": format!("{other:?}"),
                    }));
                }
            };
            let rows = updates
                .iter()
                .map(update_summary)
                .collect::<Vec<serde_json::Value>>();
            Ok(serde_json::json!({
                "updates": rows,
                "users": users,
                "chats": chats,
            }))
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
            let tl::enums::stats::BroadcastStats::Stats(r) = r;
            Ok(serde_json::json!({
                "period": stats_period(&r.period),
                "followers": stats_abs(&r.followers),
                "views_per_post": stats_abs(&r.views_per_post),
                "shares_per_post": stats_abs(&r.shares_per_post),
                "reactions_per_post": stats_abs(&r.reactions_per_post),
                "enabled_notifications": stats_percent(&r.enabled_notifications),
                "recent_posts_interactions": r.recent_posts_interactions.len(),
            }))
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
            let tl::enums::stats::MegagroupStats::Stats(r) = r;
            Ok(serde_json::json!({
                "period": stats_period(&r.period),
                "members": stats_abs(&r.members),
                "messages": stats_abs(&r.messages),
                "viewers": stats_abs(&r.viewers),
                "posters": stats_abs(&r.posters),
                "top_posters": r.top_posters.len(),
                "top_admins": r.top_admins.len(),
                "top_inviters": r.top_inviters.len(),
            }))
        }
        "account.UpdateProfile" => {
            let r: tl::enums::User = client
                .invoke(&tl::functions::account::UpdateProfile {
                    first_name: opt_str_field(p, "first_name")?,
                    last_name: opt_str_field(p, "last_name")?,
                    about: opt_str_field(p, "about")?,
                })
                .await?;
            let (id, first_name, last_name, username) = match r {
                tl::enums::User::User(u) => (u.id, u.first_name, u.last_name, u.username),
                _ => (0, None, None, None),
            };
            Ok(serde_json::json!({
                "id": id,
                "first_name": first_name,
                "last_name": last_name,
                "username": username,
            }))
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

fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(p) => p.user_id,
        tl::enums::Peer::Chat(p) => p.chat_id,
        tl::enums::Peer::Channel(p) => p.channel_id,
    }
}

fn update_summary(u: &tl::enums::Update) -> serde_json::Value {
    match u {
        tl::enums::Update::NewMessage(m) => match &m.message {
            tl::enums::Message::Message(msg) => serde_json::json!({
                "type": "new_message",
                "id": msg.id,
                "peer_id": peer_id(&msg.peer_id),
                "out": msg.out,
                "text": msg.message,
            }),
            _ => serde_json::json!({"type": "new_message"}),
        },
        tl::enums::Update::EditMessage(m) => match &m.message {
            tl::enums::Message::Message(msg) => serde_json::json!({
                "type": "edit_message",
                "id": msg.id,
                "peer_id": peer_id(&msg.peer_id),
                "text": msg.message,
            }),
            _ => serde_json::json!({"type": "edit_message"}),
        },
        tl::enums::Update::DraftMessage(d) => {
            let text = match &d.draft {
                tl::enums::DraftMessage::Message(draft) => draft.message.clone(),
                tl::enums::DraftMessage::Empty(_) => String::new(),
            };
            serde_json::json!({"type": "draft_message", "peer_id": peer_id(&d.peer), "text": text})
        }
        _ => serde_json::json!({"type": "other"}),
    }
}

fn stats_period(v: &tl::enums::StatsDateRangeDays) -> serde_json::Value {
    match v {
        tl::enums::StatsDateRangeDays::Days(d) => {
            serde_json::json!({"min_date": d.min_date, "max_date": d.max_date})
        }
    }
}

fn stats_abs(v: &tl::enums::StatsAbsValueAndPrev) -> serde_json::Value {
    match v {
        tl::enums::StatsAbsValueAndPrev::Prev(p) => {
            serde_json::json!({"current": p.current, "previous": p.previous})
        }
    }
}

fn stats_percent(v: &tl::enums::StatsPercentValue) -> serde_json::Value {
    match v {
        tl::enums::StatsPercentValue::Value(p) => {
            serde_json::json!({"part": p.part, "total": p.total})
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_missing_required_field_fails() {
        assert!(matches!(
            validate_params("contacts.Search", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("messages.ExportChatInvite", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("stats.GetBroadcastStats", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_wrong_type_fails() {
        assert!(matches!(
            validate_params("contacts.Search", &serde_json::json!({"q": 42})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "contacts.Search",
                &serde_json::json!({"q": "a", "limit": "5"})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_out_of_range_int_fails() {
        assert!(matches!(
            validate_params(
                "contacts.Search",
                &serde_json::json!({"q": "a", "limit": 9_999_999_999i64})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_non_object_fails() {
        assert!(matches!(
            validate_params("messages.GetAllDrafts", &serde_json::json!([])),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_valid_pass() {
        assert!(validate_params(
            "contacts.Search",
            &serde_json::json!({"q": "alice", "limit": 5, "broadcasts": true})
        )
        .is_ok());
        assert!(validate_params(
            "stats.GetMegagroupStats",
            &serde_json::json!({"channel": "@x"})
        )
        .is_ok());
        assert!(validate_params("messages.GetAllDrafts", &serde_json::json!({})).is_ok());
    }
}
