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

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
                rows.push(serde_json::json!({
                    "key": key,
                    "rules": format!("{rules:?}"),
                }));
            }
            Ok(serde_json::json!({"privacy": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let key_name = args.key.clone();
        let allow = args.allow.clone().unwrap_or_default();
        let deny = args.deny.clone().unwrap_or_default();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "key": key_name}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let tl_key = key_to_tl(&key_name).ok_or_else(|| {
                TeleError::Usage(format!(
                    "unknown privacy key {key_name} (one of {})",
                    keys().join(", ")
                ))
            })?;
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

fn creds() -> crate::TeleResult<crate::config::Credentials> {
    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))
}

fn creds_api_id() -> crate::TeleResult<i32> {
    Ok(creds()?.api_id)
}
