use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};

#[derive(Subcommand)]
pub enum PrivacyCmd {
    Get(GetArgs),
    Set(SetArgs),
}

#[derive(Args)]
pub struct GetArgs {
    #[arg(long)]
    key: Option<String>,
}

#[derive(Args)]
pub struct SetArgs {
    #[arg(long)]
    key: String,
    #[arg(long, value_delimiter = ',')]
    allow: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',')]
    deny: Option<Vec<String>>,
}

pub async fn run(cmd: PrivacyCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        PrivacyCmd::Get(a) => get(a, flags).await,
        PrivacyCmd::Set(a) => set(a, flags).await,
    }
}

fn keys() -> Vec<&'static str> {
    vec![
        "status",
        "profile_photo",
        "phone_number",
        "calls",
        "forwards",
        "chat_invite",
        "added_by_phone",
        "voice_messages",
        "about",
    ]
}

fn key_to_tl(key: &str) -> Option<tl::enums::InputPrivacyKey> {
    use tl::enums::InputPrivacyKey as K;
    match key {
        "status" => Some(K::StatusTimestamp),
        "profile_photo" => Some(K::ProfilePhoto),
        "phone_number" => Some(K::PhoneNumber),
        "calls" => Some(K::PhoneCall),
        "forwards" => Some(K::Forwards),
        "chat_invite" => Some(K::ChatInvite),
        "added_by_phone" => Some(K::AddedByPhone),
        "voice_messages" => Some(K::VoiceMessages),
        "about" => Some(K::About),
        _ => None,
    }
}

fn validate_get(args: &GetArgs) -> TeleResult<()> {
    if let Some(key) = &args.key {
        if !keys().contains(&key.as_str()) {
            return Err(TeleError::Usage(format!(
                "unknown privacy key {key} (one of {})",
                keys().join(", ")
            )));
        }
    }
    Ok(())
}

fn set_key(key: &str) -> TeleResult<tl::enums::InputPrivacyKey> {
    key_to_tl(key).ok_or_else(|| {
        TeleError::Usage(format!(
            "unknown privacy key {key} (one of {})",
            keys().join(", ")
        ))
    })
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_get(&args)?;
    let config_path = flags.config_path.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let key_filter = args.key.clone();
        Box::pin(async move {
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let mut rows = Vec::new();
            for key in keys() {
                if let Some(filter) = &key_filter {
                    if key != filter {
                        continue;
                    }
                }
                let Some(tl_key) = key_to_tl(key) else {
                    continue;
                };
                let rules: tl::enums::account::PrivacyRules = guard
                    .client
                    .invoke(&tl::functions::account::GetPrivacy { key: tl_key })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::account::PrivacyRules::Rules(rules) = rules;
                let summary = rules
                    .rules
                    .iter()
                    .map(privacy_rule_summary)
                    .collect::<Vec<serde_json::Value>>();
                rows.push(serde_json::json!({
                    "key": key,
                    "rules": summary,
                }));
            }
            Ok(serde_json::json!({"privacy": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let tl_key = set_key(&args.key)?;
    if args.allow.is_none() && args.deny.is_none() {
        return Err(TeleError::Usage(
            "privacy set requires --allow or --deny".to_string(),
        ));
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let key_name = args.key.clone();
        let allow = args.allow.clone().unwrap_or_default();
        let deny = args.deny.clone().unwrap_or_default();
        let tl_key = tl_key.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "key": key_name}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let mut rules: Vec<tl::enums::InputPrivacyRule> = Vec::new();
            if !allow.is_empty() {
                let mut users = Vec::new();
                for target in &allow {
                    let peer = entities::resolve_peer(&guard.client, target)
                        .await
                        .map_err(tele_invocation)?;
                    users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
                }
                rules.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users },
                ));
            }
            if !deny.is_empty() {
                let mut users = Vec::new();
                for target in &deny {
                    let peer = entities::resolve_peer(&guard.client, target)
                        .await
                        .map_err(tele_invocation)?;
                    users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
                }
                rules.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users },
                ));
            }
            if rules.is_empty() {
                rules.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll);
            }
            let _: tl::enums::account::PrivacyRules = guard
                .client
                .invoke(&tl::functions::account::SetPrivacy { key: tl_key, rules })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"key": key_name, "allow": allow, "deny": deny}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn privacy_rule_summary(r: &tl::enums::PrivacyRule) -> serde_json::Value {
    match r {
        tl::enums::PrivacyRule::PrivacyValueAllowAll => serde_json::json!("allow_all"),
        tl::enums::PrivacyRule::PrivacyValueDisallowAll => serde_json::json!("disallow_all"),
        tl::enums::PrivacyRule::PrivacyValueAllowContacts => {
            serde_json::json!("allow_contacts")
        }
        tl::enums::PrivacyRule::PrivacyValueDisallowContacts => {
            serde_json::json!("disallow_contacts")
        }
        tl::enums::PrivacyRule::PrivacyValueAllowCloseFriends => {
            serde_json::json!("allow_close_friends")
        }
        tl::enums::PrivacyRule::PrivacyValueAllowPremium => serde_json::json!("allow_premium"),
        tl::enums::PrivacyRule::PrivacyValueAllowBots => serde_json::json!("allow_bots"),
        tl::enums::PrivacyRule::PrivacyValueDisallowBots => serde_json::json!("disallow_bots"),
        tl::enums::PrivacyRule::PrivacyValueAllowUsers(v) => {
            serde_json::json!({"kind": "allow_users", "ids": v.users})
        }
        tl::enums::PrivacyRule::PrivacyValueDisallowUsers(v) => {
            serde_json::json!({"kind": "disallow_users", "ids": v.users})
        }
        tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(v) => {
            serde_json::json!({"kind": "allow_chats", "ids": v.chats})
        }
        tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(v) => {
            serde_json::json!({"kind": "disallow_chats", "ids": v.chats})
        }
    }
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
    fn get_rejects_unknown_key() {
        let args = GetArgs {
            key: Some("shoe_size".to_string()),
        };
        assert!(matches!(validate_get(&args), Err(TeleError::Usage(_))));
        let ok = GetArgs {
            key: Some("status".to_string()),
        };
        assert!(validate_get(&ok).is_ok());
        let all = GetArgs { key: None };
        assert!(validate_get(&all).is_ok());
    }

    #[test]
    fn set_rejects_unknown_key() {
        assert!(matches!(set_key("nope"), Err(TeleError::Usage(_))));
        assert!(set_key("calls").is_ok());
    }
}
