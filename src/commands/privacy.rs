use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum PrivacyCmd {
    Get(GetArgs),
    Set(SetArgs),
}

#[derive(Args, Clone)]
pub struct GetArgs {
    #[arg(
        long,
        help = "privacy key: status, profile_photo, phone_number, calls, forwards, chat_invite, added_by_phone, voice_messages, about, phone_p2p, birthday, star_gifts_auto_save, no_paid_messages, saved_music"
    )]
    key: Option<String>,
}

#[derive(Args, Clone)]
pub struct SetArgs {
    #[arg(long, help = "privacy key to change")]
    key: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "users to allow: comma-separated @username, ID, or me"
    )]
    allow: Option<Vec<String>>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "chat/group/channel member IDs to allow: comma-separated positive IDs"
    )]
    allow_chat: Option<Vec<i64>>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "users to deny: comma-separated @username, ID, or me"
    )]
    deny: Option<Vec<String>>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "chat/group/channel member IDs to deny: comma-separated positive IDs"
    )]
    deny_chat: Option<Vec<i64>>,
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
        "phone_p2p",
        "birthday",
        "star_gifts_auto_save",
        "no_paid_messages",
        "saved_music",
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
        "phone_p2p" => Some(K::PhoneP2P),
        "birthday" => Some(K::Birthday),
        "star_gifts_auto_save" => Some(K::StarGiftsAutoSave),
        "no_paid_messages" => Some(K::NoPaidMessages),
        "saved_music" => Some(K::SavedMusic),
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
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return get_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let data = get_core(&guard.shares(), GetParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                output::print_table(&["key", "rule", "peers"], &privacy_table_rows(&data))?;
            }
            Ok(data)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn fetch_privacy_rules(
    client: &grammers_client::Client,
    key: &tl::enums::InputPrivacyKey,
) -> TeleResult<Vec<tl::enums::PrivacyRule>> {
    let rules: tl::enums::account::PrivacyRules = client
        .invoke(&tl::functions::account::GetPrivacy { key: key.clone() })
        .await
        .map_err(tele_invocation)?;
    let tl::enums::account::PrivacyRules::Rules(rules) = rules;
    Ok(rules.rules)
}

fn dry_run_get_data(key: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "key": key,
        "would": match &key {
            Some(k) => format!("get privacy rules for key {k}"),
            None => "get privacy rules for all keys".to_string(),
        }
    })
}

fn validate_set(args: &SetArgs) -> TeleResult<tl::enums::InputPrivacyKey> {
    let key = set_key(&args.key)?;
    if args.allow.is_none()
        && args.allow_chat.is_none()
        && args.deny.is_none()
        && args.deny_chat.is_none()
    {
        return Err(TeleError::Usage(
            "privacy set requires --allow, --allow-chat, --deny or --deny-chat".to_string(),
        ));
    }
    for (name, values) in [
        ("--allow", args.allow.as_ref()),
        ("--deny", args.deny.as_ref()),
    ] {
        if let Some(values) = values {
            if values.is_empty() || values.iter().any(|t| t.trim().is_empty()) {
                return Err(TeleError::Usage(format!(
                    "privacy set {name} must name at least one user; got an empty value"
                )));
            }
        }
    }
    for (name, values) in [
        ("--allow-chat", args.allow_chat.as_ref()),
        ("--deny-chat", args.deny_chat.as_ref()),
    ] {
        if let Some(values) = values {
            if values.is_empty() {
                return Err(TeleError::Usage(format!(
                    "privacy set {name} must name at least one positive chat ID"
                )));
            }
            if let Some(bad) = values.iter().find(|id| **id <= 0) {
                return Err(TeleError::Usage(format!(
                    "privacy set {name} takes positive chat IDs; got {bad}"
                )));
            }
        }
    }
    reject_allow_deny_overlap(args)?;
    Ok(key)
}

fn normalize_raw_target(raw: &str) -> String {
    let mut s = raw.trim();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
        }
    }
    for prefix in ["t.me/", "telegram.me/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
        }
    }
    let s = s.strip_prefix('@').unwrap_or(s);
    match s.parse::<i64>() {
        Ok(n) => n.to_string(),
        Err(_) => s.to_ascii_lowercase(),
    }
}

fn normalize_target(raw: &str, tag: &str) -> String {
    format!("{tag}:{}", normalize_raw_target(raw))
}

