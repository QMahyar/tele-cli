use clap::{Args, Subcommand};
use grammers_client::tl;
use grammers_session::types::PeerInfo;
use grammers_session::Session;
use std::collections::HashMap;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::helpers::{peer_id, stats_abs, stats_percent, stats_period};
use crate::commands::require_chat_target;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum ChatCmd {
    Join(ChatArgs),
    Leave(ChatArgs),
    Invite(InviteArgs),
    Participants(ParticipantsArgs),
    Kick(KickArgs),
    Admin(AdminArgs),
    AdminLog(AdminLogArgs),
    Stats(StatsArgs),
    Create(CreateArgs),
}

#[derive(Args)]
pub struct ChatArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
}

#[derive(Args)]
pub struct InviteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to invite: @username, t.me link, numeric ID, or phone"
    )]
    user: String,
}

#[derive(Args)]
pub struct ParticipantsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(
        long,
        default_value_t = 100,
        help = "max participants to return (1-10000)"
    )]
    limit: u32,
}

#[derive(Args)]
pub struct KickArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to kick: @username, t.me link, numeric ID, or phone"
    )]
    user: String,
}

#[derive(Args)]
pub struct AdminArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to promote or demote: @username, t.me link, numeric ID, or phone"
    )]
    user: String,
    #[arg(long, help = "grant admin rights (mutually exclusive with --demote)")]
    promote: bool,
    #[arg(long, help = "revoke admin rights (mutually exclusive with --promote)")]
    demote: bool,
    #[arg(long, help = "admin rank title (e.g. Mod, Admin)")]
    title: Option<String>,
    #[arg(
        long,
        help = "preset rights: moderator, editor, admin (mutually exclusive with --rights)"
    )]
    preset: Option<String>,
    #[arg(
        long,
        help = "comma-separated rights: change_info,post,edit,delete,ban,invite,pin,add_admins,manage_call (mutually exclusive with --preset)"
    )]
    rights: Option<String>,
}

#[derive(Args)]
pub struct AdminLogArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(long, default_value_t = 20, help = "max events to return (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct StatsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, or invite link"
    )]
    chat: String,
    #[arg(long, help = "use broadcast channel stats (default: megagroup stats)")]
    broadcast: bool,
}

#[derive(Args)]
pub struct CreateArgs {
    #[arg(long, help = "chat title")]
    title: String,
    #[arg(long, help = "chat description (for supergroups and channels)")]
    description: Option<String>,
    #[arg(
        long,
        default_value = "group",
        help = "chat type: group, supergroup, or channel"
    )]
    kind: String,
    #[arg(long, help = "enable forum topics (supergroups only)")]
    forum: bool,
}

pub async fn run(cmd: ChatCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ChatCmd::Join(a) => join(a, flags).await,
        ChatCmd::Leave(a) => leave(a, flags).await,
        ChatCmd::Invite(a) => invite(a, flags).await,
        ChatCmd::Participants(a) => participants(a, flags).await,
        ChatCmd::Kick(a) => kick(a, flags).await,
        ChatCmd::Admin(a) => admin(a, flags).await,
        ChatCmd::AdminLog(a) => admin_log(a, flags).await,
        ChatCmd::Stats(a) => stats(a, flags).await,
        ChatCmd::Create(a) => create(a, flags).await,
    }
}

