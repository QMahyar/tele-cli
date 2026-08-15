use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::{creds, creds_api_id};
use crate::commands::helpers::{peer_id, stats_abs, stats_percent, stats_period};
use crate::entities;
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
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            if let Some(link) = grammers_client::Client::parse_invite_link(&target) {
                guard
                    .client
                    .accept_invite_link(&link)
                    .await
                    .map_err(tele_invocation)?;
            } else {
                let peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                    .await
                    .map_err(tele_invocation)?;
                let chat_ref = entities::peer_ref(&peer).await.map_err(tele_invocation)?;
                guard
                    .client
                    .join_chat(chat_ref)
                    .await
                    .map_err(tele_invocation)?;
            }
            Ok(serde_json::json!({"chat": target, "joined": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn leave(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
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
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target, "user": user}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let user_peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), &user)
                .await
                .map_err(tele_invocation)?;
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
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard = ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            ensure_chat_peer(&chat, "participants")?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let mut iter = guard.client.iter_participants(chat_ref);
            let mut rows = Vec::new();
            let mut count = 0u32;
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
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target, "user": user}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            ensure_chat_peer(&chat, "kick")?;
            let user_peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), &user)
                .await
                .map_err(tele_invocation)?;
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
    Ok(())
}

async fn admin(args: AdminArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_admin(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let promote = args.promote;
    let demote = args.demote;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        let title = args.title.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "user": user,
                    "promote": promote,
                    "demote": demote,
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let user_peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), &user)
                .await
                .map_err(tele_invocation)?;
            let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
            let user_ref = entities::peer_ref(&user_peer)
                .await
                .map_err(tele_invocation)?;
            let mut builder = guard.client.set_admin_rights(chat_ref, user_ref);
            if promote {
                builder = builder
                    .change_info(true)
                    .post_messages(true)
                    .edit_messages(true)
                    .delete_messages(true)
                    .ban_users(true)
                    .invite_users(true)
                    .pin_messages(true)
                    .add_admins(true)
                    .manage_call(true);
            }
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
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let own_id = guard
                .client
                .get_me()
                .await
                .map_err(tele_invocation)?
                .id()
                .bare_id()
                .unwrap_or_default();
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let channel = entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let raw: tl::enums::channels::AdminLogResults = guard
                .client
                .invoke(&tl::functions::channels::GetAdminLog {
                    channel,
                    q: String::new(),
                    events_filter: None,
                    admins: None,
                    max_id: 0,
                    min_id: 0,
                    limit: limit as i32,
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::channels::AdminLogResults::Results(results) = raw;
            let mut rows = Vec::new();
            for event in results.events {
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

async fn stats(args: StatsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let broadcast = args.broadcast;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
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
                return Ok(serde_json::json!({"dry_run": true, "title": title}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client, &creds()?).await?;
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

fn ensure_chat_peer(peer: &grammers_client::peer::Peer, action: &str) -> TeleResult<()> {
    if matches!(peer, grammers_client::peer::Peer::User(_)) {
        return Err(TeleError::Other(format!(
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

    #[test]
    fn admin_promote_and_demote_conflict() {
        let both = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: true,
            title: None,
        };
        assert!(matches!(validate_admin(&both), Err(TeleError::Usage(_))));
        let promote_only = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: false,
            title: None,
        };
        assert!(validate_admin(&promote_only).is_ok());
        let demote_only = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: false,
            demote: true,
            title: None,
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
        assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
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
}