fn reject_allow_deny_overlap(args: &SetArgs) -> TeleResult<()> {
    let mut allow_keys: Vec<String> = args
        .allow
        .iter()
        .flatten()
        .map(|t| normalize_target(t, "u"))
        .collect();
    allow_keys.extend(
        args.allow_chat
            .iter()
            .flatten()
            .map(|id| normalize_target(&id.to_string(), "c")),
    );
    let deny_labels: Vec<(String, &str)> = args
        .deny
        .iter()
        .flatten()
        .map(|t| (t.clone(), "u"))
        .chain(
            args.deny_chat
                .iter()
                .flatten()
                .map(|id| (id.to_string(), "c")),
        )
        .collect();
    for (target, tag) in &deny_labels {
        if allow_keys.contains(&normalize_target(target, tag)) {
            return Err(TeleError::Usage(format!(
                "privacy set cannot place target {target} on both the allow side (--allow/--allow-chat) and the deny side (--deny/--deny-chat)"
            )));
        }
    }
    Ok(())
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_set(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return set_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            set_core(&guard.shares(), SetParams::from(&args)).await
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

fn input_user_from_id(user_id: i64) -> tl::enums::InputUser {
    tl::enums::InputUser::User(tl::types::InputUser {
        user_id,
        access_hash: 0,
    })
}

struct PrivacyTargets {
    allow_users: Vec<tl::enums::InputUser>,
    allow_chats: Vec<i64>,
    disallow_users: Vec<tl::enums::InputUser>,
    disallow_chats: Vec<i64>,
}

fn merge_privacy_rules(
    base: &[tl::enums::PrivacyRule],
    targets: &PrivacyTargets,
) -> Vec<tl::enums::InputPrivacyRule> {
    let mut merged = Vec::with_capacity(base.len() + 4);
    let mut base_allow_user_ids: Vec<i64> = Vec::new();
    let mut base_allow_chat_ids: Vec<i64> = Vec::new();
    let mut base_disallow_user_ids: Vec<i64> = Vec::new();
    let mut base_disallow_chat_ids: Vec<i64> = Vec::new();
    for rule in base {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(v) => {
                base_allow_user_ids.extend(&v.users);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(v) => {
                base_allow_chat_ids.extend(&v.chats);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(v) => {
                base_disallow_user_ids.extend(&v.users);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(v) => {
                base_disallow_chat_ids.extend(&v.chats);
            }
            _ => {}
        }
    }
    let new_allow_users: Vec<tl::enums::InputUser> = targets
        .allow_users
        .iter()
        .filter(|u| match u {
            tl::enums::InputUser::User(u) => !base_allow_user_ids.contains(&u.user_id),
            _ => true,
        })
        .cloned()
        .collect();
    let mut merged_allow_users: Vec<tl::enums::InputUser> = base_allow_user_ids
        .iter()
        .map(|id| input_user_from_id(*id))
        .collect();
    merged_allow_users.extend(new_allow_users);
    if !merged_allow_users.is_empty() {
        merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
            tl::types::InputPrivacyValueAllowUsers {
                users: merged_allow_users,
            },
        ));
    }
    let mut merged_allow_chats = base_allow_chat_ids;
    for id in &targets.allow_chats {
        if !merged_allow_chats.contains(id) {
            merged_allow_chats.push(*id);
        }
    }
    if !merged_allow_chats.is_empty() {
        merged.push(
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                tl::types::InputPrivacyValueAllowChatParticipants {
                    chats: merged_allow_chats,
                },
            ),
        );
    }
    for rule in base {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueAllowContacts => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowCloseFriends => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowCloseFriends);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowPremium => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowPremium);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowBots => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowBots);
            }
            _ => {}
        }
    }
    let new_disallow_users: Vec<tl::enums::InputUser> = targets
        .disallow_users
        .iter()
        .filter(|u| match u {
            tl::enums::InputUser::User(u) => !base_disallow_user_ids.contains(&u.user_id),
            _ => true,
        })
        .cloned()
        .collect();
    let mut merged_disallow_users: Vec<tl::enums::InputUser> = base_disallow_user_ids
        .iter()
        .map(|id| input_user_from_id(*id))
        .collect();
    merged_disallow_users.extend(new_disallow_users);
    if !merged_disallow_users.is_empty() {
        merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
            tl::types::InputPrivacyValueDisallowUsers {
                users: merged_disallow_users,
            },
        ));
    }
    for rule in base {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueDisallowContacts => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowContacts);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowBots => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowBots);
            }
            _ => {}
        }
    }
    let mut merged_disallow_chats = base_disallow_chat_ids;
    for id in &targets.disallow_chats {
        if !merged_disallow_chats.contains(id) {
            merged_disallow_chats.push(*id);
        }
    }
    if !merged_disallow_chats.is_empty() {
        merged.push(
            tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                tl::types::InputPrivacyValueDisallowChatParticipants {
                    chats: merged_disallow_chats,
                },
            ),
        );
    }
    for rule in base {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueAllowAll => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowAll => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll);
            }
            _ => {}
        }
    }
    merged
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub(crate) struct GetParams {
    #[serde(default)]
    pub(crate) key: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&GetArgs> for GetParams {
    fn from(a: &GetArgs) -> Self {
        Self {
            key: a.key.clone(),
            dry_run: false,
        }
    }
}

impl From<&GetParams> for GetArgs {
    fn from(p: &GetParams) -> Self {
        Self { key: p.key.clone() }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub(crate) struct SetParams {
    pub(crate) key: String,
    pub(crate) allow: Option<Vec<String>>,
    pub(crate) allow_chat: Option<Vec<i64>>,
    pub(crate) deny: Option<Vec<String>>,
    pub(crate) deny_chat: Option<Vec<i64>>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SetArgs> for SetParams {
    fn from(a: &SetArgs) -> Self {
        Self {
            key: a.key.clone(),
            allow: a.allow.clone(),
            allow_chat: a.allow_chat.clone(),
            deny: a.deny.clone(),
            deny_chat: a.deny_chat.clone(),
            dry_run: false,
        }
    }
}

impl From<&SetParams> for SetArgs {
    fn from(p: &SetParams) -> Self {
        Self {
            key: p.key.clone(),
            allow: p.allow.clone(),
            allow_chat: p.allow_chat.clone(),
            deny: p.deny.clone(),
            deny_chat: p.deny_chat.clone(),
        }
    }
}

fn get_serve_dry_run(args: &GetArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_get_data(args.key.clone()))
}

fn set_serve_dry_run(args: &SetArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "key": args.key,
        "would": format!(
            "set privacy rules for key {} (allow {} users + {} chats, deny {} users + {} chats)",
            args.key,
            args.allow.as_ref().map_or(0, Vec::len),
            args.allow_chat.as_ref().map_or(0, Vec::len),
            args.deny.as_ref().map_or(0, Vec::len),
            args.deny_chat.as_ref().map_or(0, Vec::len),
        )
    }))
}

