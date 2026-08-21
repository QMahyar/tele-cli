use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::TeleError;
use crate::error::TeleResult;
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
    #[arg(long, default_value_t = 100, help = "max contacts to list (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct AddArgs {
    #[arg(long, help = "user to add: @username, numeric ID, +phone, or me")]
    user: String,
    #[arg(long, help = "first name (defaults to peer name)")]
    first: Option<String>,
    #[arg(long, help = "last name (defaults to peer name)")]
    last: Option<String>,
    #[arg(long, help = "phone number to associate")]
    phone: Option<String>,
}

#[derive(Args)]
pub struct BlockArgs {
    #[arg(
        long,
        help = "user to block/unblock: @username, numeric ID, +phone, or me"
    )]
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
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();

        Box::pin(async move {
            if dry_run {
                return Ok(dry_run_payload("list contacts", None));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
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
                output::print_account_table(&name, multi, &["id", "name", "phone"], &table_rows)?;
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
                return Ok(dry_run_payload("add contact", Some(&user_target)));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user_target).await?;
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
            let updates: tl::enums::Updates = guard
                .client
                .invoke(&tl::functions::contacts::AddContact {
                    add_phone_privacy_exception: false,
                    id: user_input,
                    first_name: f.clone(),
                    last_name: l.clone(),
                    phone: phone.unwrap_or_default(),
                    note: None,
                })
                .await
                .map_err(tele_invocation)?;
            let Some((contact, mutual, server_name)) = returned_user_state(&updates) else {
                return Err(TeleError::Other(format!(
                    "contact not added: {user_target}'s privacy settings do not allow adding your number"
                )));
            };
            if !contact {
                return Err(TeleError::Other(format!(
                    "contact not saved to your contact list (privacy settings of {user_target})"
                )));
            }
            let sent = sent_display_name(&f, &l);
            if !sent.is_empty() && !server_name.is_empty() && server_name != sent {
                crate::output::log_line(
                    "warn",
                    "contact already existed; its display name was updated to the new values",
                );
            }
            Ok(serde_json::json!({
                "user": user_target,
                "added": true,
                "contact": contact,
                "mutual": mutual,
            }))
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
                return Ok(dry_run_payload("block", Some(&user_target)));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user_target).await?;
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
                return Ok(dry_run_payload("unblock", Some(&user_target)));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user_target).await?;
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

fn dry_run_payload(action: &str, user: Option<&str>) -> serde_json::Value {
    match user {
        Some(u) => serde_json::json!({
            "dry_run": true,
            "user": u,
            "would": format!("{action} user {u}")
        }),
        None => serde_json::json!({
            "dry_run": true,
            "would": action
        }),
    }
}

type ContactState = (bool, bool, String);

fn returned_user_state(updates: &tl::enums::Updates) -> Option<ContactState> {
    let users: &[tl::enums::User] = match updates {
        tl::enums::Updates::Updates(u) => &u.users,
        tl::enums::Updates::Combined(u) => &u.users,
        _ => return None,
    };
    let user = users.first()?;
    match user {
        tl::enums::User::User(u) => {
            let name = format!(
                "{} {}",
                u.first_name.as_deref().unwrap_or_default(),
                u.last_name.as_deref().unwrap_or_default()
            )
            .trim()
            .to_string();
            Some((u.contact, u.mutual_contact, name))
        }
        tl::enums::User::Empty(_) => None,
    }
}

fn sent_display_name(first: &str, last: &str) -> String {
    format!("{first} {last}").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact_added_updates(
        contact: bool,
        mutual: bool,
        first: &str,
        last: &str,
    ) -> tl::enums::Updates {
        tl::enums::Updates::Updates(tl::types::Updates {
            updates: Vec::new(),
            users: vec![tl::enums::User::User(tl::types::User {
                is_self: false,
                contact,
                mutual_contact: mutual,
                deleted: false,
                bot: false,
                bot_chat_history: false,
                bot_nochats: false,
                verified: false,
                restricted: false,
                min: false,
                bot_inline_geo: false,
                support: false,
                scam: false,
                apply_min_photo: false,
                fake: false,
                bot_attach_menu: false,
                premium: false,
                attach_menu_enabled: false,
                bot_can_edit: false,
                close_friend: false,
                stories_hidden: false,
                stories_unavailable: false,
                contact_require_premium: false,
                bot_business: false,
                bot_has_main_app: false,
                bot_forum_view: false,
                bot_forum_can_manage_topics: false,
                bot_can_manage_bots: false,
                bot_guestchat: false,
                bot_guard: false,
                id: 4242,
                access_hash: Some(777),
                first_name: Some(first.to_string()),
                last_name: Some(last.to_string()),
                username: None,
                phone: None,
                photo: None,
                status: None,
                bot_info_version: None,
                restriction_reason: None,
                bot_inline_placeholder: None,
                lang_code: None,
                emoji_status: None,
                usernames: None,
                stories_max_id: None,
                color: None,
                profile_color: None,
                bot_active_users: None,
                bot_verification_icon: None,
                send_paid_messages_stars: None,
            })],
            chats: Vec::new(),
            date: 0,
            seq: 0,
        })
    }
    #[test]
    fn returned_user_state_reads_contact_flags() {
        let updates = contact_added_updates(true, true, "Jane", "Doe");
        let state = returned_user_state(&updates).expect("user present");
        assert!(state.0);
        assert!(state.1);
        assert_eq!(state.2, "Jane Doe");
    }

    #[test]
    fn returned_user_state_flags_blocked_add_as_not_contact() {
        let updates = contact_added_updates(false, false, "Jane", "Doe");
        let state = returned_user_state(&updates).expect("user present");
        assert!(!state.0);
        assert!(!state.1);
    }

    #[test]
    fn returned_user_state_none_without_users_payload() {
        let combined = tl::enums::Updates::Combined(tl::types::UpdatesCombined {
            updates: Vec::new(),
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq_start: 0,
            seq: 0,
        });
        assert!(returned_user_state(&combined).is_none());
    }

    #[test]
    fn sent_display_name_trims_and_joins() {
        assert_eq!(sent_display_name("Jane", "Doe"), "Jane Doe");
        assert_eq!(sent_display_name("Jane", ""), "Jane");
        assert_eq!(sent_display_name("", ""), "");
    }

    #[test]
    fn dry_run_payload_marks_dry_run_only() {
        let v = dry_run_payload("list contacts", None);
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert!(v.get("user").is_none());
        assert_eq!(v["would"], serde_json::json!("list contacts"));
    }

    #[test]
    fn dry_run_payload_carries_user_target() {
        let v = dry_run_payload("block", Some("alice"));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["user"], serde_json::json!("alice"));
        assert_eq!(v["would"], serde_json::json!("block user alice"));
    }
}
