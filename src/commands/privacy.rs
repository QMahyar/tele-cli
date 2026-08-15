use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum PrivacyCmd {
    Get(GetArgs),
    Set(SetArgs),
}

#[derive(Args)]
pub struct GetArgs {
    #[arg(
        long,
        help = "privacy key: status, profile_photo, phone_number, calls, forwards, chat_invite, added_by_phone, voice_messages, about"
    )]
    key: Option<String>,
}

#[derive(Args)]
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
        help = "users to deny: comma-separated @username, ID, or me"
    )]
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
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let key_filter = args.key.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(dry_run_get_data(key_filter));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let mut rows = Vec::new();
            let mut table_rows = Vec::new();
            for key in keys() {
                if let Some(filter) = &key_filter {
                    if key != filter {
                        continue;
                    }
                }
                let Some(tl_key) = key_to_tl(key) else {
                    continue;
                };
                let rules = fetch_privacy_rules(&guard.client, &tl_key).await?;
                let summary = rules
                    .iter()
                    .map(privacy_rule_summary)
                    .collect::<Vec<serde_json::Value>>();
                rows.push(serde_json::json!({
                    "key": key,
                    "rules": summary,
                }));
                if !output::machine_mode(json, jsonl) {
                    for rule in &rules {
                        let (kind, peers) = privacy_rule_display(rule);
                        table_rows.push(vec![key.to_string(), kind, peers]);
                    }
                }
            }
            if !output::machine_mode(json, jsonl) {
                output::print_table(&["key", "rule", "peers"], &table_rows);
            }
            Ok(serde_json::json!({"privacy": rows}))
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
    if args.allow.is_none() && args.deny.is_none() {
        return Err(TeleError::Usage(
            "privacy set requires --allow or --deny".to_string(),
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
    Ok(key)
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let tl_key = validate_set(&args)?;
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
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "key": key_name,
                    "would": format!("set privacy rules for key {key_name}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let mut allow_users = Vec::new();
            for target in &allow {
                let peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), target)
                    .await
                    .map_err(tele_invocation)?;
                allow_users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
            }
            let mut disallow_users = Vec::new();
            for target in &deny {
                let peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), target)
                    .await
                    .map_err(tele_invocation)?;
                disallow_users.push(entities::input_user(&peer).await.map_err(tele_invocation)?);
            }
            let base = fetch_privacy_rules(&guard.client, &tl_key).await?;
            let rules = merge_privacy_rules(&base, &allow_users, &disallow_users);
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

fn input_user_from_id(user_id: i64) -> tl::enums::InputUser {
    tl::enums::InputUser::User(tl::types::InputUser {
        user_id,
        access_hash: 0,
    })
}

fn merge_privacy_rules(
    base: &[tl::enums::PrivacyRule],
    allow_users: &[tl::enums::InputUser],
    disallow_users: &[tl::enums::InputUser],
) -> Vec<tl::enums::InputPrivacyRule> {
    let mut merged = Vec::with_capacity(base.len() + 2);
    for rule in base {
        match rule {
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(_) if !allow_users.is_empty() => {}
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(_) if !disallow_users.is_empty() => {}
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(v) => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers {
                        users: v.users.iter().map(|id| input_user_from_id(*id)).collect(),
                    },
                ));
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowUsers(v) => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers {
                        users: v.users.iter().map(|id| input_user_from_id(*id)).collect(),
                    },
                ));
            }
            tl::enums::PrivacyRule::PrivacyValueAllowAll => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowAll => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowContacts => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts);
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowContacts => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowContacts);
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
            tl::enums::PrivacyRule::PrivacyValueDisallowBots => {
                merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowBots);
            }
            tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(v) => {
                merged.push(
                    tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                        tl::types::InputPrivacyValueAllowChatParticipants {
                            chats: v.chats.clone(),
                        },
                    ),
                );
            }
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(v) => {
                merged.push(
                    tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(
                        tl::types::InputPrivacyValueDisallowChatParticipants {
                            chats: v.chats.clone(),
                        },
                    ),
                );
            }
        }
    }
    if !allow_users.is_empty() {
        merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
            tl::types::InputPrivacyValueAllowUsers {
                users: allow_users.to_vec(),
            },
        ));
    }
    if !disallow_users.is_empty() {
        merged.push(tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
            tl::types::InputPrivacyValueDisallowUsers {
                users: disallow_users.to_vec(),
            },
        ));
    }
    merged
}

fn privacy_rule_display(rule: &tl::enums::PrivacyRule) -> (String, String) {
    match rule {
        tl::enums::PrivacyRule::PrivacyValueAllowAll => ("allow_all".to_string(), String::new()),
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
        tl::enums::PrivacyRule::PrivacyValueAllowBots => ("allow_bots".to_string(), String::new()),
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
                deny,
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
        };
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_rejects_all_blank_deny() {
        let args = SetArgs {
            key: "status".to_string(),
            allow: None,
            deny: Some(vec!["  ".to_string(), "\t".to_string()]),
        };
        assert!(matches!(validate_set(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_accepts_allow_and_deny_together() {
        let args = SetArgs {
            key: "status".to_string(),
            allow: Some(vec!["@alice".to_string()]),
            deny: Some(vec!["@bob".to_string()]),
        };
        assert!(validate_set(&args).is_ok());
    }

    #[test]
    fn set_absent_allow_unchanged() {
        let with_deny = SetArgs {
            key: "calls".to_string(),
            allow: None,
            deny: Some(vec!["@x".to_string()]),
        };
        assert!(validate_set(&with_deny).is_ok());
        let neither = SetArgs {
            key: "calls".to_string(),
            allow: None,
            deny: None,
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
    fn merge_keeps_non_user_rules_and_replaces_only_allow() {
        let base = base_with_contacts_and_user_rules();
        let merged = merge_privacy_rules(&base, &[iu(5)], &[]);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(3)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
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
        let merged = merge_privacy_rules(&base, &[], &[iu(4)]);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll,
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(1)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(4)] },
                ),
            ]
        );
    }

    #[test]
    fn merge_replaces_both_user_rules_when_both_given() {
        let base = base_with_contacts_and_user_rules();
        let merged = merge_privacy_rules(&base, &[iu(5)], &[iu(6)]);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts,
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueDisallowUsers(
                    tl::types::InputPrivacyValueDisallowUsers { users: vec![iu(6)] },
                ),
            ]
        );
    }

    #[test]
    fn merge_keeps_chat_rules() {
        let base = vec![tl::enums::PrivacyRule::PrivacyValueAllowChatParticipants(
            tl::types::PrivacyValueAllowChatParticipants { chats: vec![777] },
        )];
        let merged = merge_privacy_rules(&base, &[iu(5)], &[]);
        assert_eq!(
            merged,
            vec![
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowChatParticipants(
                    tl::types::InputPrivacyValueAllowChatParticipants { chats: vec![777] },
                ),
                tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(
                    tl::types::InputPrivacyValueAllowUsers { users: vec![iu(5)] },
                ),
            ]
        );
    }

    #[test]
    fn merge_empty_base_yields_only_new_rules() {
        let merged = merge_privacy_rules(&[], &[iu(5)], &[iu(6)]);
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
        assert!(merge_privacy_rules(&[], &[], &[]).is_empty());
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
}