fn summary_display(rule: &serde_json::Value) -> (String, String) {
    if let Some(kind) = rule.as_str() {
        return (kind.to_string(), String::new());
    }
    let kind = rule["kind"].as_str().unwrap_or_default().to_string();
    let peers = rule["ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    (kind, peers)
}

fn privacy_table_rows(data: &serde_json::Value) -> Vec<Vec<String>> {
    let mut table_rows = Vec::new();
    for row in data["privacy"].as_array().unwrap_or(&Vec::new()) {
        let key = row["key"].as_str().unwrap_or_default().to_string();
        for rule in row["rules"].as_array().unwrap_or(&Vec::new()) {
            let (kind, peers) = summary_display(rule);
            table_rows.push(vec![key.clone(), kind, peers]);
        }
    }
    table_rows
}

pub(crate) async fn get_core(
    shares: &crate::client::ServeShares,
    params: GetParams,
) -> TeleResult<serde_json::Value> {
    let mut rows = Vec::new();
    for key in keys() {
        if let Some(filter) = &params.key {
            if key != filter {
                continue;
            }
        }
        let Some(tl_key) = key_to_tl(key) else {
            continue;
        };
        shares.rate_limiter.acquire().await;
        let rules = fetch_privacy_rules(&shares.client, &tl_key).await?;
        let summary = rules
            .iter()
            .map(privacy_rule_summary)
            .collect::<Vec<serde_json::Value>>();
        rows.push(serde_json::json!({
            "key": key,
            "rules": summary,
        }));
    }
    Ok(serde_json::json!({"privacy": rows}))
}

pub(crate) async fn set_core(
    shares: &crate::client::ServeShares,
    params: SetParams,
) -> TeleResult<serde_json::Value> {
    let tl_key = set_key(&params.key)?;
    let allow = params.allow.clone().unwrap_or_default();
    let deny = params.deny.clone().unwrap_or_default();
    shares.rate_limiter.acquire().await;
    let mut allow_users = Vec::new();
    for target in &allow {
        let peer = entities::resolve_peer(&shares.client, shares.session.as_ref(), target).await?;
        allow_users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
    }
    let mut disallow_users = Vec::new();
    for target in &deny {
        let peer = entities::resolve_peer(&shares.client, shares.session.as_ref(), target).await?;
        disallow_users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
    }
    let base = fetch_privacy_rules(&shares.client, &tl_key).await?;
    let targets = PrivacyTargets {
        allow_users,
        allow_chats: params.allow_chat.unwrap_or_default(),
        disallow_users,
        disallow_chats: params.deny_chat.unwrap_or_default(),
    };
    let rules = merge_privacy_rules(&base, &targets);
    let _: tl::enums::account::PrivacyRules = shares
        .client
        .invoke(&tl::functions::account::SetPrivacy { key: tl_key, rules })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "key": params.key,
        "allow": allow,
        "allow_chats": targets.allow_chats,
        "deny": deny,
        "deny_chats": targets.disallow_chats,
    }))
}

pub(crate) fn privacy_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "privacy get",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "show one privacy setting",
            GetParams,
            GetArgs,
            validate_get,
            get_serve_dry_run,
            run_get,
            crate::commands::serve::params_schema::<GetParams>
        ),
        crate::serve_route!(
            "privacy set",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "set one privacy setting",
            SetParams,
            SetArgs,
            validate_set,
            set_serve_dry_run,
            run_set,
            crate::commands::serve::params_schema::<SetParams>
        ),
    ]
}