async fn join(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("join chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let normalized = normalize_invite_link(&target);
            if validate_invite_link(&normalized).is_ok() {
                let joined = guard
                    .client
                    .accept_invite_link(&normalized)
                    .await
                    .map_err(tele_invocation)?;
                if let Some(peer) = joined {
                    cache_joined_chat(guard.session.as_ref(), &peer).await;
                }
            } else {
                let peer =
                    entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
                let chat_ref = entities::peer_ref(&peer).await.map_err(tele_invocation)?;
                let joined = guard
                    .client
                    .join_chat(chat_ref)
                    .await
                    .map_err(tele_invocation)?;
                if let Some(peer) = joined {
                    cache_joined_chat(guard.session.as_ref(), &peer).await;
                }
            }
            Ok(serde_json::json!({"chat": target, "joined": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_invite_link(input: &str) -> TeleResult<()> {
    if grammers_client::Client::parse_invite_link(input).is_some() || is_bare_invite_hash(input) {
        return Ok(());
    }
    Err(TeleError::Usage(format!(
        "not a valid invite link or chat target: \"{input}\""
    )))
}

fn normalize_invite_link(input: &str) -> String {
    let t = input.trim();
    if t.starts_with("t.me/") || t.starts_with("telegram.me/") {
        format!("https://{t}")
    } else {
        t.to_string()
    }
}

fn is_bare_invite_hash(input: &str) -> bool {
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return false;
    }
    if input.contains('/') || input.contains(':') || input.contains('@') {
        return false;
    }
    let rest = input.strip_prefix('+').unwrap_or(input);
    if rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    input.starts_with('+') || input.contains('_') || input.contains('-')
}

async fn leave(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("leave chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            match &peer {
                grammers_client::peer::Peer::Channel(_) => {
                    let channel = entities::input_channel(&peer)
                        .await
                        .map_err(tele_invocation)?;
                    guard
                        .client
                        .invoke(&tl::functions::channels::LeaveChannel { channel })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::Group(_) if entities::is_channel(&peer) => {
                    let channel = entities::input_channel(&peer)
                        .await
                        .map_err(tele_invocation)?;
                    guard
                        .client
                        .invoke(&tl::functions::channels::LeaveChannel { channel })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::Group(_) => {
                    let user_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
                    guard
                        .client
                        .invoke(&tl::functions::messages::DeleteChatUser {
                            chat_id: peer.id().bare_id().unwrap_or_default(),
                            user_id,
                            revoke_history: false,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::User(_) => {
                    return Err(TeleError::Usage(
                        "cannot leave a private chat; use tele dialog delete".to_string(),
                    ));
                }
            }
            Ok(serde_json::json!({"chat": target, "left": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn invite(args: InviteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "user": user,
                    "would": format!("invite user {user} to chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let user_peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user).await?;
            let user_input = entities::input_user(&user_peer)
                .await
                .map_err(tele_invocation)?;
            match &chat {
                grammers_client::peer::Peer::Channel(_) => {
                    let channel = entities::input_channel(&chat)
                        .await
                        .map_err(tele_invocation)?;
                    guard
                        .client
                        .invoke(&tl::functions::channels::InviteToChannel {
                            channel,
                            users: vec![user_input],
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::Group(_) if entities::is_channel(&chat) => {
                    let channel = entities::input_channel(&chat)
                        .await
                        .map_err(tele_invocation)?;
                    guard
                        .client
                        .invoke(&tl::functions::channels::InviteToChannel {
                            channel,
                            users: vec![user_input],
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::Group(_) => {
                    guard
                        .client
                        .invoke(&tl::functions::messages::AddChatUser {
                            chat_id: chat.id().bare_id().unwrap_or_default(),
                            user_id: user_input,
                            fwd_limit: 0,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::User(_) => {
                    return Err(TeleError::Usage(
                        "cannot invite into a private chat".to_string(),
                    ));
                }
            }
            Ok(serde_json::json!({"chat": target, "user": user, "invited": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn participants(args: ParticipantsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("list participants of chat {target}")
                }));
            }
            let guard = ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await?;
            ensure_chat_peer(&chat, "participants")?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut rows = Vec::new();
            if matches!(&chat, grammers_client::peer::Peer::Group(_))
                && !entities::is_channel(&chat)
            {
                let full = guard
                    .client
                    .invoke(&tl::functions::messages::GetFullChat {
                        chat_id: chat.id().bare_id().unwrap_or_default(),
                    })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::messages::ChatFull::Full(full) = full;
                let users: HashMap<i64, tl::enums::User> =
                    full.users.into_iter().map(|u| (u.id(), u)).collect();
                let participants = match full.full_chat {
                    tl::enums::ChatFull::Full(f) => match f.participants {
                        tl::enums::ChatParticipants::Participants(p) => p.participants,
                        tl::enums::ChatParticipants::Forbidden(_) => {
                            return Err(TeleError::Other(
                                "participants unavailable for this chat".to_string(),
                            ));
                        }
                    },
                    tl::enums::ChatFull::ChannelFull(_) => {
                        return Err(TeleError::Other(
                            "participants unavailable for this chat".to_string(),
                        ));
                    }
                };
                let (mut basic_rows, missing) = participant_rows(&guard.client, &users, &participants);
                basic_rows.truncate(limit as usize);
                if missing > 0 {
                    output::log_line(
                        "warn",
                        &format!("{missing} participant(s) missing user data were skipped"),
                    );
                }
                rows = basic_rows;
            } else {
                let mut count = 0u32;
                let mut iter = guard.client.iter_participants(chat_ref);
                while count < limit {
                    match iter.next().await.map_err(tele_invocation)? {
                        Some(p) => {
                            rows.push(serde_json::json!({
                                "id": p.user.id().bare_id().unwrap_or_default(),
                                "name": crate::serialize::peer_name(&grammers_client::peer::Peer::User(p.user)),
                                "role": role_name(&p.role),
                            }));
                            count += 1;
                        }
                        None => break,
                    }
                }
            }
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| vec![
                        r["id"].to_string(),
                        r["name"].as_str().unwrap_or_default().to_string(),
                        r["role"].as_str().unwrap_or_default().to_string(),
                    ])
                    .collect();
                output::print_table(&["id", "name", "role"], &table_rows);
            }
            Ok(serde_json::json!({"participants": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn kick(args: KickArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "user": user,
                    "would": format!("kick user {user} from chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "kick")?;
            let user_peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user).await?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let user_ref = entities::peer_ref(&user_peer)
                .await
                .map_err(tele_invocation)?;
            guard
                .client
                .kick_participant(chat_ref, user_ref)
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"chat": target, "user": user, "kicked": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_admin(args: &AdminArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    if args.promote && args.demote {
        return Err(TeleError::Usage(
            "--promote and --demote are mutually exclusive".to_string(),
        ));
    }
    if !args.promote && !args.demote {
        return Err(TeleError::Usage(
            "--promote or --demote required".to_string(),
        ));
    }
    if args.preset.is_some() && args.rights.is_some() {
        return Err(TeleError::Usage(
            "--preset and --rights are mutually exclusive".to_string(),
        ));
    }
    if let Some(preset) = &args.preset {
        if preset != "moderator" && preset != "editor" && preset != "admin" {
            return Err(TeleError::Usage(format!(
                "unknown preset '{}': use moderator, editor, or admin",
                preset
            )));
        }
    }
    Ok(())
}

struct AdminRights {
    change_info: bool,
    post_messages: bool,
    edit_messages: bool,
    delete_messages: bool,
    ban_users: bool,
    invite_users: bool,
    pin_messages: bool,
    add_admins: bool,
    manage_call: bool,
}

impl AdminRights {
    fn all() -> Self {
        Self {
            change_info: true,
            post_messages: true,
            edit_messages: true,
            delete_messages: true,
            ban_users: true,
            invite_users: true,
            pin_messages: true,
            add_admins: true,
            manage_call: true,
        }
    }

    fn moderator() -> Self {
        Self {
            change_info: false,
            post_messages: false,
            edit_messages: false,
            delete_messages: true,
            ban_users: true,
            invite_users: true,
            pin_messages: true,
            add_admins: false,
            manage_call: false,
        }
    }

    fn editor() -> Self {
        Self {
            change_info: true,
            post_messages: true,
            edit_messages: true,
            delete_messages: true,
            ban_users: false,
            invite_users: true,
            pin_messages: true,
            add_admins: false,
            manage_call: false,
        }
    }

    fn from_string(s: &str) -> TeleResult<Self> {
        let mut rights = Self {
            change_info: false,
            post_messages: false,
            edit_messages: false,
            delete_messages: false,
            ban_users: false,
            invite_users: false,
            pin_messages: false,
            add_admins: false,
            manage_call: false,
        };
        for part in s.split(',') {
            let part = part.trim();
            match part {
                "change_info" => rights.change_info = true,
                "post" => rights.post_messages = true,
                "edit" => rights.edit_messages = true,
                "delete" => rights.delete_messages = true,
                "ban" => rights.ban_users = true,
                "invite" => rights.invite_users = true,
                "pin" => rights.pin_messages = true,
                "add_admins" => rights.add_admins = true,
                "manage_call" => rights.manage_call = true,
                "" => {}
                _ => {
                    return Err(TeleError::Usage(format!(
                        "unknown right '{}': use change_info,post,edit,delete,ban,invite,pin,add_admins,manage_call",
                        part
                    )))
                }
            }
        }
        Ok(rights)
    }
}

fn resolve_admin_rights(args: &AdminArgs) -> TeleResult<AdminRights> {
    if args.demote {
        return Ok(AdminRights {
            change_info: false,
            post_messages: false,
            edit_messages: false,
            delete_messages: false,
            ban_users: false,
            invite_users: false,
            pin_messages: false,
            add_admins: false,
            manage_call: false,
        });
    }
    if let Some(preset) = &args.preset {
        return Ok(match preset.as_str() {
            "moderator" => AdminRights::moderator(),
            "editor" => AdminRights::editor(),
            "admin" => AdminRights::all(),
            _ => AdminRights::all(),
        });
    }
    if let Some(rights_str) = &args.rights {
        return AdminRights::from_string(rights_str);
    }
    Ok(AdminRights::all())
}

async fn admin(args: AdminArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_admin(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let promote = args.promote;
    let demote = args.demote;
    let rights = resolve_admin_rights(&args)?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        let title = args.title.clone();
        let rights = AdminRights {
            change_info: rights.change_info,
            post_messages: rights.post_messages,
            edit_messages: rights.edit_messages,
            delete_messages: rights.delete_messages,
            ban_users: rights.ban_users,
            invite_users: rights.invite_users,
            pin_messages: rights.pin_messages,
            add_admins: rights.add_admins,
            manage_call: rights.manage_call,
        };
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "user": user,
                    "promote": promote,
                    "demote": demote,
                    "would": format!("change admin status of user {user} in chat {target}"),
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let user_peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &user).await?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let user_ref = entities::peer_ref(&user_peer)
                .await
                .map_err(tele_invocation)?;
            let mut builder = guard.client.set_admin_rights(chat_ref, user_ref);
            builder = builder
                .change_info(rights.change_info)
                .post_messages(rights.post_messages)
                .edit_messages(rights.edit_messages)
                .delete_messages(rights.delete_messages)
                .ban_users(rights.ban_users)
                .invite_users(rights.invite_users)
                .pin_messages(rights.pin_messages)
                .add_admins(rights.add_admins)
                .manage_call(rights.manage_call);
            if let Some(t) = &title {
                builder = builder.rank(t.clone());
            }
            builder.await.map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "user": user,
                "promote": promote,
                "demote": demote,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn admin_log(args: AdminLogArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("list admin log of chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let own_id = guard
                .client
                .get_me()
                .await
                .map_err(tele_invocation)?
                .id()
                .bare_id()
                .unwrap_or_default();
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let channel = entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let events = {
                let guard_ref = &guard;
                let channel_ref = &channel;
                collect_admin_log(limit, move |max_id, page_limit| async move {
                    let raw: tl::enums::channels::AdminLogResults = guard_ref
                        .client
                        .invoke(&tl::functions::channels::GetAdminLog {
                            channel: (*channel_ref).clone(),
                            q: String::new(),
                            events_filter: None,
                            admins: None,
                            max_id,
                            min_id: 0,
                            limit: page_limit as i32,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    let tl::enums::channels::AdminLogResults::Results(results) = raw;
                    let next_max_id = results
                        .events
                        .iter()
                        .last()
                        .map(|e| match e {
                            tl::enums::ChannelAdminLogEvent::Event(e) => e.id,
                        })
                        .unwrap_or_default();
                    Ok(AdminLogPage {
                        events: results.events,
                        max_id: next_max_id,
                    })
                })
            }
            .await?;
            let mut rows = Vec::new();
            for event in events {
                let tl::enums::ChannelAdminLogEvent::Event(event) = event;
                let date = chrono::DateTime::from_timestamp(event.date as i64, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
                rows.push(serde_json::json!({
                    "id": event.id,
                    "date": date,
                    "action": admin_action_summary(&event.action, own_id),
                }));
            }
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["date"].as_str().unwrap_or_default().to_string(),
                            admin_action_display(&r["action"]),
                        ]
                    })
                    .collect();
                output::print_table(&["id", "date", "action"], &table_rows);
            }
            Ok(serde_json::json!({"events": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

struct AdminLogPage {
    events: Vec<tl::enums::ChannelAdminLogEvent>,
    max_id: i64,
}

async fn collect_admin_log<F, Fut>(
    limit: u32,
    mut fetch: F,
) -> TeleResult<Vec<tl::enums::ChannelAdminLogEvent>>
where
    F: FnMut(i64, u32) -> Fut,
    Fut: std::future::Future<Output = TeleResult<AdminLogPage>>,
{
    let mut events = Vec::new();
    let mut max_id = 0i64;
    loop {
        let remaining = limit.saturating_sub(events.len() as u32);
        if remaining == 0 {
            break;
        }
        let page = fetch(max_id, remaining.min(100)).await?;
        max_id = page.max_id;
        let page_len = page.events.len();
        events.extend(page.events);
        if page_len == 0 {
            break;
        }
    }
    Ok(events)
}

fn stats_dry_run_payload(chat: &str, broadcast: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "broadcast": broadcast,
        "would": format!("show stats of chat {chat}"),
    })
}

async fn stats(args: StatsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let broadcast = args.broadcast;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(stats_dry_run_payload(&target, broadcast));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let channel = entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let raw = if broadcast {
                let r: tl::enums::stats::BroadcastStats = guard
                    .client
                    .invoke(&tl::functions::stats::GetBroadcastStats {
                        channel,
                        dark: false,
                    })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::stats::BroadcastStats::Stats(r) = r;
                serde_json::json!({
                    "period": stats_period(&r.period),
                    "followers": stats_abs(&r.followers),
                    "views_per_post": stats_abs(&r.views_per_post),
                    "shares_per_post": stats_abs(&r.shares_per_post),
                    "reactions_per_post": stats_abs(&r.reactions_per_post),
                    "enabled_notifications": stats_percent(&r.enabled_notifications),
                    "recent_posts_interactions": r.recent_posts_interactions.len(),
                })
            } else {
                let r: tl::enums::stats::MegagroupStats = guard
                    .client
                    .invoke(&tl::functions::stats::GetMegagroupStats {
                        channel,
                        dark: false,
                    })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::stats::MegagroupStats::Stats(r) = r;
                serde_json::json!({
                    "period": stats_period(&r.period),
                    "members": stats_abs(&r.members),
                    "messages": stats_abs(&r.messages),
                    "viewers": stats_abs(&r.viewers),
                    "posters": stats_abs(&r.posters),
                    "top_posters": r.top_posters.len(),
                    "top_admins": r.top_admins.len(),
                    "top_inviters": r.top_inviters.len(),
                })
            };
            Ok(serde_json::json!({"chat": target, "stats": raw}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_create(args: &CreateArgs) -> TeleResult<()> {
    match args.kind.as_str() {
        "group" | "supergroup" | "channel" => Ok(()),
        other => Err(TeleError::Usage(format!(
            "unknown chat kind {other} (use group, supergroup or channel)"
        ))),
    }
}

async fn create(args: CreateArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_create(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let forum = args.forum;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let title = args.title.clone();
        let description = args.description.clone();
        let kind = args.kind.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "title": title,
                    "would": format!("create {kind} chat \"{title}\"")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let result = match kind.as_str() {
                "group" => {
                    let r: tl::enums::messages::InvitedUsers = guard
                        .client
                        .invoke(&tl::functions::messages::CreateChat {
                            users: Vec::new(),
                            title,
                            ttl_period: None,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    let tl::enums::messages::InvitedUsers::Users(r) = r;
                    let chat = created_chat(&r.updates);
                    let chat_id = chat.map(|c| c.id()).unwrap_or(0);
                    cache_created_chat(&guard, chat).await;
                    serde_json::json!({"kind": "group", "chat_id": chat_id})
                }
                "supergroup" => {
                    let r: tl::enums::Updates = guard
                        .client
                        .invoke(&tl::functions::channels::CreateChannel {
                            broadcast: false,
                            megagroup: true,
                            for_import: false,
                            forum,
                            title,
                            about: description.unwrap_or_default(),
                            geo_point: None,
                            address: None,
                            ttl_period: None,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    let chat = created_chat(&r);
                    let chat_id = chat.map(|c| c.id()).unwrap_or(0);
                    cache_created_chat(&guard, chat).await;
                    serde_json::json!({"kind": "supergroup", "forum": forum, "chat_id": chat_id})
                }
                "channel" => {
                    let r: tl::enums::Updates = guard
                        .client
                        .invoke(&tl::functions::channels::CreateChannel {
                            broadcast: true,
                            megagroup: false,
                            for_import: false,
                            forum: false,
                            title,
                            about: description.unwrap_or_default(),
                            geo_point: None,
                            address: None,
                            ttl_period: None,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    let chat = created_chat(&r);
                    let chat_id = chat.map(|c| c.id()).unwrap_or(0);
                    cache_created_chat(&guard, chat).await;
                    serde_json::json!({"kind": "channel", "chat_id": chat_id})
                }
                other => {
                    return Err(TeleError::Usage(format!(
                        "unknown chat kind {other} (use group, supergroup or channel)"
                    )));
                }
            };
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn created_chat(r: &tl::enums::Updates) -> Option<&tl::enums::Chat> {
    match r {
        tl::enums::Updates::Updates(u) => u.chats.first(),
        _ => None,
    }
}

async fn cache_created_chat(guard: &client::ClientGuard, chat: Option<&tl::enums::Chat>) {
    if let Some(chat) = chat {
        if let Err(e) = entities::cache_chat(guard.session.as_ref(), chat).await {
            log::warn!(
                "failed to cache access_hash for created chat {}: {e}",
                chat.id()
            );
        }
    }
}

async fn cache_joined_chat<S: Session>(session: &S, peer: &grammers_client::peer::Peer)
where
    S::Error: std::fmt::Display,
{
    if let Err(e) = session.cache_peer(&PeerInfo::from(peer)).await {
        log::warn!(
            "failed to cache access_hash for joined chat {}: {e}",
            peer.id()
        );
    }
}

fn participant_user_id(p: &tl::enums::ChannelParticipant, own_id: i64) -> i64 {
    match p {
        tl::enums::ChannelParticipant::Participant(p) => p.user_id,
        tl::enums::ChannelParticipant::ParticipantSelf(_) => own_id,
        tl::enums::ChannelParticipant::Creator(p) => p.user_id,
        tl::enums::ChannelParticipant::Admin(p) => p.user_id,
        tl::enums::ChannelParticipant::Banned(p) => peer_id(&p.peer),
        tl::enums::ChannelParticipant::Left(p) => peer_id(&p.peer),
    }
}

fn role_name(role: &grammers_client::peer::Role) -> &'static str {
    match role {
        grammers_client::peer::Role::Creator(_) => "creator",
        grammers_client::peer::Role::Admin(_) => "admin",
        grammers_client::peer::Role::Banned(_) => "banned",
        grammers_client::peer::Role::User(_) => "member",
        grammers_client::peer::Role::Left(_) => "left",
        _ => "unknown",
    }
}

fn participant_rows(
    client: &grammers_client::Client,
    users: &HashMap<i64, tl::enums::User>,
    participants: &[tl::enums::ChatParticipant],
) -> (Vec<serde_json::Value>, usize) {
    let mut rows = Vec::new();
    let mut missing = 0usize;
    for participant in participants {
        let role = match participant {
            tl::enums::ChatParticipant::Creator(_) => "creator",
            tl::enums::ChatParticipant::Admin(_) => "admin",
            tl::enums::ChatParticipant::Participant(_) => "member",
        };
        match users.get(&participant.user_id()) {
            Some(user) => rows.push(serde_json::json!({
                "id": user.id(),
                "name": crate::serialize::peer_name(&grammers_client::peer::Peer::User(
                    grammers_client::peer::User::from_raw(client, user.clone()),
                )),
                "role": role,
            })),
            None => missing += 1,
        }
    }
    (rows, missing)
}

fn ensure_chat_peer(peer: &grammers_client::peer::Peer, action: &str) -> TeleResult<()> {
    if matches!(peer, grammers_client::peer::Peer::User(_)) {
        return Err(TeleError::Usage(format!(
            "{action} requires a chat, got a user"
        )));
    }
    Ok(())
}

fn admin_action_summary(
    a: &tl::enums::ChannelAdminLogEventAction,
    own_id: i64,
) -> serde_json::Value {
    match a {
        tl::enums::ChannelAdminLogEventAction::ChangeTitle(v) => {
            serde_json::json!({"kind": "change_title", "title": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ChangeAbout(v) => {
            serde_json::json!({"kind": "change_about", "text": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ChangeUsername(v) => {
            serde_json::json!({"kind": "change_username", "username": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::SendMessage(v) => {
            message_action_summary("send_message", &v.message)
        }
        tl::enums::ChannelAdminLogEventAction::EditMessage(v) => {
            message_action_summary("edit_message", &v.new_message)
        }
        tl::enums::ChannelAdminLogEventAction::DeleteMessage(v) => match &v.message {
            tl::enums::Message::Message(m) => {
                serde_json::json!({"kind": "delete_message", "id": m.id})
            }
            _ => serde_json::json!({"kind": "delete_message"}),
        },
        tl::enums::ChannelAdminLogEventAction::ParticipantJoin => {
            serde_json::json!({"kind": "participant_join"})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantLeave => {
            serde_json::json!({"kind": "participant_leave"})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantInvite(v) => {
            serde_json::json!({
                "kind": "participant_invite",
                "user_id": participant_user_id(&v.participant, own_id),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantToggleBan(v) => {
            serde_json::json!({
                "kind": "toggle_ban",
                "user_id": participant_user_id(&v.new_participant, own_id),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantToggleAdmin(v) => {
            serde_json::json!({
                "kind": "toggle_admin",
                "user_id": participant_user_id(&v.new_participant, own_id),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ChangePhoto(_) => {
            serde_json::json!({"kind": "change_photo"})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleInvites(v) => {
            serde_json::json!({"kind": "toggle_invites", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleSignatures(v) => {
            serde_json::json!({"kind": "toggle_signatures", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::UpdatePinned(_) => {
            serde_json::json!({"kind": "update_pinned"})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantJoinByInvite(_) => {
            serde_json::json!({"kind": "join_by_invite"})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantJoinByRequest(v) => {
            serde_json::json!({"kind": "join_by_request", "approved_by": v.approved_by})
        }
        _ => serde_json::json!({"kind": "other"}),
    }
}

fn message_action_summary(kind: &str, message: &tl::enums::Message) -> serde_json::Value {
    match message {
        tl::enums::Message::Message(m) => {
            serde_json::json!({"kind": kind, "id": m.id, "text": m.message})
        }
        _ => serde_json::json!({"kind": kind}),
    }
}

fn admin_action_display(action: &serde_json::Value) -> String {
    let kind = action["kind"].as_str().unwrap_or("other");
    let detail = action
        .get("title")
        .or_else(|| action.get("text"))
        .or_else(|| action.get("username"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let text = if detail.is_empty() {
        kind.to_string()
    } else {
        format!("{kind}: {detail}")
    };
    if text.len() > 60 {
        format!("{}...", text.chars().take(57).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::{Client, SenderPool};
    use grammers_session::storages::MemorySession;
    use grammers_session::types::PeerId;
    use std::sync::Arc;

    #[test]
    fn stats_dry_run_carries_argument_keys() {
        let value = stats_dry_run_payload("@x", true);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["chat"], serde_json::json!("@x"));
        assert_eq!(value["broadcast"], serde_json::json!(true));
        assert_eq!(value["would"], serde_json::json!("show stats of chat @x"));
        assert_eq!(
            stats_dry_run_payload("@x", false)["broadcast"],
            serde_json::json!(false)
        );
    }

    #[tokio::test]
    async fn cache_joined_chat_stores_channel_access_hash() {
        let session = Arc::new(MemorySession::default());
        let pool = SenderPool::new(Arc::clone(&session), 0);
        let client = Client::new(pool.handle);
        let chat = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 123456,
            access_hash: 987654321,
            title: "t".to_string(),
            until_date: None,
        });
        let peer = grammers_client::peer::Peer::from_raw(&client, chat);
        cache_joined_chat(session.as_ref(), &peer).await;
        let pref = session
            .peer_ref(PeerId::channel_unchecked(123456))
            .await
            .unwrap()
            .expect("joined chat must be cached");
        assert_eq!(pref.auth.hash(), 987654321);
    }

    #[tokio::test]
    async fn cache_joined_chat_stores_basic_group() {
        let session = Arc::new(MemorySession::default());
        let pool = SenderPool::new(Arc::clone(&session), 0);
        let client = Client::new(pool.handle);
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        let peer = grammers_client::peer::Peer::from_raw(&client, chat);
        cache_joined_chat(session.as_ref(), &peer).await;
        assert!(session
            .peer_ref(PeerId::chat_unchecked(123))
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn validate_invite_link_accepts_full_invite_urls() {
        for input in [
            "https://t.me/+abc",
            "https://t.me/joinchat/abc",
            "http://telegram.me/+abc",
            "https://t.me/+abc?start=1",
        ] {
            assert!(validate_invite_link(input).is_ok(), "for {input}");
        }
    }

    #[test]
    fn validate_invite_link_accepts_bare_hashes() {
        for input in ["+abc", "+abc-xyz_123", "abc_def-123"] {
            assert!(validate_invite_link(input).is_ok(), "for {input}");
        }
    }

    #[test]
    fn validate_invite_link_rejects_garbage_and_chat_targets() {
        for input in [
            "t.me/+abc",
            "joinchat/abc",
            "not a link",
            "@telegram",
            "12345",
            "me",
            "+9891234567",
            "https://t.me/somepublic",
            "",
        ] {
            let err = validate_invite_link(input).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {input}");
        }
    }

    #[test]
    fn normalize_invite_link_prepends_scheme() {
        assert_eq!(
            normalize_invite_link("t.me/+abc123"),
            "https://t.me/+abc123"
        );
        assert_eq!(
            normalize_invite_link("t.me/joinchat/hash"),
            "https://t.me/joinchat/hash"
        );
        assert_eq!(
            normalize_invite_link("telegram.me/+abc"),
            "https://telegram.me/+abc"
        );
        assert_eq!(
            normalize_invite_link("https://t.me/+abc123"),
            "https://t.me/+abc123"
        );
        assert_eq!(normalize_invite_link("+abc123"), "+abc123");
        assert_eq!(
            normalize_invite_link("  t.me/+abc123  "),
            "https://t.me/+abc123"
        );
    }

    #[test]
    fn validate_invite_link_accepts_normalized_t_me_forms() {
        assert!(validate_invite_link("https://t.me/+abc123").is_ok());
    }

    #[test]
    fn is_bare_invite_hash_rejects_slashed_forms() {
        assert!(!is_bare_invite_hash("t.me/+x"));
        assert!(!is_bare_invite_hash("t.me/joinchat/hash"));
    }

    fn fake_event(id: i64) -> tl::enums::ChannelAdminLogEvent {
        tl::enums::ChannelAdminLogEvent::Event(tl::types::ChannelAdminLogEvent {
            id,
            date: 0,
            user_id: 0,
            action: tl::enums::ChannelAdminLogEventAction::ParticipantJoin,
        })
    }

    #[tokio::test]
    async fn collect_admin_log_stops_on_empty_page() {
        let mut calls = Vec::new();
        let events = collect_admin_log(10, |max_id, page_limit| {
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: Vec::new(),
                    max_id: 0,
                })
            }
        })
        .await
        .unwrap();
        assert!(events.is_empty());
        assert_eq!(calls, vec![(0, 10)]);
    }

    #[tokio::test]
    async fn collect_admin_log_probes_after_partial_page() {
        let pages: [(i64, u32, Vec<i64>, i64); 2] = [(0, 5, vec![10, 9], 9), (9, 3, Vec::new(), 0)];
        let mut next = 0usize;
        let mut calls = Vec::new();
        let events = collect_admin_log(5, |max_id, page_limit| {
            let (want_max, want_limit, ids, new_max) = pages[next].clone();
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                assert_eq!(max_id, want_max);
                assert_eq!(page_limit, want_limit);
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(calls, vec![(0, 5), (9, 3)]);
    }

    #[tokio::test]
    async fn collect_admin_log_paginates_until_limit() {
        let pages: [(i64, u32, Vec<i64>, i64); 2] =
            [(0, 5, vec![10, 9, 8], 8), (8, 2, vec![7, 6], 6)];
        let mut next = 0usize;
        let mut calls = Vec::new();
        let events = collect_admin_log(5, |max_id, page_limit| {
            let (want_max, want_limit, ids, new_max) = pages[next].clone();
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                assert_eq!(max_id, want_max);
                assert_eq!(page_limit, want_limit);
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(calls, vec![(0, 5), (8, 2)]);
    }

    #[tokio::test]
    async fn collect_admin_log_stops_when_limit_reached_exactly() {
        let mut calls = Vec::new();
        let events = collect_admin_log(3, |max_id, page_limit| {
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: vec![fake_event(7), fake_event(6), fake_event(5)],
                    max_id: 5,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(calls, vec![(0, 3)]);
    }

    #[tokio::test]
    async fn collect_admin_log_page_size_capped_at_100() {
        let mut next = 0usize;
        let mut calls = Vec::new();
        let events = collect_admin_log(250, |max_id, page_limit| {
            let ids: Vec<i64> = (0..page_limit)
                .map(|i| 1000 - next as i64 * 100 - i as i64)
                .collect();
            let new_max = ids.last().copied().unwrap_or(0);
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(events.len(), 250);
        assert_eq!(calls, vec![(0, 100), (901, 100), (801, 50)]);
    }

    fn admin_rights() -> tl::enums::ChatAdminRights {
        tl::enums::ChatAdminRights::Rights(tl::types::ChatAdminRights {
            change_info: false,
            post_messages: false,
            edit_messages: false,
            delete_messages: false,
            ban_users: false,
            invite_users: false,
            pin_messages: false,
            add_admins: false,
            anonymous: false,
            manage_call: false,
            other: false,
            manage_topics: false,
            post_stories: false,
            edit_stories: false,
            delete_stories: false,
            manage_direct_messages: false,
            manage_ranks: false,
        })
    }

    fn banned_rights() -> tl::enums::ChatBannedRights {
        tl::enums::ChatBannedRights::Rights(tl::types::ChatBannedRights {
            view_messages: false,
            send_messages: false,
            send_media: false,
            send_stickers: false,
            send_gifs: false,
            send_games: false,
            send_inline: false,
            embed_links: false,
            send_polls: false,
            change_info: false,
            invite_users: false,
            pin_messages: false,
            manage_topics: false,
            send_photos: false,
            send_videos: false,
            send_roundvideos: false,
            send_audios: false,
            send_voices: false,
            send_docs: false,
            send_plain: false,
            edit_rank: false,
            send_reactions: false,
            until_date: 0,
        })
    }

    fn offline_client() -> grammers_client::Client {
        let session = std::sync::Arc::new(grammers_session::storages::MemorySession::default());
        let pool = grammers_client::sender::SenderPool::new(session, 12345);
        grammers_client::Client::new(pool.handle)
    }

    fn create_args(kind: &str) -> CreateArgs {
        CreateArgs {
            title: "t".to_string(),
            description: None,
            kind: kind.to_string(),
            forum: false,
        }
    }

    #[test]
    fn create_rejects_unknown_kind() {
        assert!(matches!(
            validate_create(&create_args("broadcast")),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn create_accepts_known_kinds() {
        for kind in ["group", "supergroup", "channel"] {
            assert!(
                validate_create(&create_args(kind)).is_ok(),
                "kind {kind} should pass"
            );
        }
    }

    fn dryrun_flags(command: &str) -> GlobalFlags {
        GlobalFlags {
            account: vec!["me".to_string()],
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: true,
            quiet: true,
            config_path: None,
            command: command.to_string(),
        }
    }

    #[test]
    fn admin_rejects_empty_chat() {
        let mut args = AdminArgs {
            chat: "  ".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: false,
            title: None,
            preset: None,
            rights: None,
        };
        assert!(matches!(validate_admin(&args), Err(TeleError::Usage(_))));
        args.chat = "c".to_string();
        assert!(validate_admin(&args).is_ok());
    }

    #[tokio::test]
    async fn chat_commands_reject_empty_chat_before_connect() {
        let flags = dryrun_flags("chat join");
        assert!(matches!(
            join(
                ChatArgs {
                    chat: String::new()
                },
                &flags
            )
            .await,
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            leave(
                ChatArgs {
                    chat: "   ".to_string()
                },
                &flags
            )
            .await,
            Err(TeleError::Usage(_))
        ));

        let flags = dryrun_flags("chat invite");
        assert!(matches!(
            invite(
                InviteArgs {
                    chat: String::new(),
                    user: "u".to_string(),
                },
                &flags,
            )
            .await,
            Err(TeleError::Usage(_))
        ));

        let flags = dryrun_flags("chat participants");
        assert!(matches!(
            participants(
                ParticipantsArgs {
                    chat: "\t".to_string(),
                    limit: 10,
                },
                &flags,
            )
            .await,
            Err(TeleError::Usage(_))
        ));

        let flags = dryrun_flags("chat kick");
        assert!(matches!(
            kick(
                KickArgs {
                    chat: String::new(),
                    user: "u".to_string(),
                },
                &flags,
            )
            .await,
            Err(TeleError::Usage(_))
        ));

        let flags = dryrun_flags("chat admin-log");
        assert!(matches!(
            admin_log(
                AdminLogArgs {
                    chat: "   ".to_string(),
                    limit: 10,
                },
                &flags,
            )
            .await,
            Err(TeleError::Usage(_))
        ));

        let flags = dryrun_flags("chat stats");
        assert!(matches!(
            stats(
                StatsArgs {
                    chat: String::new(),
                    broadcast: false,
                },
                &flags,
            )
            .await,
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn admin_promote_and_demote_conflict() {
        let both = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: true,
            title: None,
            preset: None,
            rights: None,
        };
        assert!(matches!(validate_admin(&both), Err(TeleError::Usage(_))));
        let promote_only = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: false,
            title: None,
            preset: None,
            rights: None,
        };
        assert!(validate_admin(&promote_only).is_ok());
        let demote_only = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: false,
            demote: true,
            title: None,
            preset: None,
            rights: None,
        };
        assert!(validate_admin(&demote_only).is_ok());
    }

    #[test]
    fn admin_requires_promote_or_demote() {
        let neither = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: false,
            demote: false,
            title: None,
            preset: None,
            rights: None,
        };
        assert!(matches!(validate_admin(&neither), Err(TeleError::Usage(_))));
    }

    #[test]
    fn participant_user_id_never_masks_to_zero() {
        let own = 777;
        let participant =
            tl::enums::ChannelParticipant::Participant(tl::types::ChannelParticipant {
                user_id: 101,
                date: 0,
                subscription_until_date: None,
                rank: None,
            });
        assert_eq!(participant_user_id(&participant, own), 101);
        let self_p =
            tl::enums::ChannelParticipant::ParticipantSelf(tl::types::ChannelParticipantSelf {
                via_request: false,
                user_id: 0,
                inviter_id: 0,
                date: 0,
                subscription_until_date: None,
                rank: None,
            });
        assert_eq!(participant_user_id(&self_p, own), own);
        let creator =
            tl::enums::ChannelParticipant::Creator(tl::types::ChannelParticipantCreator {
                user_id: 202,
                admin_rights: admin_rights(),
                rank: None,
            });
        assert_eq!(participant_user_id(&creator, own), 202);
        let admin = tl::enums::ChannelParticipant::Admin(tl::types::ChannelParticipantAdmin {
            can_edit: false,
            is_self: false,
            user_id: 303,
            inviter_id: None,
            promoted_by: 1,
            date: 0,
            admin_rights: admin_rights(),
            rank: None,
        });
        assert_eq!(participant_user_id(&admin, own), 303);
        let banned = tl::enums::ChannelParticipant::Banned(tl::types::ChannelParticipantBanned {
            left: false,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 404 }),
            kicked_by: 1,
            date: 0,
            banned_rights: banned_rights(),
            rank: None,
        });
        assert_eq!(participant_user_id(&banned, own), 404);
        let left = tl::enums::ChannelParticipant::Left(tl::types::ChannelParticipantLeft {
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 505 }),
        });
        assert_eq!(participant_user_id(&left, own), 505);
    }

    #[tokio::test]
    async fn ensure_chat_peer_rejects_user_peer() {
        let client = offline_client();
        let user_peer = grammers_client::peer::Peer::User(grammers_client::peer::User::from_raw(
            &client,
            tl::enums::User::Empty(tl::types::UserEmpty { id: 0 }),
        ));
        let err = ensure_chat_peer(&user_peer, "kick").unwrap_err();
        assert!(err.message().contains("kick requires a chat, got a user"));
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
    }

    #[tokio::test]
    async fn ensure_chat_peer_accepts_group() {
        let client = offline_client();
        let group_peer =
            grammers_client::peer::Peer::Group(grammers_client::peer::Group::from_raw(
                &client,
                tl::enums::Chat::Empty(tl::types::ChatEmpty { id: 1 }),
            ));
        assert!(ensure_chat_peer(&group_peer, "participants").is_ok());
    }

    #[test]
    fn admin_action_display_composes_kind_and_title() {
        let action = serde_json::json!({"kind": "change_title", "title": "New Title"});
        assert_eq!(admin_action_display(&action), "change_title: New Title");
    }

    #[test]
    fn admin_action_display_uses_username_field() {
        let action = serde_json::json!({"kind": "change_username", "username": "new_handle"});
        assert_eq!(admin_action_display(&action), "change_username: new_handle");
    }

    #[test]
    fn admin_action_display_uses_text_field() {
        let action = serde_json::json!({"kind": "send_message", "id": 5, "text": "hello"});
        assert_eq!(admin_action_display(&action), "send_message: hello");
    }

    #[test]
    fn admin_action_display_ignores_number_fields() {
        let action = serde_json::json!({"kind": "delete_message", "id": 7});
        assert_eq!(admin_action_display(&action), "delete_message");
    }

    #[test]
    fn admin_action_display_empty_title_falls_back_to_kind() {
        let action = serde_json::json!({"kind": "change_title", "title": ""});
        assert_eq!(admin_action_display(&action), "change_title");
    }

    #[test]
    fn admin_action_display_missing_kind_is_other() {
        assert_eq!(admin_action_display(&serde_json::json!({})), "other");
    }

    #[test]
    fn admin_action_display_does_not_truncate_short() {
        let action = serde_json::json!({"kind": "change_title", "title": "t".repeat(40)});
        let out = admin_action_display(&action);
        assert!(!out.ends_with("..."));
        assert_eq!(out.chars().count(), 54);
    }

    #[test]
    fn admin_action_display_truncates_at_char_boundary() {
        let title = format!("{}{}", "a".repeat(42), "😀".repeat(4));
        let action = serde_json::json!({"kind": "change_title", "title": title});
        let out = admin_action_display(&action);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), 60);
        assert!(out.starts_with(&format!("change_title: {}", "a".repeat(42))));
        assert!(out.contains('😀'));
    }

    #[test]
    fn participant_rows_skips_entries_missing_user_data() {
        let mut users = HashMap::new();
        users.insert(11, test_user(11, "alice"));
        users.insert(22, test_user(22, "bob"));
        let participants = vec![
            tl::enums::ChatParticipant::Participant(tl::types::ChatParticipant {
                user_id: 11,
                inviter_id: 0,
                date: 0,
                rank: None,
            }),
            tl::enums::ChatParticipant::Admin(tl::types::ChatParticipantAdmin {
                user_id: 22,
                inviter_id: 0,
                date: 0,
                rank: None,
            }),
            tl::enums::ChatParticipant::Creator(tl::types::ChatParticipantCreator {
                user_id: 99,
                rank: None,
            }),
        ];
        let (rows, missing) = participant_rows(&offline_client(), &users, &participants);
        assert_eq!(rows.len(), 2);
        assert_eq!(missing, 1);
        assert_eq!(rows[0]["id"], 11);
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[0]["role"], "member");
        assert_eq!(rows[1]["id"], 22);
        assert_eq!(rows[1]["name"], "bob");
        assert_eq!(rows[1]["role"], "admin");
    }

    #[test]
    fn participant_rows_maps_roles_for_found_users() {
        let mut users = HashMap::new();
        users.insert(7, test_user(7, "creator"));
        let participants = vec![
            tl::enums::ChatParticipant::Creator(tl::types::ChatParticipantCreator {
                user_id: 7,
                rank: None,
            }),
            tl::enums::ChatParticipant::Admin(tl::types::ChatParticipantAdmin {
                user_id: 7,
                inviter_id: 0,
                date: 0,
                rank: None,
            }),
            tl::enums::ChatParticipant::Participant(tl::types::ChatParticipant {
                user_id: 7,
                inviter_id: 0,
                date: 0,
                rank: None,
            }),
        ];
        let (rows, missing) = participant_rows(&offline_client(), &users, &participants);
        assert_eq!(rows.len(), 3);
        assert_eq!(missing, 0);
        assert_eq!(rows[0]["role"], "creator");
        assert_eq!(rows[1]["role"], "admin");
        assert_eq!(rows[2]["role"], "member");
    }

    fn test_user(id: i64, name: &str) -> tl::enums::User {
        tl::enums::User::User(tl::types::User {
            is_self: false,
            contact: false,
            mutual_contact: false,
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
            id,
            access_hash: None,
            first_name: Some(name.to_string()),
            last_name: None,
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
        })
    }
}