crate::serve_runner!(run_get, get_core, GetParams);
crate::serve_runner!(run_set, set_core, SetParams);

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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

    #[test]
    fn set_rejects_empty_allow() {
        let cases = vec![
            Some(vec![]),
            Some(vec!["".to_string()]),
            Some(vec!["   ".to_string()]),
            Some(vec!["@alice".to_string(), " ".to_string()]),
        ];
        for allow in cases {
            let label = format!("{allow:?}");
            let args = SetArgs {
                key: "status".to_string(),
                allow,
                deny: None,
                allow_chat: None,
                deny_chat: None,
            };
            assert!(
                matches!(validate_set(&args), Err(TeleError::Usage(_))),
                "allow = {label}"
            );
        }
    }

    #[test]
    fn set_rejects_empty_deny() {
        let cases = vec![
            Some(vec![]),
            Some(vec!["".to_string()]),
            Some(vec!["   ".to_string()]),
            Some(vec!["@bob".to_string(), "\t".to_string()]),
        ];
        for deny in cases {
            let label = format!("{deny:?}");
            let args = SetArgs {
                key: "status".to_string(),
                allow: None,
                allow_chat: None,
                deny,
                deny_chat: None,
            };
            assert!(
                matches!(validate_set(&args), Err(TeleError::Usage(_))),
                "deny = {label}"
            );
        }
    }

    #[test]
    fn set_rejects_all_blank_allow() {
        let args = SetArgs {
            key: "status".to_string(),
            allow: Some(vec!["  ".to_string(), "\t".to_string(), "".to_string()]),
            deny: None,
            allow_chat: None,
            deny_chat: None,
        };
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_rejects_all_blank_deny() {
        let args = SetArgs {
            key: "status".to_string(),
            allow: None,
            deny: Some(vec!["  ".to_string(), "\t".to_string()]),
            allow_chat: None,
            deny_chat: None,
        };
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_accepts_allow_and_deny_together() {
        let args = SetArgs {
            key: "status".to_string(),
            allow: Some(vec!["@alice".to_string()]),
            deny: Some(vec!["@bob".to_string()]),
            allow_chat: None,
            deny_chat: None,
        };
        assert!(validate_set(&args).is_ok());
    }

    #[test]
    fn set_absent_allow_unchanged() {
        let with_deny = SetArgs {
            key: "calls".to_string(),
            allow: None,
            deny: Some(vec!["@x".to_string()]),
            allow_chat: None,
            deny_chat: None,
        };
        assert!(validate_set(&with_deny).is_ok());
        let neither = SetArgs {
            key: "calls".to_string(),
            allow: None,
            deny: None,
            allow_chat: None,
            deny_chat: None,
        };
        assert!(matches!(validate_set(&neither), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_empty_allow_flag_rejected() {
        let parsed = crate::Cli::try_parse_from([
            "tele", "privacy", "set", "--key", "status", "--allow", "",
        ]);
        if let Ok(cli) = parsed {
            let crate::Command::Privacy(PrivacyCmd::Set(args)) = cli.command else {
                panic!("expected privacy set");
            };
            assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
        }
    }

    #[test]
    fn set_empty_deny_flag_rejected() {
        let parsed =
            crate::Cli::try_parse_from(["tele", "privacy", "set", "--key", "status", "--deny", ""]);
        if let Ok(cli) = parsed {
            let crate::Command::Privacy(PrivacyCmd::Set(args)) = cli.command else {
                panic!("expected privacy set");
            };
            assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
        }
    }

    fn privacy_flags(command: &str, config: &std::path::Path) -> GlobalFlags {
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
        let dir =
            std::env::temp_dir().join(format!("telecli-privacy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "[accounts.work]\ntags = []\n").unwrap();
        dir
    }

    #[test]
    fn dry_run_get_data_marks_dry_run_and_echoes_key() {
        let filtered = dry_run_get_data(Some("status".to_string()));
        assert_eq!(filtered["dry_run"], serde_json::json!(true));
        assert_eq!(filtered["key"], serde_json::json!("status"));
        let all = dry_run_get_data(None);
        assert_eq!(all["dry_run"], serde_json::json!(true));
        assert_eq!(all["key"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn get_dry_run_exits_ok_before_connect() {
        let dir = temp_app("get-dry");
        let flags = privacy_flags("privacy get", &dir.join("config.toml"));
        let code = get(
            GetArgs {
                key: Some("status".to_string()),
            },
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let code_all = get(GetArgs { key: None }, &flags).await.unwrap();
        assert_eq!(code_all, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_dry_run_still_validates_key() {
        let dir = temp_app("get-key");
        let flags = privacy_flags("privacy get", &dir.join("config.toml"));
        let err = get(
            GetArgs {
                key: Some("bogus".to_string()),
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn iu(user_id: i64) -> tl::enums::InputUser {
        input_user_from_id(user_id)
    }

    fn targets(
        allow: &[tl::enums::InputUser],
        disallow: &[tl::enums::InputUser],
    ) -> PrivacyTargets {
        PrivacyTargets {
            allow_users: allow.to_vec(),
            allow_chats: Vec::new(),
            disallow_users: disallow.to_vec(),
            disallow_chats: Vec::new(),
        }
    }

    fn base_with_contacts_and_user_rules() -> Vec<tl::enums::PrivacyRule> {
        vec![
            tl::enums::PrivacyRule::PrivacyValueAllowContacts,
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(tl::types::PrivacyValueAllowUsers {
                users: vec![1, 2],
            }),
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(
                tl::types::PrivacyValueDisallowUsers { users: vec![3] },
            ),
        ]
    }

    #[test]
    fn merge_unions_new_allow_with_base_allow() {
        let base = base_with_contacts_and_user_rules();
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers {
                        users: vec![iu(1), iu(2), iu(5)]
                    },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(3)] },
                ),
            ]
        );
    }

    #[test]
    fn merge_keeps_allow_rule_when_only_deny_given() {
        let base = vec![
            tl::enums::PrivacyRule::PrivacyValueAllowAll,
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(tl::types::PrivacyValueAllowUsers {
                users: vec![1],
            }),
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(
                tl::types::PrivacyValueDisallowUsers { users: vec![2] },
            ),
        ];
        let merged = merge_privacy_rules(&base, &targets(&[], &[iu(4)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(1)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers {
                        users: vec![iu(2), iu(4)]
                    },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll,
            ]
        );
    }

    #[test]
    fn merge_unions_both_user_rules_when_both_given() {
        let base = base_with_contacts_and_user_rules();
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[iu(6)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers {
                        users: vec![iu(1), iu(2), iu(5)]
                    },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers {
                        users: vec![iu(3), iu(6)]
                    },
                ),
            ]
        );
    }

    #[test]
    fn merge_keeps_chat_rules() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(
            tl::types::PrivacyValueAllowChatParticipants { chats: vec![777] },
        )];
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants { chats: vec![777] },
                ),
            ]
        );
    }

    #[test]
    fn merge_orders_deny_before_allow_all() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowAll];
        let merged = merge_privacy_rules(&base, &targets(&[], &[iu(4)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(4)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll,
            ]
        );
    }

    #[test]
    fn merge_orders_allow_before_disallow_all() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueDisallowAll];
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll,
            ]
        );
    }

    #[test]
    fn merge_orders_contacts_deny_then_disallow_all() {
        let base = vec![
            tl::enums::PrivacyRule::PrivacyValueAllowContacts,
            tl::enums::PrivacyRule::PrivacyValueDisallowAll,
        ];
        let merged = merge_privacy_rules(&base, &targets(&[], &[iu(4)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(4)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll,
            ]
        );
    }

    #[test]
    fn merge_keeps_non_user_disallow_rules_in_place() {
        let base = vec![
            tl::enums::PrivacyRule::PrivacyValueAllowContacts,
            tl::enums::PrivacyRule::PrivacyValueDisallowContacts,
            tl::enums::PrivacyRule::PrivacyValueDisallowBots,
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![555] },
            ),
            tl::enums::PrivacyRule::PrivacyValueDisallowAll,
        ];
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowBots,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                    tl::types::InputPrivacyValueDisallowChatParticipants { chats: vec![555] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll,
            ]
        );
    }

    #[test]
    fn merge_empty_base_yields_only_new_rules() {
        let merged = merge_privacy_rules(&[], &targets(&[iu(5)], &[iu(6)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(6)] },
                ),
            ]
        );
        assert!(merge_privacy_rules(&[], &targets(&[], &[])).is_empty());
    }

    #[test]
    fn rule_display_renders_kind_and_peers() {
        let (kind, peers) = privacy_rule_display(&tl::enums::PrivacyRule::PrivacyValueAllowAll);
        assert_eq!(kind, "allow_all");
        assert_eq!(peers, "");
        let (kind, peers) = privacy_rule_display(&tl::enums::PrivacyRule::PrivacyValueAllowUsers(
            tl::types::PrivacyValueAllowUsers { users: vec![1, 2] },
        ));
        assert_eq!(kind, "allow_users");
        assert_eq!(peers, "1, 2");
        let (kind, peers) = privacy_rule_display(
            &tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![9] },
            ),
        );
        assert_eq!(kind, "disallow_chats");
        assert_eq!(peers, "9");
    }

    #[test]
    fn key_mapping_covers_full_table_both_directions() {
        use tl::enums::InputPrivacyKey as K;
        let table: Vec<(&str, K)> = vec![
            ("status", K::StatusTimestamp),
            ("profile_photo", K::ProfilePhoto),
            ("phone_number", K::PhoneNumber),
            ("calls", K::PhoneCall),
            ("forwards", K::Forwards),
            ("chat_invite", K::ChatInvite),
            ("added_by_phone", K::AddedByPhone),
            ("voice_messages", K::VoiceMessages),
            ("about", K::About),
            ("phone_p2p", K::PhoneP2P),
            ("birthday", K::Birthday),
            ("star_gifts_auto_save", K::StarGiftsAutoSave),
            ("no_paid_messages", K::NoPaidMessages),
            ("saved_music", K::SavedMusic),
        ];
        for (key, want) in &table {
            let got = key_to_tl(key).unwrap_or_else(|| panic!("{key} missing mapping"));
            assert_eq!(
                std::mem::discriminant(&got),
                std::mem::discriminant(want),
                "{key} maps to the wrong variant"
            );
        }
        assert_eq!(keys(), table.iter().map(|(k, _)| *k).collect::<Vec<_>>());
        assert!(key_to_tl("no_such_key").is_none());
    }

    fn overlap_args(
        allow: Option<Vec<String>>,
        allow_chat: Option<Vec<i64>>,
        deny: Option<Vec<String>>,
        deny_chat: Option<Vec<i64>>,
    ) -> SetArgs {
        SetArgs {
            key: "status".to_string(),
            allow,
            allow_chat,
            deny,
            deny_chat,
        }
    }

    #[test]
    fn set_rejects_same_username_on_allow_and_deny() {
        let args = overlap_args(
            Some(vec!["@alice".to_string()]),
            None,
            Some(vec!["alice".to_string()]),
            None,
        );
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_rejects_overlap_across_case_and_link_forms() {
        let cases = [
            (
                Some(vec!["@Alice".to_string()]),
                Some(vec!["https://t.me/alice".to_string()]),
            ),
            (
                Some(vec!["t.me/Bob".to_string()]),
                Some(vec!["@bob".to_string()]),
            ),
            (
                Some(vec!["12345".to_string()]),
                Some(vec!["12345".to_string()]),
            ),
        ];
        for (allow, deny) in cases {
            let args = overlap_args(allow, None, deny, None);
            assert!(
                matches!(validate_set(&args), Err(TeleError::Usage(_))),
                "expected overlap rejection"
            );
        }
    }

    #[test]
    fn set_rejects_same_chat_id_on_allow_and_deny() {
        let args = overlap_args(None, Some(vec![777]), None, Some(vec![777]));
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
        let cross = overlap_args(
            Some(vec!["@alice".to_string()]),
            Some(vec![777]),
            None,
            Some(vec![777, 888]),
        );
        assert!(matches!(validate_set(&cross), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_allows_disjoint_targets_on_each_side() {
        let args = overlap_args(
            Some(vec!["@alice".to_string(), "999".to_string()]),
            Some(vec![777]),
            Some(vec!["@bob".to_string()]),
            Some(vec![555]),
        );
        assert!(validate_set(&args).is_ok());
    }

    #[test]
    fn set_accepts_chat_only_rules() {
        let allow_only = overlap_args(None, Some(vec![777]), None, None);
        assert!(validate_set(&allow_only).is_ok());
        let deny_only = overlap_args(None, None, None, Some(vec![777]));
        assert!(validate_set(&deny_only).is_ok());
    }

    #[test]
    fn set_rejects_nonpositive_chat_ids() {
        let bad_allow = overlap_args(None, Some(vec![0]), None, None);
        assert!(matches!(validate_set(&bad_allow), Err(TeleError::Usage(_))));
        let bad_deny = overlap_args(None, None, None, Some(vec![-5]));
        assert!(matches!(validate_set(&bad_deny), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_allows_user777_and_chat777_without_false_collision() {
        let args = overlap_args(None, Some(vec![777]), Some(vec!["777".to_string()]), None);
        assert!(validate_set(&args).is_ok());
    }

    #[test]
    fn set_rejects_same_user_id_on_allow_and_deny() {
        let args = overlap_args(
            Some(vec!["12345".to_string()]),
            None,
            Some(vec!["12345".to_string()]),
            None,
        );
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_rejects_same_chat_id_on_allow_and_deny_tagged() {
        let args = overlap_args(None, Some(vec![777]), None, Some(vec![777]));
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn merge_unions_allow_users_deduplicating() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowUsers(
            tl::types::PrivacyValueAllowUsers { users: vec![1, 2] },
        )];
        let merged = merge_privacy_rules(&base, &targets(&[iu(2), iu(3)], &[]));
        assert_eq!(
            merged,
            vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                tl::types::InputPrivacyValueAllowUsers {
                    users: vec![iu(1), iu(2), iu(3)]
                },
            )]
        );
    }

    #[test]
    fn merge_unions_disallow_users_deduplicating() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueDisallowUsers(
            tl::types::PrivacyValueDisallowUsers { users: vec![10] },
        )];
        let merged = merge_privacy_rules(&base, &targets(&[], &[iu(10), iu(20)]));
        assert_eq!(
            merged,
            vec![tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                tl::types::InputPrivacyValueDisallowUsers {
                    users: vec![iu(10), iu(20)]
                },
            )]
        );
    }

    #[test]
    fn merge_unions_chat_participants_deduplicating() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(
            tl::types::PrivacyValueAllowChatParticipants { chats: vec![1, 2] },
        )];
        let t = PrivacyTargets {
            allow_users: Vec::new(),
            allow_chats: vec![2, 3],
            disallow_users: Vec::new(),
            disallow_chats: Vec::new(),
        };
        let merged = merge_privacy_rules(&base, &t);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants {
                        chats: vec![1, 2, 3]
                    },
                )
            ]
        );
    }

    #[test]
    fn normalize_target_canonicalizes_equivalent_spellings() {
        assert_eq!(normalize_target("@Alice", "u"), "u:alice");
        assert_eq!(normalize_target("https://t.me/alice", "u"), "u:alice");
        assert_eq!(normalize_target("telegram.me/@alice", "u"), "u:alice");
        assert_eq!(normalize_target(" http://t.me/Alice ", "u"), "u:alice");
        assert_eq!(normalize_target("12345", "u"), "u:12345");
        assert_eq!(normalize_target("+989121234567", "u"), "u:989121234567");
    }

    #[test]
    fn merge_adds_chat_participant_rules_from_cli() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowAll];
        let t = PrivacyTargets {
            allow_users: Vec::new(),
            allow_chats: vec![10],
            disallow_users: Vec::new(),
            disallow_chats: vec![20],
        };
        let merged = merge_privacy_rules(&base, &t);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants { chats: vec![10] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                    tl::types::InputPrivacyValueDisallowChatParticipants { chats: vec![20] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll,
            ]
        );
    }

    #[test]
    fn merge_unions_base_chat_rules_when_cli_chat_rules_given() {
        let base = vec![
            tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(
                tl::types::PrivacyValueAllowChatParticipants { chats: vec![1] },
            ),
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![2] },
            ),
        ];
        let t = PrivacyTargets {
            allow_users: Vec::new(),
            allow_chats: vec![3],
            disallow_users: Vec::new(),
            disallow_chats: Vec::new(),
        };
        let merged = merge_privacy_rules(&base, &t);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants { chats: vec![1, 3] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                    tl::types::InputPrivacyValueDisallowChatParticipants { chats: vec![2] },
                ),
            ]
        );
    }

    #[test]
    fn merge_keeps_base_chat_rules_when_no_chat_flags_given() {
        let base = vec![
            tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(
                tl::types::PrivacyValueAllowChatParticipants { chats: vec![1] },
            ),
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![2] },
            ),
        ];
        let merged = merge_privacy_rules(&base, &targets(&[iu(5)], &[iu(6)]));
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants { chats: vec![1] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(6)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                    tl::types::InputPrivacyValueDisallowChatParticipants { chats: vec![2] },
                ),
            ]
        );
    }

    #[test]
    fn cli_parses_new_privacy_flags() {
        let parsed = crate::Cli::try_parse_from([
            "tele",
            "privacy",
            "set",
            "--key",
            "birthday",
            "--allow-chat",
            "100,200",
            "--deny-chat",
            "300",
        ]);
        if let Ok(cli) = parsed {
            let crate::Command::Privacy(PrivacyCmd::Set(args)) = cli.command else {
                panic!("expected privacy set");
            };
            assert_eq!(args.allow_chat, Some(vec![100, 200]));
            assert_eq!(args.deny_chat, Some(vec![300]));
            assert!(validate_set(&args).is_ok());
        } else {
            panic!("privacy set with chat flags failed to parse");
        }
    }

    fn plan_for(
        op: &str,
        params: serde_json::Value,
    ) -> Result<crate::commands::serve::Plan, serde_json::Value> {
        let routes = privacy_serve_routes();
        let route = routes
            .iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    fn privacy_rule_display(rule: &tl::enums::PrivacyRule) -> (String, String) {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueAllowAll => {
                ("allow_all".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowAll => {
                ("disallow_all".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueAllowContacts => {
                ("allow_contacts".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowContacts => {
                ("disallow_contacts".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueAllowCloseFriends => {
                ("allow_close_friends".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueAllowPremium => {
                ("allow_premium".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueAllowBots => {
                ("allow_bots".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowBots => {
                ("disallow_bots".to_string(), String::new())
            }
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(v) => (
                "allow_users".to_string(),
                v.users
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            ),
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(v) => (
                "disallow_users".to_string(),
                v.users
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            ),
            tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(v) => (
                "allow_chats".to_string(),
                v.chats
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            ),
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(v) => (
                "disallow_chats".to_string(),
                v.chats
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            ),
        }
    }

    #[test]
    fn serve_routes_declare_lanes_and_timeouts() {
        use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
        let routes = privacy_serve_routes();
        let want: Vec<(&str, Lane, Option<std::time::Duration>)> = vec![
            ("privacy get", Lane::Read, Some(OP_TIMEOUT_PAGINATED)),
            ("privacy set", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
        ];
        assert_eq!(routes.len(), want.len());
        for (op, lane, timeout) in want {
            let route = routes
                .iter()
                .find(|r| r.op == op)
                .unwrap_or_else(|| panic!("{op}"));
            assert_eq!(route.lane, lane, "{op}");
            assert_eq!(route.timeout, timeout, "{op}");
        }
    }

    #[test]
    fn serve_missing_required_field_yields_serve_error() {
        let err = plan_for("privacy set", serde_json::json!({})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("privacy set"), "{msg}");
        assert!(msg.contains("key"), "{msg}");
        assert!(msg.contains("missing field"), "{msg}");
    }

    #[test]
    fn serve_wrong_type_param_yields_serve_error() {
        for (op, params, fragment) in [
            (
                "privacy get",
                serde_json::json!({"key": 5}),
                "expected a string",
            ),
            (
                "privacy set",
                serde_json::json!({"key": "status", "allow": "@alice"}),
                "expected a sequence",
            ),
            (
                "privacy set",
                serde_json::json!({"key": "status", "allow_chat": ["x"]}),
                "i64",
            ),
            (
                "privacy set",
                serde_json::json!({"key": "status", "deny_chat": -3}),
                "expected a sequence",
            ),
        ] {
            let err = plan_for(op, params).unwrap_err();
            assert_eq!(err["type"], "ServeError", "{op}");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.contains(op), "{op}: {msg}");
            assert!(msg.contains(fragment), "{op}: {msg}");
        }
    }

    #[test]
    fn serve_unknown_param_yields_serve_error() {
        let err = plan_for(
            "privacy set",
            serde_json::json!({"key": "status", "denyy": ["@x"]}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("denyy"), "{msg}");
    }

    #[test]
    fn serve_validation_usage_errors_stay_pure() {
        let err = plan_for("privacy get", serde_json::json!({"key": "shoe_size"})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("privacy set", serde_json::json!({"key": "nope"})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("privacy set", serde_json::json!({"key": "status"})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--allow"));

        let err = plan_for(
            "privacy set",
            serde_json::json!({"key": "status", "allow": []}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for(
            "privacy set",
            serde_json::json!({"key": "calls", "deny_chat": [0]}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
    }

    #[test]
    fn serve_overlap_rejection_stays_pure_through_planner() {
        for params in [
            serde_json::json!({"key": "status", "allow": ["@alice"], "deny": ["alice"]}),
            serde_json::json!({"key": "status", "allow": ["@Alice"], "deny": ["https://t.me/alice"]}),
            serde_json::json!({"key": "status", "allow_chat": [777], "deny_chat": [777]}),
        ] {
            let err = plan_for("privacy set", params).unwrap_err();
            assert_eq!(err["type"], "UsageError");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.contains("both the allow side"), "{}", msg);
        }
    }

    #[test]
    fn serve_dry_run_payloads_match_cli_shapes() {
        let plan = plan_for("privacy get", serde_json::json!({"dry_run": true})).unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({
                "dry_run": true,
                "key": null,
                "would": "get privacy rules for all keys"
            })
        );

        let plan = plan_for(
            "privacy get",
            serde_json::json!({"key": "status", "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({
                "dry_run": true,
                "key": "status",
                "would": "get privacy rules for key status"
            })
        );

        let plan = plan_for(
            "privacy set",
            serde_json::json!({
                "key": "status",
                "allow": ["@alice"],
                "allow_chat": [10, 20],
                "deny": ["@bob"],
                "dry_run": true
            }),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({
                "dry_run": true,
                "key": "status",
                "would": "set privacy rules for key status (allow 1 users + 2 chats, deny 1 users + 0 chats)"
            })
        );
    }

    #[test]
    fn serve_execute_plan_passes_raw_params_through() {
        for (op, raw) in [
            ("privacy get", serde_json::json!({})),
            ("privacy get", serde_json::json!({"key": "calls"})),
            (
                "privacy set",
                serde_json::json!({"key": "status", "allow": ["@alice"]}),
            ),
            (
                "privacy set",
                serde_json::json!({"key": "birthday", "deny_chat": [55]}),
            ),
        ] {
            let plan = plan_for(op, raw.clone()).unwrap();
            match plan {
                crate::commands::serve::Plan::Execute(passed) => {
                    assert_eq!(passed, raw, "{op}")
                }
                other => panic!("expected execute plan for {op}, got {other:?}"),
            }
        }
    }

    #[test]
    fn summary_display_matches_rule_display_for_all_variants() {
        use tl::enums::PrivacyRule as R;
        let rules: Vec<R> = vec![
            R::PrivacyValueAllowAll,
            R::PrivacyValueDisallowAll,
            R::PrivacyValueAllowContacts,
            R::PrivacyValueDisallowContacts,
            R::PrivacyValueAllowCloseFriends,
            R::PrivacyValueAllowPremium,
            R::PrivacyValueAllowBots,
            R::PrivacyValueDisallowBots,
            R::PrivacyValueAllowUsers(tl::types::PrivacyValueAllowUsers { users: vec![1, 2] }),
            R::PrivacyValueDisallowUsers(tl::types::PrivacyValueDisallowUsers { users: vec![3] }),
            R::PrivacyValueAllowChatParticipants(tl::types::PrivacyValueAllowChatParticipants {
                chats: vec![7],
            }),
            R::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![9] },
            ),
        ];
        for rule in &rules {
            assert_eq!(
                summary_display(&privacy_rule_summary(rule)),
                privacy_rule_display(rule),
                "{rule:?}"
            );
        }
    }

    #[test]
    fn set_params_schema_requires_key_and_is_closed() {
        let v = crate::commands::serve::params_schema::<SetParams>();
        assert_eq!(v["type"], "object");
        assert_eq!(v["additionalProperties"], serde_json::json!(false));
        assert_eq!(v["required"], serde_json::json!(["key"]));
        assert_eq!(v["properties"]["key"]["type"], "string");
        for field in ["allow", "allow_chat", "deny", "deny_chat", "dry_run"] {
            assert!(v["properties"][field].is_object(), "{field} missing");
        }
    }

    #[test]
    fn get_params_schema_is_closed_object() {
        let v = crate::commands::serve::params_schema::<GetParams>();
        assert_eq!(v["type"], "object");
        assert_eq!(v["additionalProperties"], serde_json::json!(false));
        for field in ["key", "dry_run"] {
            assert!(v["properties"][field].is_object(), "{field} missing");
        }
    }
}
