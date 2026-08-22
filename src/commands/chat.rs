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
    Settings(SettingsArgs),
    Edit(EditArgs),
    Link(LinkArgs),
    Create(CreateArgs),
}

#[derive(Args)]
pub struct ChatArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
}

#[derive(Args)]
pub struct InviteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to invite (default mode): @username, t.me link, numeric ID, +phone, or me"
    )]
    user: Option<String>,
    #[arg(
        long,
        value_name = "TS|RFC3339|DURATION",
        help = "export/edit: link expiry as unix ts, RFC3339 date, or duration (90s/30m/24h/7d/2w)"
    )]
    expire: Option<String>,
    #[arg(long, help = "export/edit: link title shown to joiners")]
    title: Option<String>,
    #[arg(long, help = "export/edit: max number of link uses")]
    usage_limit: Option<u32>,
    #[arg(
        long,
        value_name = "true|false",
        help = "export/edit: require admin approval to join via the link"
    )]
    request_approval: Option<String>,
    #[arg(long, help = "list invite links exported by this account (admin only)")]
    list: bool,
    #[arg(long, help = "with --list: list revoked links instead of active ones")]
    revoked: bool,
    #[arg(
        long,
        value_name = "LINK",
        help = "with --list: show who joined LINK instead of listing links"
    )]
    importers: Option<String>,
    #[arg(
        long,
        value_name = "LINK",
        help = "edit an exported link (options below apply); add --revoke to revoke it"
    )]
    edit: Option<String>,
    #[arg(long, help = "with --edit: revoke the link")]
    revoke: bool,
    #[arg(long, help = "delete every revoked link exported by this account")]
    delete_revoked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InviteMode {
    User,
    Export,
    List,
    Edit,
    DeleteRevoked,
}

#[derive(Args)]
pub struct ParticipantsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "filter by role: admin, banned, kicked, recent (channels/supergroups only)"
    )]
    role: Option<String>,
    #[arg(
        long,
        help = "search participants by name/username (channels/supergroups only)"
    )]
    search: Option<String>,
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
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to kick: @username, t.me link, numeric ID, +phone, or me"
    )]
    user: String,
    #[arg(
        long,
        help = "ban instead of plain kick (deny view_messages; permanent unless --duration)"
    )]
    ban: bool,
    #[arg(
        long,
        value_name = "SECS|forever",
        help = "ban/restriction length in seconds, or forever (requires --ban or --rights)"
    )]
    duration: Option<String>,
    #[arg(
        long,
        value_name = "CSV",
        help = "comma-separated right:value pairs, e.g. send_stickers:false (right: view_messages,send_messages,send_media,send_stickers,send_gifs,send_games,send_inline,embed_links,send_polls,change_info,invite_users,pin_messages)"
    )]
    rights: Option<String>,
}

#[derive(Args)]
pub struct AdminArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "user to promote or demote: @username, t.me link, numeric ID, +phone, or me"
    )]
    user: String,
    #[arg(
        long,
        conflicts_with = "demote",
        help = "grant admin rights (mutually exclusive with --demote)"
    )]
    promote: bool,
    #[arg(
        long,
        conflicts_with = "promote",
        help = "revoke admin rights (mutually exclusive with --promote)"
    )]
    demote: bool,
    #[arg(long, help = "admin rank title (e.g. Mod, Admin)")]
    title: Option<String>,
    #[arg(
        long,
        conflicts_with = "rights",
        help = "preset rights: moderator, editor, admin (mutually exclusive with --rights)"
    )]
    preset: Option<String>,
    #[arg(
        long,
        conflicts_with = "preset",
        value_name = "CSV",
        help = "comma-separated rights: change_info,post,edit,delete,ban,invite,pin,add_admins,manage_call,anonymous,other,manage_topics (mutually exclusive with --preset)"
    )]
    rights: Option<String>,
}

#[derive(Args)]
pub struct AdminLogArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(long, default_value_t = 20, help = "max events to return (1-10000)")]
    limit: u32,
    #[arg(
        long,
        help = "only events by this admin: @username, t.me link, numeric ID, +phone, or me"
    )]
    admin: Option<String>,
    #[arg(long, help = "server-side string search over events")]
    search: Option<String>,
    #[arg(
        long,
        value_name = "TS|RFC3339",
        help = "only events at or after this unix ts or RFC3339 date (client-side)"
    )]
    since: Option<String>,
    #[arg(
        long,
        value_name = "TS|RFC3339",
        help = "only events at or before this unix ts or RFC3339 date (client-side)"
    )]
    until: Option<String>,
    #[arg(
        long,
        value_name = "CSV",
        help = "comma-separated event flags mapped to AdminLogEventsFilter: join,leave,invite,ban,unban,kick,unkick,promote,demote,info,settings,pinned,edit,delete,group_call,invites,send,forums,sub_extend,edit_rank"
    )]
    events: Option<String>,
}

#[derive(Args)]
pub struct StatsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "use broadcast channel stats (default: megagroup stats)")]
    broadcast: bool,
}

#[derive(Args)]
pub struct EditArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "new title (1-128 chars)")]
    title: Option<String>,
    #[arg(long, help = "new description (0-255 chars; empty string clears it)")]
    about: Option<String>,
    #[arg(
        long,
        value_name = "PATH|remove",
        help = "path to new photo, or 'remove' to delete the current one"
    )]
    photo: Option<String>,
}

#[derive(Args)]
pub struct LinkArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        value_name = "CHANNEL|remove",
        help = "discussion channel/group to link with --chat; omit to show the current link"
    )]
    to: Option<String>,
}

#[derive(Args)]
pub struct SettingsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        value_name = "SECS|off",
        help = "slow mode seconds (0-3600) or off; channels/supergroups only"
    )]
    slow_mode: Option<String>,
    #[arg(
        long,
        value_name = "on|off",
        help = "restrict saving content (not available in this API layer; read-only)"
    )]
    noforwards: Option<String>,
    #[arg(
        long,
        value_name = "on|off",
        help = "show author signatures in broadcast channels"
    )]
    signatures: Option<String>,
    #[arg(
        long,
        value_name = "on|off",
        help = "hide history before join (supergroups/channels)"
    )]
    pre_history: Option<String>,
    #[arg(
        long,
        value_name = "on|off",
        help = "require admin approval to join (channels/supergroups)"
    )]
    join_request: Option<String>,
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
        ChatCmd::Settings(a) => settings(a, flags).await,
        ChatCmd::Edit(a) => edit_chat(a, flags).await,
        ChatCmd::Link(a) => link_chat(a, flags).await,
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

const INVITE_LIST_LIMIT: i32 = 100;

#[derive(Debug, Clone)]
pub struct ValidatedInvite {
    mode: InviteMode,
    user: Option<String>,
    link: Option<String>,
    title: Option<String>,
    expire_date: Option<i32>,
    usage_limit: Option<i32>,
    request_needed: Option<bool>,
    revoked: bool,
}

impl Default for ValidatedInvite {
    fn default() -> Self {
        Self {
            mode: InviteMode::Export,
            user: None,
            link: None,
            title: None,
            expire_date: None,
            usage_limit: None,
            request_needed: None,
            revoked: false,
        }
    }
}

async fn invite(args: InviteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let plan = validate_invite(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let plan = plan.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(invite_dry_run_payload(&target, &plan));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            match plan.mode {
                InviteMode::User => {
                    let user = plan.user.clone().unwrap_or_default();
                    let chat =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                            .await?;
                    let user_peer =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), &user)
                            .await?;
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
                }
                InviteMode::Export | InviteMode::Edit => {
                    let chat =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                            .await?;
                    ensure_chat_peer(&chat, "chat invite")?;
                    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                    let rows = if plan.mode == InviteMode::Export {
                        let r: tl::enums::ExportedChatInvite = guard
                            .client
                            .invoke(&tl::functions::messages::ExportChatInvite {
                                legacy_revoke_permanent: false,
                                request_needed: plan.request_needed.unwrap_or(false),
                                peer,
                                expire_date: plan.expire_date,
                                usage_limit: plan.usage_limit,
                                title: plan.title.clone(),
                                subscription_pricing: None,
                            })
                            .await
                            .map_err(tele_invocation)?;
                        vec![exported_invite_row(&r)]
                    } else {
                        let link = plan.link.clone().unwrap_or_default();
                        let r: tl::enums::messages::ExportedChatInvite = guard
                            .client
                            .invoke(&tl::functions::messages::EditExportedChatInvite {
                                revoked: plan.revoked,
                                peer,
                                link,
                                expire_date: plan.expire_date,
                                usage_limit: plan.usage_limit,
                                request_needed: plan.request_needed,
                                title: plan.title.clone(),
                            })
                            .await
                            .map_err(tele_invocation)?;
                        exported_invite_result_rows(&r)
                    };
                    if !output::machine_mode(json, jsonl) {
                        print_invite_link_table(&name, multi, &rows)?;
                    }
                    Ok(serde_json::json!({"links": rows}))
                }
                InviteMode::List => {
                    let chat =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                            .await?;
                    ensure_chat_peer(&chat, "chat invite")?;
                    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                    let admin_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
                    match plan.link.clone() {
                        Some(link) => {
                            let r: tl::enums::messages::ChatInviteImporters = guard
                                .client
                                .invoke(&tl::functions::messages::GetChatInviteImporters {
                                    requested: false,
                                    subscription_expired: false,
                                    peer,
                                    link: Some(link),
                                    q: None,
                                    offset_date: 0,
                                    offset_user: tl::types::InputUserEmpty {}.into(),
                                    limit: INVITE_LIST_LIMIT,
                                })
                                .await
                                .map_err(tele_invocation)?;
                            let rows = chat_invite_importers_rows(&guard.client, &r);
                            if !output::machine_mode(json, jsonl) {
                                print_importer_table(&name, multi, &rows)?;
                            }
                            Ok(serde_json::json!({"importers": rows}))
                        }
                        None => {
                            let r: tl::enums::messages::ExportedChatInvites = guard
                                .client
                                .invoke(&tl::functions::messages::GetExportedChatInvites {
                                    revoked: plan.revoked,
                                    peer,
                                    admin_id,
                                    offset_date: None,
                                    offset_link: None,
                                    limit: INVITE_LIST_LIMIT,
                                })
                                .await
                                .map_err(tele_invocation)?;
                            let rows = exported_chat_invites_rows(&r);
                            if !output::machine_mode(json, jsonl) {
                                print_invite_link_table(&name, multi, &rows)?;
                            }
                            Ok(serde_json::json!({"links": rows}))
                        }
                    }
                }
                InviteMode::DeleteRevoked => {
                    let chat =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                            .await?;
                    ensure_chat_peer(&chat, "chat invite")?;
                    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                    let admin_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
                    let deleted: bool = guard
                        .client
                        .invoke(&tl::functions::messages::DeleteRevokedExportedChatInvites {
                            peer,
                            admin_id,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    if !output::machine_mode(json, jsonl) {
                        output::print_account_table(
                            &name,
                            multi,
                            &["deleted_revoked"],
                            &[vec![deleted.to_string()]],
                        )?;
                    }
                    Ok(serde_json::json!({"deleted_revoked": deleted}))
                }
            }
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn invite_dry_run_payload(chat: &str, plan: &ValidatedInvite) -> serde_json::Value {
    match plan.mode {
        InviteMode::User => {
            let user = plan.user.clone().unwrap_or_default();
            serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "user": user,
                "would": format!("invite user {user} to chat {chat}")
            })
        }
        InviteMode::Export => {
            let mut v = serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "export",
                "would": format!("export invite link of chat {chat}"),
            });
            invite_echo_options(&mut v, plan);
            v
        }
        InviteMode::List => {
            let would = match &plan.link {
                Some(link) => format!("list who joined link {link} in chat {chat}"),
                None => format!(
                    "list {} invite links of chat {chat}",
                    if plan.revoked { "revoked" } else { "active" }
                ),
            };
            serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "list",
                "revoked": plan.revoked,
                "importers": plan.link,
                "would": would,
            })
        }
        InviteMode::Edit => {
            let link = plan.link.clone().unwrap_or_default();
            let action = if plan.revoked { "revoke" } else { "edit" };
            let mut v = serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "edit",
                "link": link,
                "revoke": plan.revoked,
                "would": format!("{action} invite link {link} in chat {chat}"),
            });
            invite_echo_options(&mut v, plan);
            v
        }
        InviteMode::DeleteRevoked => {
            serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "delete_revoked",
                "would": format!("delete revoked invite links exported from chat {chat}"),
            })
        }
    }
}

fn invite_echo_options(v: &mut serde_json::Value, plan: &ValidatedInvite) {
    if plan.title.is_some() {
        v["title"] = serde_json::json!(plan.title);
    }
    if plan.expire_date.is_some() {
        v["expire_date"] = serde_json::json!(plan.expire_date);
    }
    if plan.usage_limit.is_some() {
        v["usage_limit"] = serde_json::json!(plan.usage_limit);
    }
    if plan.request_needed.is_some() {
        v["request_approval"] = serde_json::json!(plan.request_needed);
    }
}

fn has_any_option(args: &InviteArgs) -> bool {
    args.title.is_some()
        || args.expire.is_some()
        || args.usage_limit.is_some()
        || args.request_approval.is_some()
}

fn validate_invite(args: &InviteArgs) -> TeleResult<ValidatedInvite> {
    require_chat_target(&args.chat, "chat")?;
    let requested_modes = [
        args.user.is_some(),
        args.list,
        args.edit.is_some(),
        args.delete_revoked,
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if requested_modes > 1 {
        return Err(TeleError::Usage(
            "--user, --list, --edit, and --delete-revoked are mutually exclusive".to_string(),
        ));
    }
    if args.revoke && args.edit.is_none() {
        return Err(TeleError::Usage(
            "--revoke requires --edit <link>".to_string(),
        ));
    }
    if args.revoked && !args.list {
        return Err(TeleError::Usage("--revoked requires --list".to_string()));
    }
    if args.importers.is_some() && !args.list {
        return Err(TeleError::Usage("--importers requires --list".to_string()));
    }
    if args.importers.is_some() && args.revoked {
        return Err(TeleError::Usage(
            "--revoked cannot be combined with --importers".to_string(),
        ));
    }
    let mut plan = ValidatedInvite::default();
    if let Some(user) = &args.user {
        if has_any_option(args) {
            return Err(TeleError::Usage(
                "--title/--expire/--usage-limit/--request-approval configure invite links, not user invites".to_string(),
            ));
        }
        if user.trim().is_empty() {
            return Err(TeleError::Usage("--user must not be empty".to_string()));
        }
        plan.mode = InviteMode::User;
        plan.user = Some(user.trim().to_string());
        return Ok(plan);
    }
    if args.delete_revoked {
        if has_any_option(args) {
            return Err(TeleError::Usage(
                "--title/--expire/--usage-limit/--request-approval apply to link export/edit, not --delete-revoked".to_string(),
            ));
        }
        plan.mode = InviteMode::DeleteRevoked;
        return Ok(plan);
    }
    if let Some(link) = &args.edit {
        if !args.revoke && !has_any_option(args) {
            return Err(TeleError::Usage(
                "--edit needs at least one of --title/--expire/--usage-limit/--request-approval/--revoke".to_string(),
            ));
        }
        plan.mode = InviteMode::Edit;
        plan.link = Some(normalized_validated_link(link, "--edit")?);
        plan.revoked = args.revoke;
    } else if args.list {
        if let Some(link) = &args.importers {
            plan.link = Some(normalized_validated_link(link, "--importers")?);
        }
        if has_any_option(args) {
            return Err(TeleError::Usage(
                "--title/--expire/--usage-limit/--request-approval apply to link export/edit, not --list".to_string(),
            ));
        }
        plan.mode = InviteMode::List;
        plan.revoked = args.revoked;
    } else {
        plan.mode = InviteMode::Export;
    }
    if let Some(title) = &args.title {
        let t = title.trim();
        if t.is_empty() {
            return Err(TeleError::Usage("--title must not be empty".to_string()));
        }
        plan.title = Some(t.to_string());
    }
    if let Some(expire) = &args.expire {
        plan.expire_date = Some(parse_invite_expire(expire)?);
    }
    if let Some(limit) = args.usage_limit {
        if limit == 0 {
            return Err(TeleError::Usage(
                "--usage-limit must be greater than zero".to_string(),
            ));
        }
        plan.usage_limit = Some(limit as i32);
    }
    if let Some(flag) = &args.request_approval {
        plan.request_needed = Some(parse_invite_bool(flag)?);
    }
    Ok(plan)
}

fn normalized_validated_link(input: &str, flag: &str) -> TeleResult<String> {
    let normalized = normalize_invite_link(input);
    validate_invite_link(&normalized)
        .map_err(|e| TeleError::Usage(format!("{flag}: {}", e.message())))?;
    Ok(normalized)
}

fn parse_invite_bool(value: &str) -> TeleResult<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(TeleError::Usage(format!(
            "--request-approval must be true or false, got \"{other}\""
        ))),
    }
}

fn invite_duration_seconds(value: &str) -> Option<i64> {
    let v = value.trim();
    let (digits, unit_secs) = match v.chars().last()? {
        's' => (&v[..v.len() - 1], 1i64),
        'm' => (&v[..v.len() - 1], 60),
        'h' => (&v[..v.len() - 1], 3600),
        'd' => (&v[..v.len() - 1], 86_400),
        'w' => (&v[..v.len() - 1], 604_800),
        _ => return None,
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()?.checked_mul(unit_secs)
}

fn parse_invite_expire_at(now_ts: i64, value: &str) -> TeleResult<i32> {
    let v = value.trim();
    if v.is_empty() {
        return Err(TeleError::Usage("--expire must not be empty".to_string()));
    }
    let ts = if let Some(secs) = invite_duration_seconds(v) {
        now_ts
            .checked_add(secs)
            .ok_or_else(|| TeleError::Usage(format!("--expire out of range: {value}")))?
    } else {
        crate::commands::parse_unixtime(v)?.timestamp()
    };
    if ts <= now_ts {
        return Err(TeleError::Usage(format!(
            "--expire must be in the future: {value}"
        )));
    }
    i32::try_from(ts).map_err(|_| TeleError::Usage(format!("--expire out of range: {value}")))
}

fn parse_invite_expire(value: &str) -> TeleResult<i32> {
    parse_invite_expire_at(chrono::Utc::now().timestamp(), value)
}

fn exported_invite_row(invite: &tl::enums::ExportedChatInvite) -> serde_json::Value {
    match invite {
        tl::enums::ExportedChatInvite::ChatInviteExported(i) => {
            serde_json::json!({
                "link": i.link,
                "title": i.title,
                "revoked": i.revoked,
                "permanent": i.permanent,
                "request_needed": i.request_needed,
                "start_date": i.start_date,
                "expire_date": i.expire_date,
                "usage_limit": i.usage_limit,
                "usage": i.usage,
                "requested": i.requested,
                "admin_id": i.admin_id,
                "date": rfc3339_or_empty(Some(i.date)),
            })
        }
        tl::enums::ExportedChatInvite::ChatInvitePublicJoinRequests => {
            serde_json::json!({"public_join_requests": true})
        }
    }
}

fn exported_invite_result_rows(
    result: &tl::enums::messages::ExportedChatInvite,
) -> Vec<serde_json::Value> {
    match result {
        tl::enums::messages::ExportedChatInvite::Invite(wrapped) => {
            vec![exported_invite_row(&wrapped.invite)]
        }
        tl::enums::messages::ExportedChatInvite::Replaced(replaced) => {
            vec![
                exported_invite_row(&replaced.invite),
                exported_invite_row(&replaced.new_invite),
            ]
        }
    }
}

fn exported_chat_invites_rows(
    result: &tl::enums::messages::ExportedChatInvites,
) -> Vec<serde_json::Value> {
    match result {
        tl::enums::messages::ExportedChatInvites::Invites(list) => {
            list.invites.iter().map(exported_invite_row).collect()
        }
    }
}

fn chat_invite_importers_rows(
    client: &grammers_client::Client,
    result: &tl::enums::messages::ChatInviteImporters,
) -> Vec<serde_json::Value> {
    let tl::enums::messages::ChatInviteImporters::Importers(list) = result;
    let mut users: HashMap<i64, tl::enums::User> = HashMap::new();
    for u in &list.users {
        if let tl::enums::User::User(uu) = u {
            users.insert(uu.id, u.clone());
        }
    }
    list.importers
        .iter()
        .map(|importer| {
            let tl::enums::ChatInviteImporter::Importer(imp) = importer;
            serde_json::json!({
                "id": imp.user_id,
                "name": user_display_name(client, &users, imp.user_id),
                "date": rfc3339_or_empty(Some(imp.date)),
                "requested": imp.requested,
                "approved_by": imp.approved_by,
            })
        })
        .collect()
}

fn user_display_name(
    client: &grammers_client::Client,
    users: &HashMap<i64, tl::enums::User>,
    id: i64,
) -> String {
    match users.get(&id) {
        Some(u) => crate::serialize::peer_name(&grammers_client::peer::Peer::User(
            grammers_client::peer::User::from_raw(client, u.clone()),
        )),
        None => id.to_string(),
    }
}

fn rfc3339_or_empty(ts: Option<i32>) -> String {
    ts.and_then(|t| chrono::DateTime::from_timestamp(t as i64, 0))
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn print_invite_link_table(
    account: &str,
    multi: bool,
    rows: &[serde_json::Value],
) -> TeleResult<()> {
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r["link"].as_str().unwrap_or_default().to_string(),
                r["title"].as_str().unwrap_or_default().to_string(),
                truncate_cell(r["expire_date"].as_i64()),
                r["usage"].to_string(),
                r["usage_limit"].to_string(),
            ]
        })
        .collect();
    output::print_account_table(
        account,
        multi,
        &["link", "title", "expires", "uses", "limit"],
        &table_rows,
    )
}

fn truncate_cell(expire_date: Option<i64>) -> String {
    match expire_date {
        Some(0) | None => String::new(),
        Some(ts) => chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| ts.to_string()),
    }
}

fn print_importer_table(account: &str, multi: bool, rows: &[serde_json::Value]) -> TeleResult<()> {
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r["id"].to_string(),
                r["name"].as_str().unwrap_or_default().to_string(),
                r["date"].as_str().unwrap_or_default().to_string(),
                r["requested"].to_string(),
                r["approved_by"].to_string(),
            ]
        })
        .collect();
    output::print_account_table(
        account,
        multi,
        &["id", "name", "date", "requested", "approved_by"],
        &table_rows,
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ParticipantRole {
    Admin,
    Banned,
    Kicked,
    Recent,
}

fn parse_participant_role(role: Option<&str>) -> TeleResult<Option<ParticipantRole>> {
    match role {
        None => Ok(None),
        Some("admin") => Ok(Some(ParticipantRole::Admin)),
        Some("banned") => Ok(Some(ParticipantRole::Banned)),
        Some("kicked") => Ok(Some(ParticipantRole::Kicked)),
        Some("recent") => Ok(Some(ParticipantRole::Recent)),
        Some(other) => Err(TeleError::Usage(format!(
            "unknown role '{other}': use admin, banned, kicked, or recent"
        ))),
    }
}

fn participant_filter(
    role: Option<ParticipantRole>,
    search: Option<&str>,
) -> tl::enums::ChannelParticipantsFilter {
    use tl::enums::ChannelParticipantsFilter as F;
    use tl::types::{
        ChannelParticipantsBanned, ChannelParticipantsKicked, ChannelParticipantsSearch,
    };
    let q = search.unwrap_or_default().to_string();
    match role {
        Some(ParticipantRole::Admin) => F::ChannelParticipantsAdmins,
        Some(ParticipantRole::Banned) => {
            F::ChannelParticipantsBanned(ChannelParticipantsBanned { q })
        }
        Some(ParticipantRole::Kicked) => {
            F::ChannelParticipantsKicked(ChannelParticipantsKicked { q })
        }
        Some(ParticipantRole::Recent) | None => match search {
            Some(s) if !s.is_empty() => {
                F::ChannelParticipantsSearch(ChannelParticipantsSearch { q: s.to_string() })
            }
            _ => F::ChannelParticipantsRecent,
        },
    }
}

async fn participants(args: ParticipantsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let role = parse_participant_role(args.role.as_deref())?;
    let search = args
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let search = search.clone();
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
                if role.is_some() || search.is_some() {
                    return Err(TeleError::Usage(
                        "--role/--search filters require a channel or supergroup; basic groups list all members".to_string(),
                    ));
                }
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
                let mut iter = guard
                    .client
                    .iter_participants(chat_ref)
                    .filter(participant_filter(role, search.as_deref()));
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
                output::print_account_table(&name, multi, &["id", "name", "role"], &table_rows)?;
            }
            Ok(serde_json::json!({"participants": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

const BANNED_RIGHT_NAMES: &[&str] = &[
    "view_messages",
    "send_messages",
    "send_media",
    "send_stickers",
    "send_gifs",
    "send_games",
    "send_inline",
    "embed_links",
    "send_polls",
    "change_info",
    "invite_users",
    "pin_messages",
];

fn parse_ban_duration(duration: Option<&str>) -> TeleResult<Option<u32>> {
    match duration {
        None | Some("forever") => Ok(None),
        Some(raw) => {
            let secs: u64 = raw.parse().map_err(|_| {
                TeleError::Usage(format!(
                    "invalid --duration '{raw}': use seconds or 'forever'"
                ))
            })?;
            if secs == 0 {
                return Err(TeleError::Usage(
                    "--duration must be greater than 0 or 'forever'".to_string(),
                ));
            }
            u32::try_from(secs)
                .ok()
                .map(Some)
                .ok_or_else(|| TeleError::Usage("--duration is too large".to_string()))
        }
    }
}

fn parse_banned_rights_csv(csv: &str) -> TeleResult<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for part in csv.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once(':') else {
            return Err(TeleError::Usage(format!(
                "invalid rights entry '{part}': use name:true or name:false"
            )));
        };
        let name = name.trim();
        if !BANNED_RIGHT_NAMES.contains(&name) {
            return Err(TeleError::Usage(format!(
                "unknown right '{name}': use {}",
                BANNED_RIGHT_NAMES.join(",")
            )));
        }
        let allowed = match value.trim() {
            "true" => true,
            "false" => false,
            other => {
                return Err(TeleError::Usage(format!(
                    "invalid value '{other}' for right '{name}': use true or false"
                )))
            }
        };
        out.push((name.to_string(), allowed));
    }
    Ok(out)
}

fn validate_kick(args: &KickArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    let has_duration = args.duration.is_some();
    if has_duration && !args.ban && args.rights.is_none() {
        return Err(TeleError::Usage(
            "--duration requires --ban or --rights".to_string(),
        ));
    }
    parse_ban_duration(args.duration.as_deref())?;
    if let Some(rights) = &args.rights {
        parse_banned_rights_csv(rights)?;
    }
    Ok(())
}

async fn kick(args: KickArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_kick(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let ban = args.ban;
    let until_secs = parse_ban_duration(args.duration.as_deref())?;
    let rights_entries = match &args.rights {
        Some(csv) => parse_banned_rights_csv(csv)?,
        None => Vec::new(),
    };
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let user = args.user.clone();
        let rights_entries = rights_entries.clone();
        Box::pin(async move {
            if dry_run {
                let mut data = serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "user": user,
                    "ban": ban,
                    "would": format!("kick user {user} from chat {target}")
                });
                if let Some(secs) = until_secs {
                    data["duration"] = serde_json::json!(secs);
                }
                if !rights_entries.is_empty() {
                    data["rights"] = serde_json::json!(rights_entries);
                }
                return Ok(data);
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
            if !ban && rights_entries.is_empty() && until_secs.is_none() {
                guard
                    .client
                    .kick_participant(chat_ref, user_ref)
                    .await
                    .map_err(tele_invocation)?;
                return Ok(serde_json::json!({"chat": target, "user": user, "kicked": true}));
            }
            let mut call = guard.client.set_banned_rights(chat_ref, user_ref);
            for (right, allowed) in &rights_entries {
                call = match right.as_str() {
                    "view_messages" => call.view_messages(*allowed),
                    "send_messages" => call.send_messages(*allowed),
                    "send_media" => call.send_media(*allowed),
                    "send_stickers" => call.send_stickers(*allowed),
                    "send_gifs" => call.send_gifs(*allowed),
                    "send_games" => call.send_games(*allowed),
                    "send_inline" => call.send_inline(*allowed),
                    "embed_links" => call.embed_link_previews(*allowed),
                    "send_polls" => call.send_polls(*allowed),
                    "change_info" => call.change_info(*allowed),
                    "invite_users" => call.invite_users(*allowed),
                    "pin_messages" => call.pin_messages(*allowed),
                    _ => call,
                };
            }
            if ban {
                call = call.view_messages(false);
            }
            if let Some(secs) = until_secs {
                call = call.duration(std::time::Duration::from_secs(u64::from(secs)));
            }
            call.await.map_err(tele_invocation)?;
            let mut data = serde_json::json!({
                "chat": target,
                "user": user,
                "kicked": true,
                "banned": ban,
            });
            if let Some(secs) = until_secs {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                data["until"] = serde_json::json!(i64::from(secs) + now as i64);
            }
            if !rights_entries.is_empty() {
                let denied: Vec<&str> = rights_entries
                    .iter()
                    .filter(|(_, allowed)| !allowed)
                    .map(|(right, _)| right.as_str())
                    .collect();
                data["restricted"] = serde_json::json!(denied);
            }
            Ok(data)
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

#[derive(Clone, Copy, Debug)]
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
    anonymous: bool,
    other: bool,
    manage_topics: bool,
}

impl AdminRights {
    fn none() -> Self {
        Self {
            change_info: false,
            post_messages: false,
            edit_messages: false,
            delete_messages: false,
            ban_users: false,
            invite_users: false,
            pin_messages: false,
            add_admins: false,
            manage_call: false,
            anonymous: false,
            other: false,
            manage_topics: false,
        }
    }

    fn all() -> Self {
        Self::none().with_all_set()
    }

    fn with_all_set(mut self) -> Self {
        self.change_info = true;
        self.post_messages = true;
        self.edit_messages = true;
        self.delete_messages = true;
        self.ban_users = true;
        self.invite_users = true;
        self.pin_messages = true;
        self.add_admins = true;
        self.manage_call = true;
        self.other = true;
        self.manage_topics = true;
        self
    }

    fn moderator() -> Self {
        Self {
            delete_messages: true,
            ban_users: true,
            invite_users: true,
            pin_messages: true,
            manage_topics: true,
            ..Self::none()
        }
    }

    fn editor() -> Self {
        Self {
            change_info: true,
            post_messages: true,
            edit_messages: true,
            delete_messages: true,
            invite_users: true,
            pin_messages: true,
            manage_topics: true,
            ..Self::none()
        }
    }

    fn from_string(s: &str) -> TeleResult<Self> {
        let mut rights = Self::none();
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
                "anonymous" => rights.anonymous = true,
                "other" => rights.other = true,
                "manage_topics" => rights.manage_topics = true,
                "" => {}
                _ => {
                    return Err(TeleError::Usage(format!(
                        "unknown right '{}': use change_info,post,edit,delete,ban,invite,pin,add_admins,manage_call,anonymous,other,manage_topics",
                        part
                    )))
                }
            }
        }
        Ok(rights)
    }

    fn to_raw(self) -> tl::enums::ChatAdminRights {
        tl::enums::ChatAdminRights::Rights(tl::types::ChatAdminRights {
            change_info: self.change_info,
            post_messages: self.post_messages,
            edit_messages: self.edit_messages,
            delete_messages: self.delete_messages,
            ban_users: self.ban_users,
            invite_users: self.invite_users,
            pin_messages: self.pin_messages,
            add_admins: self.add_admins,
            manage_call: self.manage_call,
            anonymous: self.anonymous,
            other: self.other,
            manage_topics: self.manage_topics,
            post_stories: false,
            edit_stories: false,
            delete_stories: false,
            manage_direct_messages: false,
            manage_ranks: false,
        })
    }

    fn needs_raw_edit_admin(self) -> bool {
        self.other || self.manage_topics
    }
}

fn resolve_admin_rights(args: &AdminArgs) -> TeleResult<AdminRights> {
    if args.demote {
        return Ok(AdminRights::none());
    }
    if let Some(preset) = &args.preset {
        return Ok(match preset.as_str() {
            "moderator" => AdminRights::moderator(),
            "editor" => AdminRights::editor(),
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
            if rights.needs_raw_edit_admin() {
                guard
                    .client
                    .invoke(&tl::functions::channels::EditAdmin {
                        channel: entities::input_channel(&chat)
                            .await
                            .map_err(tele_invocation)?,
                        user_id: entities::input_user(&user_peer)
                            .await
                            .map_err(tele_invocation)?,
                        admin_rights: rights.to_raw(),
                        rank: title,
                    })
                    .await
                    .map_err(tele_invocation)?;
                return Ok(serde_json::json!({
                    "chat": target,
                    "user": user,
                    "promote": promote,
                    "demote": demote,
                }));
            }
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
                .manage_call(rights.manage_call)
                .anonymous(rights.anonymous);
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

fn parse_on_off(value: Option<&str>) -> TeleResult<Option<bool>> {
    match value {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(TeleError::Usage(format!(
            "invalid value '{other}': use on or off"
        ))),
    }
}

fn parse_slow_mode(value: Option<&str>) -> TeleResult<Option<i32>> {
    match value {
        None => Ok(None),
        Some("off") => Ok(Some(0)),
        Some(raw) => {
            let secs: i64 = raw.parse().map_err(|_| {
                TeleError::Usage(format!("invalid --slow-mode '{raw}': use seconds or 'off'"))
            })?;
            if !(0..=3600).contains(&secs) {
                return Err(TeleError::Usage(
                    "--slow-mode must be between 0 and 3600 seconds, or 'off'".to_string(),
                ));
            }
            Ok(Some(secs as i32))
        }
    }
}

fn validate_settings(args: &SettingsArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    parse_slow_mode(args.slow_mode.as_deref())?;
    parse_on_off(args.noforwards.as_deref())?;
    if let Some(value) = args.noforwards.as_deref() {
        return Err(TeleError::Usage(format!(
            "--noforwards {value} cannot be applied: the toggle method is not available in this API layer; current value is reported by read-back"
        )));
    }
    parse_on_off(args.signatures.as_deref())?;
    parse_on_off(args.pre_history.as_deref())?;
    parse_on_off(args.join_request.as_deref())?;
    Ok(())
}

fn channel_from_chats(chats: &[tl::enums::Chat], id: i64) -> Option<&tl::types::Channel> {
    for chat in chats {
        if let tl::enums::Chat::Channel(c) = chat {
            if c.id == id {
                return Some(c);
            }
        }
    }
    None
}

async fn settings(args: SettingsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_settings(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let slow_mode = parse_slow_mode(args.slow_mode.as_deref())?;
    let signatures = parse_on_off(args.signatures.as_deref())?;
    let pre_history = parse_on_off(args.pre_history.as_deref())?;
    let join_request = parse_on_off(args.join_request.as_deref())?;
    let has_toggles = slow_mode.is_some()
        || signatures.is_some()
        || pre_history.is_some()
        || join_request.is_some();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                let mut data = serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": if has_toggles {
                        format!("update settings of chat {target}")
                    } else {
                        format!("read settings of chat {target}")
                    },
                });
                if let Some(secs) = slow_mode {
                    data["slow_mode"] = serde_json::json!(secs);
                }
                if let Some(v) = signatures {
                    data["signatures"] = serde_json::json!(v);
                }
                if let Some(v) = pre_history {
                    data["pre_history"] = serde_json::json!(v);
                }
                if let Some(v) = join_request {
                    data["join_request"] = serde_json::json!(v);
                }
                return Ok(data);
            }            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "settings")?;
            let is_basic_group = matches!(&chat, grammers_client::peer::Peer::Group(_))
                && !entities::is_channel(&chat);
            if is_basic_group {
                return Err(TeleError::Usage(
                    "chat settings are not supported for basic groups; these toggles apply to channels and supergroups only".to_string(),
                ));
            }
            let input_channel = entities::input_channel(&chat).await.map_err(tele_invocation)?;
            if has_toggles {
                let mut applied = Vec::new();
                if let Some(secs) = slow_mode {
                    applied.push("slow_mode");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleSlowMode {
                            channel: input_channel.clone(),
                            seconds: secs,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = signatures {
                    applied.push("signatures");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleSignatures {
                            signatures_enabled: enabled,
                            profiles_enabled: false,
                            channel: input_channel.clone(),
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = pre_history {
                    applied.push("pre_history");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::TogglePreHistoryHidden {
                            channel: input_channel.clone(),
                            enabled,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = join_request {
                    applied.push("join_request");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleJoinRequest {
                            apply_to_invites: enabled,
                            channel: input_channel.clone(),
                            enabled,
                            guard_bot: None,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                return Ok(serde_json::json!({
                    "chat": target,
                    "applied": applied,
                }));
            }
            guard.rate_limiter.acquire().await;
            let full = guard
                .client
                .invoke(&tl::functions::channels::GetFullChannel {
                    channel: input_channel,
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::messages::ChatFull::Full(full) = full;
            let full_chat = match full.full_chat {
                tl::enums::ChatFull::ChannelFull(f) => f,
                tl::enums::ChatFull::Full(_) => {
                    return Err(TeleError::Other(
                        "settings unavailable: server returned group info for this chat"
                            .to_string(),
                    ));
                }
            };
            let channel = channel_from_chats(&full.chats, full_chat.id);
            Ok(serde_json::json!({
                "chat": target,
                "slow_mode": full_chat.slowmode_seconds.unwrap_or(0),
                "noforwards": channel.map(|c| c.noforwards),
                "signatures": channel.map(|c| c.signatures),
                "pre_history_hidden": full_chat.hidden_prehistory,
                "join_request": channel.map(|c| c.join_request),
                "linked_chat_id": full_chat.linked_chat_id,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

const CHAT_TITLE_MAX_CHARS: usize = 128;
const CHAT_ABOUT_MAX_CHARS: usize = 255;

fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    if args.title.is_none() && args.about.is_none() && args.photo.is_none() {
        return Err(TeleError::Usage(
            "at least one of --title, --about, --photo required".to_string(),
        ));
    }
    if let Some(title) = &args.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(TeleError::Usage("--title cannot be empty".to_string()));
        }
        if title.chars().count() > CHAT_TITLE_MAX_CHARS {
            return Err(TeleError::Usage(format!(
                "--title is too long: {} chars (max {CHAT_TITLE_MAX_CHARS})",
                title.chars().count()
            )));
        }
    }
    if let Some(about) = &args.about {
        if about.trim().chars().count() > CHAT_ABOUT_MAX_CHARS {
            return Err(TeleError::Usage(format!(
                "--about is too long: {} chars (max {CHAT_ABOUT_MAX_CHARS})",
                about.trim().chars().count()
            )));
        }
    }
    if let Some(photo) = &args.photo {
        if photo != "remove" {
            crate::commands::msg::validate_upload_path(photo)?;
        }
    }
    Ok(())
}

fn parse_link_target(target: Option<&str>) -> TeleResult<Option<String>> {
    match target {
        None => Ok(None),
        Some("remove") => Err(TeleError::Usage(
            "--to remove is not supported: this API layer has no unlink method (channels.setDiscussionGroup requires a group); re-point the link to another group instead".to_string(),
        )),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(TeleError::Usage("--to cannot be empty".to_string()));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

fn validate_link(args: &LinkArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    parse_link_target(args.to.as_deref())?;
    Ok(())
}

pub(crate) fn chat_photo_input_photo(photo: &tl::enums::Photo) -> Option<tl::enums::InputPhoto> {
    if let tl::enums::Photo::Photo(p) = photo {
        return Some(tl::enums::InputPhoto::Photo(tl::types::InputPhoto {
            id: p.id,
            access_hash: p.access_hash,
            file_reference: p.file_reference.clone(),
        }));
    }
    None
}

async fn fetch_full_chat_info(
    guard: &ClientGuard,
    chat: &grammers_client::peer::Peer,
) -> TeleResult<tl::enums::messages::ChatFull> {
    if matches!(chat, grammers_client::peer::Peer::Group(_)) && !entities::is_channel(chat) {
        guard
            .client
            .invoke(&tl::functions::messages::GetFullChat {
                chat_id: chat.id().bare_id().unwrap_or_default(),
            })
            .await
            .map_err(tele_invocation)
    } else {
        let input_channel = entities::input_channel(chat)
            .await
            .map_err(tele_invocation)?;
        guard
            .client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: input_channel,
            })
            .await
            .map_err(tele_invocation)
    }
}

async fn edit_chat(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_edit(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let title = args.title.clone();
    let about = args.about.clone();
    let photo = args.photo.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let title = title.clone();
        let about = about.clone();
        let photo = photo.clone();
        Box::pin(async move {
            if dry_run {
                let mut data = serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("edit metadata of chat {target}"),
                });
                if let Some(t) = &title {
                    data["title"] = serde_json::json!(t.trim());
                }
                if let Some(a) = &about {
                    data["about"] = serde_json::json!(a.trim());
                }
                if let Some(p) = &photo {
                    data["photo"] = serde_json::json!(p);
                }
                return Ok(data);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "chat edit")?;
            let is_basic_group = matches!(&chat, grammers_client::peer::Peer::Group(_))
                && !entities::is_channel(&chat);
            let mut applied = Vec::new();
            if let Some(new_title) = &title {
                applied.push("title");
                let new_title = new_title.trim().to_string();
                if is_basic_group {
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::messages::EditChatTitle {
                            chat_id: chat.id().bare_id().unwrap_or_default(),
                            title: new_title,
                        })
                        .await
                        .map_err(tele_invocation)?;
                } else {
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::EditTitle {
                            channel: entities::input_channel(&chat)
                                .await
                                .map_err(tele_invocation)?,
                            title: new_title,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
            }
            if let Some(new_about) = &about {
                applied.push("about");
                let new_about = new_about.trim().to_string();
                let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                guard.rate_limiter.acquire().await;
                guard
                    .client
                    .invoke(&tl::functions::messages::EditChatAbout {
                        peer,
                        about: new_about,
                    })
                    .await
                    .map_err(tele_invocation)?;
            }
            if let Some(photo) = &photo {
                applied.push("photo");
                if photo == "remove" {
                    let full = fetch_full_chat_info(&guard, &chat).await?;
                    let tl::enums::messages::ChatFull::Full(full) = full;
                    let current: Option<tl::enums::Photo> = match &full.full_chat {
                        tl::enums::ChatFull::ChannelFull(f) => Some(f.chat_photo.clone()),
                        tl::enums::ChatFull::Full(f) => f.chat_photo.clone(),
                    };
                    let input_photo = current
                        .as_ref()
                        .and_then(chat_photo_input_photo)
                        .ok_or_else(|| {
                            TeleError::Other("chat has no photo to remove".to_string())
                        })?;
                    guard.rate_limiter.acquire().await;
                    let _: Vec<i64> = guard
                        .client
                        .invoke(&tl::functions::photos::DeletePhotos {
                            id: vec![input_photo],
                        })
                        .await
                        .map_err(tele_invocation)?;
                } else {
                    let uploaded = guard
                        .client
                        .upload_file(photo)
                        .await
                        .map_err(|e| TeleError::TaskPanic(e.to_string()))?;
                    let chat_photo = tl::enums::InputChatPhoto::InputChatUploadedPhoto(
                        tl::types::InputChatUploadedPhoto {
                            file: Some(uploaded.raw),
                            video: None,
                            video_start_ts: None,
                            video_emoji_markup: None,
                        },
                    );
                    guard.rate_limiter.acquire().await;
                    if is_basic_group {
                        guard
                            .client
                            .invoke(&tl::functions::messages::EditChatPhoto {
                                chat_id: chat.id().bare_id().unwrap_or_default(),
                                photo: chat_photo,
                            })
                            .await
                            .map_err(tele_invocation)?;
                    } else {
                        guard
                            .client
                            .invoke(&tl::functions::channels::EditPhoto {
                                channel: entities::input_channel(&chat)
                                    .await
                                    .map_err(tele_invocation)?,
                                photo: chat_photo,
                            })
                            .await
                            .map_err(tele_invocation)?;
                    }
                }
            }
            Ok(serde_json::json!({
                "chat": target,
                "applied": applied,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn discussion_pair(
    x: grammers_client::peer::Peer,
    y: grammers_client::peer::Peer,
) -> TeleResult<(grammers_client::peer::Peer, grammers_client::peer::Peer)> {
    const NOT_A_DISCUSSION_PEER: &str =
        "discussion links need one broadcast channel and one supergroup";
    let x_broadcast = match &x {
        grammers_client::peer::Peer::Channel(_) => Ok(true),
        grammers_client::peer::Peer::Group(g) if g.is_megagroup() => Ok(false),
        _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
    }?;
    let y_broadcast = match &y {
        grammers_client::peer::Peer::Channel(_) => Ok(true),
        grammers_client::peer::Peer::Group(g) if g.is_megagroup() => Ok(false),
        _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
    }?;
    match (x_broadcast, y_broadcast) {
        (true, false) => Ok((x, y)),
        (false, true) => Ok((y, x)),
        _ => Err(TeleError::Usage(format!(
            "--chat and --to must be one broadcast channel and one supergroup (got {} + {})",
            if x_broadcast {
                "broadcast"
            } else {
                "supergroup"
            },
            if y_broadcast {
                "broadcast"
            } else {
                "supergroup"
            }
        ))),
    }
}

async fn link_chat(args: LinkArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_link(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let to_target = parse_link_target(args.to.as_deref())?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let to_target = to_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(match &to_target {
                    None => serde_json::json!({
                        "dry_run": true,
                        "chat": target,
                        "would": format!("read discussion link of chat {target}"),
                    }),
                    Some(to) => serde_json::json!({
                        "dry_run": true,
                        "chat": target,
                        "to": to,
                        "would": format!("link chat {target} with discussion group {to}"),
                    }),
                });
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "link")?;
            let Some(to_target) = to_target else {
                let full = fetch_full_chat_info(&guard, &chat).await?;
                let tl::enums::messages::ChatFull::Full(full) = full;
                let linked = match full.full_chat {
                    tl::enums::ChatFull::ChannelFull(f) => f.linked_chat_id,
                    tl::enums::ChatFull::Full(_) => None,
                };
                return Ok(serde_json::json!({
                    "chat": target,
                    "linked_chat_id": linked,
                }));
            };
            let to_peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &to_target).await?;
            ensure_chat_peer(&to_peer, "--to")?;
            let (broadcast, group) = discussion_pair(chat.clone(), to_peer)?;
            guard.rate_limiter.acquire().await;
            guard
                .client
                .invoke(&tl::functions::channels::SetDiscussionGroup {
                    broadcast: entities::input_channel(&broadcast)
                        .await
                        .map_err(tele_invocation)?,
                    group: entities::input_channel(&group)
                        .await
                        .map_err(tele_invocation)?,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "to": to_target,
                "linked": true,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn admin_log(args: AdminLogArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let since = args
        .since
        .as_deref()
        .map(crate::commands::parse_unixtime)
        .transpose()?;
    let until = args
        .until
        .as_deref()
        .map(crate::commands::parse_unixtime)
        .transpose()?;
    if let (Some(s), Some(u)) = (&since, &until) {
        if s > u {
            return Err(TeleError::Usage(
                "--since must not be after --until".to_string(),
            ));
        }
    }
    let events_filter = parse_admin_events_filter(args.events.as_deref())?;
    let search_q = args.search.clone().unwrap_or_default();
    let admin_target = args.admin.clone();
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let search_q = search_q.clone();
        let events_filter = events_filter.clone();
        let admin_target = admin_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(admin_log_dry_run_payload(
                    &target,
                    &search_q,
                    events_filter.is_some(),
                    admin_target.is_some(),
                ));
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
            let admins = match admin_target.as_deref() {
                None => None,
                Some("me") => Some(vec![tl::enums::InputUser::from(
                    tl::types::InputUserSelf {},
                )]),
                Some(user) => {
                    let peer =
                        entities::resolve_peer(&guard.client, guard.session.as_ref(), user).await?;
                    Some(vec![entities::input_user(&peer)
                        .await
                        .map_err(tele_invocation)?])
                }
            };
            let collected = {
                let guard_ref = &guard;
                let channel_ref = &channel;
                collect_admin_log(limit, move |max_id, page_limit| {
                    let q = search_q.clone();
                    let filter = events_filter.clone();
                    let admins = admins.clone();
                    async move {
                        let raw: tl::enums::channels::AdminLogResults = guard_ref
                            .client
                            .invoke(&tl::functions::channels::GetAdminLog {
                                channel: (*channel_ref).clone(),
                                q,
                                events_filter: filter,
                                admins,
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
                            users: results.users,
                            max_id: next_max_id,
                        })
                    }
                })
            }
            .await?;
            let mut rows = Vec::new();
            for event in collected.events {
                let tl::enums::ChannelAdminLogEvent::Event(event) = event;
                let date = chrono::DateTime::from_timestamp(event.date as i64, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
                rows.push(serde_json::json!({
                    "id": event.id,
                    "date": date,
                    "actor": actor_value(&guard.client, &collected.users, event.user_id),
                    "action": admin_action_summary(&event.action, own_id),
                }));
            }
            let rows = filter_events_by_range(rows, since, until);
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["date"].as_str().unwrap_or_default().to_string(),
                            r["actor"]["name"].as_str().unwrap_or_default().to_string(),
                            admin_action_display(&r["action"]),
                        ]
                    })
                    .collect();
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "date", "actor", "action"],
                    &table_rows,
                )?;
            }
            Ok(serde_json::json!({"events": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn admin_log_dry_run_payload(
    chat: &str,
    q: &str,
    has_events_filter: bool,
    has_admins: bool,
) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "search": if q.is_empty() { None::<&str> } else { Some(q) },
        "events_filter": has_events_filter,
        "admins": has_admins,
        "would": format!("list admin log of chat {chat}"),
    })
}

fn actor_value(
    client: &grammers_client::Client,
    users: &HashMap<i64, tl::enums::User>,
    user_id: i64,
) -> serde_json::Value {
    serde_json::json!({
        "id": user_id,
        "name": user_display_name(client, users, user_id),
    })
}

fn filter_events_by_range(
    rows: Vec<serde_json::Value>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<serde_json::Value> {
    if since.is_none() && until.is_none() {
        return rows;
    }
    rows.into_iter()
        .filter(|r| {
            r["date"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .map(|d| since.is_none_or(|s| d >= s) && until.is_none_or(|u| d <= u))
                .unwrap_or(true)
        })
        .collect()
}

const ADMIN_LOG_EVENT_FLAGS: &[&str] = &[
    "join",
    "leave",
    "invite",
    "ban",
    "unban",
    "kick",
    "unkick",
    "promote",
    "demote",
    "info",
    "settings",
    "pinned",
    "edit",
    "delete",
    "group_call",
    "invites",
    "send",
    "forums",
    "sub_extend",
    "edit_rank",
];

fn parse_admin_events_filter(
    csv: Option<&str>,
) -> TeleResult<Option<tl::enums::ChannelAdminLogEventsFilter>> {
    let Some(csv) = csv else {
        return Ok(None);
    };
    let names: Vec<&str> = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return Err(TeleError::Usage(format!(
            "--events must name at least one flag: {}",
            ADMIN_LOG_EVENT_FLAGS.join(",")
        )));
    }
    let mut f = tl::types::ChannelAdminLogEventsFilter {
        join: false,
        leave: false,
        invite: false,
        ban: false,
        unban: false,
        kick: false,
        unkick: false,
        promote: false,
        demote: false,
        info: false,
        settings: false,
        pinned: false,
        edit: false,
        delete: false,
        group_call: false,
        invites: false,
        send: false,
        forums: false,
        sub_extend: false,
        edit_rank: false,
    };
    for name in &names {
        match *name {
            "join" => f.join = true,
            "leave" => f.leave = true,
            "invite" => f.invite = true,
            "ban" => f.ban = true,
            "unban" => f.unban = true,
            "kick" => f.kick = true,
            "unkick" => f.unkick = true,
            "promote" => f.promote = true,
            "demote" => f.demote = true,
            "info" => f.info = true,
            "settings" => f.settings = true,
            "pinned" => f.pinned = true,
            "edit" => f.edit = true,
            "delete" => f.delete = true,
            "group_call" => f.group_call = true,
            "invites" => f.invites = true,
            "send" => f.send = true,
            "forums" => f.forums = true,
            "sub_extend" => f.sub_extend = true,
            "edit_rank" => f.edit_rank = true,
            other => {
                return Err(TeleError::Usage(format!(
                    "unknown --events flag '{other}': valid flags are {}",
                    ADMIN_LOG_EVENT_FLAGS.join(",")
                )))
            }
        }
    }
    Ok(Some(tl::enums::ChannelAdminLogEventsFilter::Filter(f)))
}

struct AdminLogPage {
    events: Vec<tl::enums::ChannelAdminLogEvent>,
    users: Vec<tl::enums::User>,
    max_id: i64,
}

struct CollectedAdminLog {
    events: Vec<tl::enums::ChannelAdminLogEvent>,
    users: HashMap<i64, tl::enums::User>,
}

async fn collect_admin_log<F, Fut>(limit: u32, mut fetch: F) -> TeleResult<CollectedAdminLog>
where
    F: FnMut(i64, u32) -> Fut,
    Fut: std::future::Future<Output = TeleResult<AdminLogPage>>,
{
    let mut events = Vec::new();
    let mut users: HashMap<i64, tl::enums::User> = HashMap::new();
    let mut max_id = 0i64;
    loop {
        let remaining = limit.saturating_sub(events.len() as u32);
        if remaining == 0 {
            break;
        }
        let page = fetch(max_id, remaining.min(100)).await?;
        max_id = page.max_id;
        let page_len = page.events.len();
        for u in page.users {
            if let tl::enums::User::User(uu) = &u {
                users.insert(uu.id, u);
            }
        }
        events.extend(page.events);
        if page_len == 0 {
            break;
        }
    }
    Ok(CollectedAdminLog { events, users })
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
            serde_json::json!({
                "kind": "change_title",
                "title": v.new_value,
                "prev_title": v.prev_value,
            })
        }
        tl::enums::ChannelAdminLogEventAction::ChangeAbout(v) => {
            serde_json::json!({
                "kind": "change_about",
                "text": v.new_value,
                "prev_text": v.prev_value,
            })
        }
        tl::enums::ChannelAdminLogEventAction::ChangeUsername(v) => {
            serde_json::json!({
                "kind": "change_username",
                "username": v.new_value,
                "prev_username": v.prev_value,
            })
        }
        tl::enums::ChannelAdminLogEventAction::SendMessage(v) => {
            message_action_summary("send_message", &v.message)
        }
        tl::enums::ChannelAdminLogEventAction::EditMessage(v) => serde_json::json!({
            "kind": "edit_message",
            "id": message_id(&v.new_message),
            "text": message_text(&v.new_message),
            "prev_text": message_text(&v.prev_message),
        }),
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
            let mut value = serde_json::json!({
                "kind": "toggle_ban",
                "user_id": participant_user_id(&v.new_participant, own_id),
            });
            if let Some(ban) = participant_ban_summary(&v.new_participant) {
                value["ban"] = ban;
            }
            if let Some(prev) = participant_ban_summary(&v.prev_participant) {
                value["prev_ban"] = prev;
            }
            value
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantToggleAdmin(v) => {
            let mut value = serde_json::json!({
                "kind": "toggle_admin",
                "user_id": participant_user_id(&v.new_participant, own_id),
            });
            if let Some(admin) = participant_admin_summary(&v.new_participant) {
                value["admin"] = admin;
            }
            if let Some(prev) = participant_admin_summary(&v.prev_participant) {
                value["prev_admin"] = prev;
            }
            value
        }
        tl::enums::ChannelAdminLogEventAction::ChangePhoto(v) => {
            serde_json::json!({
                "kind": "change_photo",
                "photo": photo_summary(&v.new_photo),
                "prev_photo": photo_summary(&v.prev_photo),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ToggleInvites(v) => {
            serde_json::json!({"kind": "toggle_invites", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleSignatures(v) => {
            serde_json::json!({"kind": "toggle_signatures", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::UpdatePinned(v) => {
            serde_json::json!({"kind": "update_pinned", "id": message_id(&v.message)})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantJoinByInvite(v) => {
            serde_json::json!({
                "kind": "join_by_invite",
                "invite_link": invite_link(&v.invite),
                "via_chatlist": v.via_chatlist,
            })
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantJoinByRequest(v) => {
            serde_json::json!({
                "kind": "join_by_request",
                "approved_by": v.approved_by,
                "invite_link": invite_link(&v.invite),
            })
        }
        tl::enums::ChannelAdminLogEventAction::TogglePreHistoryHidden(v) => {
            serde_json::json!({"kind": "toggle_pre_history_hidden", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleSlowMode(v) => {
            serde_json::json!({
                "kind": "toggle_slow_mode",
                "seconds": v.new_value,
                "prev_seconds": v.prev_value,
            })
        }
        tl::enums::ChannelAdminLogEventAction::ToggleNoForwards(v) => {
            serde_json::json!({"kind": "toggle_noforwards", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::DefaultBannedRights(v) => {
            serde_json::json!({
                "kind": "default_banned_rights",
                "rights": banned_rights_denied(&v.new_banned_rights),
                "until_date": banned_rights_until(&v.new_banned_rights),
                "prev_rights": banned_rights_denied(&v.prev_banned_rights),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ChangeLinkedChat(v) => {
            serde_json::json!({
                "kind": "change_linked_chat",
                "linked_chat_id": v.new_value,
                "prev_linked_chat_id": v.prev_value,
            })
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteDelete(v) => {
            serde_json::json!({
                "kind": "exported_invite_delete",
                "invite_link": invite_link_from_exported(&v.invite),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteRevoke(v) => {
            serde_json::json!({
                "kind": "exported_invite_revoke",
                "invite_link": invite_link_from_exported(&v.invite),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteEdit(v) => {
            serde_json::json!({
                "kind": "exported_invite_edit",
                "invite_link": invite_link_from_exported(&v.new_invite),
                "prev_invite_link": invite_link_from_exported(&v.prev_invite),
            })
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantEditRank(v) => {
            serde_json::json!({
                "kind": "edit_rank",
                "user_id": v.user_id,
                "rank": v.new_rank,
                "prev_rank": v.prev_rank,
            })
        }
        _ => serde_json::json!({"kind": "other"}),
    }
}

fn message_id(message: &tl::enums::Message) -> Option<i32> {
    match message {
        tl::enums::Message::Message(m) => Some(m.id),
        _ => None,
    }
}

fn message_text(message: &tl::enums::Message) -> String {
    match message {
        tl::enums::Message::Message(m) => m.message.clone(),
        _ => String::new(),
    }
}

fn invite_link(invite: &tl::enums::ExportedChatInvite) -> serde_json::Value {
    match invite {
        tl::enums::ExportedChatInvite::ChatInviteExported(i) => serde_json::json!(i.link),
        _ => serde_json::Value::Null,
    }
}

fn invite_link_from_exported(invite: &tl::enums::ExportedChatInvite) -> serde_json::Value {
    invite_link(invite)
}

fn photo_summary(photo: &tl::enums::Photo) -> serde_json::Value {
    match photo {
        tl::enums::Photo::Empty(p) => serde_json::json!({"empty": true, "id": p.id}),
        tl::enums::Photo::Photo(p) => serde_json::json!({
            "id": p.id,
            "date": rfc3339_or_empty(Some(p.date)),
            "sizes": p.sizes.len(),
        }),
    }
}

fn participant_ban_summary(
    participant: &tl::enums::ChannelParticipant,
) -> Option<serde_json::Value> {
    match participant {
        tl::enums::ChannelParticipant::Banned(p) => {
            let mut value = serde_json::json!({
                "left": p.left,
                "denied": banned_rights_denied(&p.banned_rights),
                "until_date": banned_rights_until(&p.banned_rights),
            });
            if p.rank.as_deref().is_some_and(|r| !r.is_empty()) {
                value["rank"] = serde_json::json!(p.rank);
            }
            Some(value)
        }
        _ => None,
    }
}

fn participant_admin_summary(
    participant: &tl::enums::ChannelParticipant,
) -> Option<serde_json::Value> {
    let (rights, rank) = match participant {
        tl::enums::ChannelParticipant::Creator(p) => (&p.admin_rights, p.rank.clone()),
        tl::enums::ChannelParticipant::Admin(p) => (&p.admin_rights, p.rank.clone()),
        _ => return None,
    };
    let mut value = serde_json::json!({"granted": admin_rights_granted(rights)});
    let tl::enums::ChatAdminRights::Rights(r) = rights;
    value["anonymous"] = serde_json::json!(r.anonymous);
    if let Some(rank) = rank.as_deref().filter(|r| !r.is_empty()) {
        value["rank"] = serde_json::json!(rank);
    }
    Some(value)
}

fn admin_rights_granted(rights: &tl::enums::ChatAdminRights) -> Vec<&'static str> {
    let tl::enums::ChatAdminRights::Rights(r) = rights;
    let mut granted = Vec::new();
    for (flag, name) in [
        (r.change_info, "change_info"),
        (r.post_messages, "post"),
        (r.edit_messages, "edit"),
        (r.delete_messages, "delete"),
        (r.ban_users, "ban"),
        (r.invite_users, "invite"),
        (r.pin_messages, "pin"),
        (r.add_admins, "add_admins"),
        (r.anonymous, "anonymous"),
        (r.manage_call, "manage_call"),
        (r.other, "other"),
        (r.manage_topics, "manage_topics"),
        (r.post_stories, "post_stories"),
        (r.edit_stories, "edit_stories"),
        (r.delete_stories, "delete_stories"),
        (r.manage_direct_messages, "manage_direct_messages"),
        (r.manage_ranks, "manage_ranks"),
    ] {
        if flag {
            granted.push(name);
        }
    }
    granted
}

fn banned_rights_denied(rights: &tl::enums::ChatBannedRights) -> Vec<&'static str> {
    let tl::enums::ChatBannedRights::Rights(r) = rights;
    let mut denied = Vec::new();
    for (flag, name) in [
        (r.view_messages, "view_messages"),
        (r.send_messages, "send_messages"),
        (r.send_media, "send_media"),
        (r.send_stickers, "send_stickers"),
        (r.send_gifs, "send_gifs"),
        (r.send_games, "send_games"),
        (r.send_inline, "send_inline"),
        (r.embed_links, "embed_links"),
        (r.send_polls, "send_polls"),
        (r.change_info, "change_info"),
        (r.invite_users, "invite_users"),
        (r.pin_messages, "pin_messages"),
        (r.manage_topics, "manage_topics"),
        (r.send_photos, "send_photos"),
        (r.send_videos, "send_videos"),
        (r.send_roundvideos, "send_roundvideos"),
        (r.send_audios, "send_audios"),
        (r.send_voices, "send_voices"),
        (r.send_docs, "send_docs"),
        (r.send_plain, "send_plain"),
        (r.edit_rank, "edit_rank"),
        (r.send_reactions, "send_reactions"),
    ] {
        if flag {
            denied.push(name);
        }
    }
    denied
}

fn banned_rights_until(rights: &tl::enums::ChatBannedRights) -> Option<i32> {
    let tl::enums::ChatBannedRights::Rights(r) = rights;
    (r.until_date > 0).then_some(r.until_date)
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
        .or_else(|| action.get("invite_link"))
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

    #[test]
    fn event_rows_carry_actor_names_from_response_users() {
        let client = offline_client();
        let mut users = HashMap::new();
        users.insert(11, test_user(11, "alice"));
        users.insert(22, test_user(22, "bob"));
        let actor = actor_value(&client, &users, 11);
        assert_eq!(actor["id"], 11);
        assert_eq!(actor["name"], "alice");
        assert_eq!(actor_value(&client, &users, 99)["name"], "99");
    }

    #[tokio::test]
    async fn collect_admin_log_accumulates_users_across_pages() {
        let pages: [(i64, u32, Vec<i64>, Vec<i64>); 2] =
            [(0, 2, vec![9], vec![11]), (9, 1, vec![8], Vec::new())];
        let mut next = 0usize;
        let collected = collect_admin_log(2, |_max_id, _limit| {
            let (_want_max, _want_limit, ids, user_ids) = pages[next].clone();
            next += 1;
            let new_max = *ids.last().unwrap_or(&0);
            async move {
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    users: user_ids.into_iter().map(test_user_id).collect(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(collected.events.len(), 2);
        assert!(collected.users.contains_key(&11));
    }

    fn test_user_id(id: i64) -> tl::enums::User {
        test_user(id, "u")
    }

    fn admin_log_args(chat: &str) -> AdminLogArgs {
        AdminLogArgs {
            chat: chat.to_string(),
            limit: 20,
            admin: None,
            search: None,
            since: None,
            until: None,
            events: None,
        }
    }

    #[test]
    fn admin_events_filter_maps_csv_to_flags() {
        let filter = parse_admin_events_filter(Some("ban, promote, edit_rank"))
            .unwrap()
            .expect("filter");
        let tl::enums::ChannelAdminLogEventsFilter::Filter(f) = &filter;
        assert!(f.ban && f.promote && f.edit_rank);
        assert!(!f.join && !f.delete && !f.send);
    }

    #[test]
    fn admin_events_filter_rejects_unknown_empty_and_none() {
        assert!(parse_admin_events_filter(None).unwrap().is_none());
        for bad in ["", "  ", "fly", "ban,fly", "Ban"] {
            assert!(
                matches!(
                    parse_admin_events_filter(Some(bad)),
                    Err(TeleError::Usage(_))
                ),
                "events '{bad}' should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn admin_log_validates_since_until_and_flags_offline() {
        let mut a = admin_log_args("@c");
        a.since = Some("not-a-date".to_string());
        assert!(matches!(
            admin_log(a, &dryrun_flags("chat admin-log")).await,
            Err(TeleError::Usage(_))
        ));

        let mut a = admin_log_args("@c");
        a.events = Some("nope".to_string());
        assert!(matches!(
            admin_log(a, &dryrun_flags("chat admin-log")).await,
            Err(TeleError::Usage(_))
        ));

        let mut a = admin_log_args("@c");
        a.since = Some("200".into());
        a.until = Some("100".into());
        assert!(matches!(
            admin_log(a, &dryrun_flags("chat admin-log")).await,
            Err(TeleError::Usage(_))
        ));

        let mut a = admin_log_args("@c");
        a.since = Some("1000000000".into());
        a.until = Some("2000000000".into());
        a.search = Some("spam".into());
        a.admin = Some("me".into());
        a.events = Some("join,leave".into());
        assert!(admin_log(a, &dryrun_flags("chat admin-log")).await.is_ok());
    }

    #[test]
    fn admin_log_dry_run_payload_echoes_filters() {
        let v = admin_log_dry_run_payload("@c", "q", true, true);
        assert_eq!(v["search"], serde_json::json!("q"));
        assert_eq!(v["events_filter"], serde_json::json!(true));
        assert_eq!(v["admins"], serde_json::json!(true));
        assert_eq!(v["chat"], serde_json::json!("@c"));
        let none = admin_log_dry_run_payload("@c", "", false, false);
        assert_eq!(none["search"], serde_json::Value::Null);
        assert_eq!(none["events_filter"], serde_json::json!(false));
        assert_eq!(none["admins"], serde_json::json!(false));
    }

    #[test]
    fn event_rows_filter_by_since_until_range() {
        let row_at = |ts: &str| serde_json::json!({"id": 1, "date": ts});
        let since = crate::commands::parse_unixtime("150").ok();
        let until = crate::commands::parse_unixtime("250").ok();
        let rows = vec![
            row_at("1970-01-01T00:02:30+00:00"),
            row_at("1970-01-01T00:04:10+00:00"),
        ];
        let kept = filter_events_by_range(rows, since, until);
        assert_eq!(kept.len(), 2);

        let strict_since = crate::commands::parse_unixtime("160").ok();
        let rows = vec![row_at("1970-01-01T00:02:30+00:00")];
        assert!(filter_events_by_range(rows, strict_since, None).is_empty());

        let rows = vec![row_at("bogus")];
        assert_eq!(filter_events_by_range(rows, strict_since, None).len(), 1);
    }

    #[test]
    fn admin_action_summary_reports_old_and_new_values() {
        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ChangeTitle(
                tl::types::ChannelAdminLogEventActionChangeTitle {
                    prev_value: "Old".into(),
                    new_value: "New".into(),
                },
            ),
            0,
        );
        assert_eq!(action["kind"], "change_title");
        assert_eq!(action["title"], "New");
        assert_eq!(action["prev_title"], "Old");

        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ChangeAbout(
                tl::types::ChannelAdminLogEventActionChangeAbout {
                    prev_value: String::new(),
                    new_value: "about".into(),
                },
            ),
            0,
        );
        assert_eq!(action["prev_text"], "");
        assert_eq!(action["text"], "about");

        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ChangeUsername(
                tl::types::ChannelAdminLogEventActionChangeUsername {
                    prev_value: "old_handle".into(),
                    new_value: "new_handle".into(),
                },
            ),
            0,
        );
        assert_eq!(action["prev_username"], "old_handle");
        assert_eq!(action["username"], "new_handle");
    }

    #[test]
    fn admin_action_summary_reports_ban_until_and_rights() {
        let tl::enums::ChatBannedRights::Rights(mut r) = banned_rights();
        r.view_messages = true;
        r.send_messages = true;
        r.until_date = 12345;
        let rights = tl::enums::ChatBannedRights::Rights(r);
        let banned = tl::enums::ChannelParticipant::Banned(tl::types::ChannelParticipantBanned {
            left: false,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 404 }),
            kicked_by: 1,
            date: 0,
            banned_rights: rights.clone(),
            rank: Some("spam".into()),
        });
        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ParticipantToggleBan(
                tl::types::ChannelAdminLogEventActionParticipantToggleBan {
                    prev_participant: banned_rights_fixture_participant(),
                    new_participant: banned,
                },
            ),
            0,
        );
        assert_eq!(action["kind"], "toggle_ban");
        assert_eq!(action["user_id"], 404);
        assert_eq!(
            action["ban"]["denied"],
            serde_json::json!(["view_messages", "send_messages"])
        );
        assert_eq!(action["ban"]["until_date"], 12345);
        assert_eq!(action["ban"]["rank"], "spam");
        assert_eq!(action["prev_ban"]["denied"], serde_json::json!([]));
    }

    fn banned_rights_fixture_participant() -> tl::enums::ChannelParticipant {
        tl::enums::ChannelParticipant::Banned(tl::types::ChannelParticipantBanned {
            left: true,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 404 }),
            kicked_by: 1,
            date: 0,
            banned_rights: banned_rights(),
            rank: None,
        })
    }

    #[test]
    fn admin_action_summary_reports_admin_rights_detail() {
        let tl::enums::ChatAdminRights::Rights(mut r) = admin_rights();
        r.ban_users = true;
        r.pin_messages = true;
        let rights = tl::enums::ChatAdminRights::Rights(r);
        let admin = tl::enums::ChannelParticipant::Admin(tl::types::ChannelParticipantAdmin {
            can_edit: false,
            is_self: false,
            user_id: 303,
            inviter_id: None,
            promoted_by: 1,
            date: 0,
            admin_rights: rights,
            rank: Some("Mod".into()),
        });
        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ParticipantToggleAdmin(
                tl::types::ChannelAdminLogEventActionParticipantToggleAdmin {
                    prev_participant: banned_rights_fixture_participant(),
                    new_participant: admin,
                },
            ),
            0,
        );
        assert_eq!(action["kind"], "toggle_admin");
        assert_eq!(
            action["admin"]["granted"],
            serde_json::json!(["ban", "pin"])
        );
        assert_eq!(action["admin"]["anonymous"], false);
        assert_eq!(action["admin"]["rank"], "Mod");
    }

    #[test]
    fn admin_action_summary_reports_photo_pinned_and_invite_link() {
        let photo = tl::enums::Photo::Photo(tl::types::Photo {
            id: 555,
            access_hash: -7,
            file_reference: vec![1],
            has_stickers: false,
            date: 0,
            sizes: Vec::new(),
            video_sizes: None,
            dc_id: 1,
        });
        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::ChangePhoto(
                tl::types::ChannelAdminLogEventActionChangePhoto {
                    prev_photo: tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 }),
                    new_photo: photo,
                },
            ),
            0,
        );
        assert_eq!(action["photo"]["id"], 555);
        assert_eq!(action["photo"]["sizes"], 0);
        assert_eq!(action["prev_photo"]["empty"], true);

        let pinned_msg = test_tl_message(77);
        let action = admin_action_summary(
            &tl::enums::ChannelAdminLogEventAction::UpdatePinned(
                tl::types::ChannelAdminLogEventActionUpdatePinned {
                    message: pinned_msg,
                },
            ),
            0,
        );
        assert_eq!(action["kind"], "update_pinned");
        assert_eq!(action["id"], 77);

        let join_invite = tl::enums::ChannelAdminLogEventAction::ParticipantJoinByInvite(
            tl::types::ChannelAdminLogEventActionParticipantJoinByInvite {
                via_chatlist: false,
                invite: tl::enums::ExportedChatInvite::ChatInviteExported(exported_link_fixture()),
            },
        );
        let action = admin_action_summary(&join_invite, 0);
        assert_eq!(action["kind"], "join_by_invite");
        assert_eq!(action["invite_link"], "https://t.me/+abcdef");
    }

    fn test_tl_message(id: i32) -> tl::enums::Message {
        tl::enums::Message::Message(tl::types::Message {
            out: false,
            mentioned: false,
            media_unread: false,
            silent: false,
            post: false,
            from_scheduled: false,
            legacy: false,
            edit_hide: false,
            pinned: false,
            noforwards: false,
            invert_media: false,
            offline: false,
            video_processing_pending: false,
            paid_suggested_post_stars: false,
            paid_suggested_post_ton: false,
            id,
            from_id: None,
            from_boosts_applied: None,
            from_rank: None,
            peer_id: tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 1 }),
            saved_peer_id: None,
            fwd_from: None,
            via_bot_id: None,
            via_business_bot_id: None,
            guestchat_via_from: None,
            reply_to: None,
            date: 0,
            message: String::new(),
            media: None,
            reply_markup: None,
            entities: None,
            views: None,
            forwards: None,
            replies: None,
            edit_date: None,
            post_author: None,
            grouped_id: None,
            reactions: None,
            restriction_reason: None,
            ttl_period: None,
            quick_reply_shortcut_id: None,
            effect: None,
            factcheck: None,
            report_delivery_until_date: None,
            paid_message_stars: None,
            suggested_post: None,
            schedule_repeat_period: None,
            summary_from_language: None,
            rich_message: None,
        })
    }

    #[tokio::test]
    async fn collect_admin_log_stops_on_empty_page() {
        let mut calls = Vec::new();
        let collected = collect_admin_log(10, |max_id, page_limit| {
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: Vec::new(),
                    users: Vec::new(),
                    max_id: 0,
                })
            }
        })
        .await
        .unwrap();
        assert!(collected.events.is_empty());
        assert_eq!(calls, vec![(0, 10)]);
    }

    #[tokio::test]
    async fn collect_admin_log_probes_after_partial_page() {
        let pages: [(i64, u32, Vec<i64>, i64); 2] = [(0, 5, vec![10, 9], 9), (9, 3, Vec::new(), 0)];
        let mut next = 0usize;
        let mut calls = Vec::new();
        let collected = collect_admin_log(5, |max_id, page_limit| {
            let (want_max, want_limit, ids, new_max) = pages[next].clone();
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                assert_eq!(max_id, want_max);
                assert_eq!(page_limit, want_limit);
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    users: Vec::new(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(collected.events.len(), 2);
        assert_eq!(calls, vec![(0, 5), (9, 3)]);
    }

    #[tokio::test]
    async fn collect_admin_log_paginates_until_limit() {
        let pages: [(i64, u32, Vec<i64>, i64); 2] =
            [(0, 5, vec![10, 9, 8], 8), (8, 2, vec![7, 6], 6)];
        let mut next = 0usize;
        let mut calls = Vec::new();
        let collected = collect_admin_log(5, |max_id, page_limit| {
            let (want_max, want_limit, ids, new_max) = pages[next].clone();
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                assert_eq!(max_id, want_max);
                assert_eq!(page_limit, want_limit);
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    users: Vec::new(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(collected.events.len(), 5);
        assert_eq!(calls, vec![(0, 5), (8, 2)]);
    }

    #[tokio::test]
    async fn collect_admin_log_stops_when_limit_reached_exactly() {
        let mut calls = Vec::new();
        let collected = collect_admin_log(3, |max_id, page_limit| {
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: vec![fake_event(7), fake_event(6), fake_event(5)],
                    users: Vec::new(),
                    max_id: 5,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(collected.events.len(), 3);
        assert_eq!(calls, vec![(0, 3)]);
    }

    #[tokio::test]
    async fn collect_admin_log_page_size_capped_at_100() {
        let mut next = 0usize;
        let mut calls = Vec::new();
        let collected = collect_admin_log(250, |max_id, page_limit| {
            let ids: Vec<i64> = (0..page_limit)
                .map(|i| 1000 - next as i64 * 100 - i as i64)
                .collect();
            let new_max = ids.last().copied().unwrap_or(0);
            next += 1;
            calls.push((max_id, page_limit));
            async move {
                Ok(AdminLogPage {
                    events: ids.into_iter().map(fake_event).collect(),
                    users: Vec::new(),
                    max_id: new_max,
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(collected.events.len(), 250);
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
                    user: Some("u".to_string()),
                    expire: None,
                    usage_limit: None,
                    request_approval: None,
                    title: None,
                    list: false,
                    revoked: false,
                    importers: None,
                    edit: None,
                    revoke: false,
                    delete_revoked: false,
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
                    role: None,
                    search: None,
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
                    ban: false,
                    duration: None,
                    rights: None,
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
                    admin: None,
                    search: None,
                    since: None,
                    until: None,
                    events: None,
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

    fn invite_args(chat: &str) -> InviteArgs {
        InviteArgs {
            chat: chat.to_string(),
            user: None,
            expire: None,
            usage_limit: None,
            request_approval: None,
            title: None,
            list: false,
            revoked: false,
            importers: None,
            edit: None,
            revoke: false,
            delete_revoked: false,
        }
    }

    #[test]
    fn invite_mode_matrix_routes_each_flag_combination() {
        let mut a = invite_args("@c");
        assert_eq!(validate_invite(&a).unwrap().mode, InviteMode::Export);

        a.user = Some("@bob".to_string());
        assert_eq!(validate_invite(&a).unwrap().mode, InviteMode::User);

        a = invite_args("@c");
        a.list = true;
        assert_eq!(validate_invite(&a).unwrap().mode, InviteMode::List);

        a.revoked = true;
        let plan = validate_invite(&a).unwrap();
        assert_eq!(plan.mode, InviteMode::List);
        assert!(plan.revoked);

        a = invite_args("@c");
        a.list = true;
        a.importers = Some("t.me/+abc123".to_string());
        let plan = validate_invite(&a).unwrap();
        assert_eq!(plan.mode, InviteMode::List);
        assert_eq!(plan.link.as_deref(), Some("https://t.me/+abc123"));

        a = invite_args("@c");
        a.edit = Some("+abc123".to_string());
        a.revoke = true;
        let plan = validate_invite(&a).unwrap();
        assert_eq!(plan.mode, InviteMode::Edit);
        assert!(plan.revoked);

        a = invite_args("@c");
        a.delete_revoked = true;
        assert_eq!(validate_invite(&a).unwrap().mode, InviteMode::DeleteRevoked);
    }

    #[test]
    fn invite_rejects_conflicting_modes_and_misplaced_options() {
        let conflict = |mutate: &dyn Fn(&mut InviteArgs)| {
            let mut a = invite_args("@c");
            mutate(&mut a);
            assert!(
                matches!(validate_invite(&a), Err(TeleError::Usage(_))),
                "expected Usage error"
            );
        };
        conflict(&|a| {
            a.user = Some("u".into());
            a.list = true;
        });
        conflict(&|a| {
            a.user = Some("u".into());
            a.edit = Some("+abc".into());
        });
        conflict(&|a| {
            a.list = true;
            a.edit = Some("+abc".into());
        });
        conflict(&|a| {
            a.list = true;
            a.delete_revoked = true;
        });
        conflict(&|a| {
            a.revoke = true;
        });
        conflict(&|a| {
            a.importers = Some("+abc".into());
        });
        conflict(&|a| {
            a.revoked = true;
        });
        conflict(&|a| {
            a.importers = Some("+abc".into());
            a.list = true;
            a.revoked = true;
        });
        conflict(&|a| {
            a.title = Some("t".into());
            a.user = Some("u".into());
        });
        conflict(&|a| {
            a.expire = Some("1h".into());
            a.delete_revoked = true;
        });
        conflict(&|a| {
            a.usage_limit = Some(5);
            a.list = true;
        });
    }

    #[test]
    fn invite_edit_requires_a_change() {
        let mut a = invite_args("@c");
        a.edit = Some("+abc123".to_string());
        assert!(matches!(validate_invite(&a), Err(TeleError::Usage(_))));
        a.request_approval = Some("true".to_string());
        assert!(validate_invite(&a).is_ok());
    }

    #[test]
    fn invite_option_values_validate_offline() {
        let bad = |mutate: &dyn Fn(&mut InviteArgs)| {
            let mut a = invite_args("@c");
            mutate(&mut a);
            assert!(
                matches!(validate_invite(&a), Err(TeleError::Usage(_))),
                "expected Usage error"
            );
        };
        bad(&|a| a.title = Some("   ".into()));
        bad(&|a| a.usage_limit = Some(0));
        bad(&|a| a.request_approval = Some("yes".into()));
        bad(&|a| a.expire = Some("next tuesday".into()));
        bad(&|a| a.edit = Some("@notalink".into()));

        let mut a = invite_args("@c");
        a.title = Some("  Weekly link ".to_string());
        a.usage_limit = Some(7);
        a.request_approval = Some("false".to_string());
        a.expire = Some("2100000000".to_string());
        let plan = validate_invite(&a).unwrap();
        assert_eq!(plan.title.as_deref(), Some("Weekly link"));
        assert_eq!(plan.usage_limit, Some(7));
        assert_eq!(plan.request_needed, Some(false));
        assert_eq!(plan.expire_date, Some(2_100_000_000));
    }

    #[test]
    fn invite_expire_parses_ts_rfc3339_and_durations() {
        let now: i64 = 1_700_000_000;
        assert_eq!(
            parse_invite_expire_at(now, "1750000000").unwrap(),
            1_750_000_000
        );
        assert_eq!(
            parse_invite_expire_at(now, "2035-01-01T00:00:00Z").unwrap(),
            2_051_222_400
        );
        assert_eq!(
            parse_invite_expire_at(now, "90s").unwrap(),
            (now + 90) as i32
        );
        assert_eq!(
            parse_invite_expire_at(now, "30m").unwrap(),
            (now + 1800) as i32
        );
        assert_eq!(
            parse_invite_expire_at(now, "24h").unwrap(),
            (now + 86_400) as i32
        );
        assert_eq!(
            parse_invite_expire_at(now, "7d").unwrap(),
            (now + 604_800) as i32
        );
        assert_eq!(
            parse_invite_expire_at(now, "2w").unwrap(),
            (now + 1_209_600) as i32
        );
        for bad in ["", "abc", "5x", "-3h", "1.5h", "99999999999999999999w"] {
            assert!(
                matches!(parse_invite_expire_at(now, bad), Err(TeleError::Usage(_))),
                "expire '{bad}' should be rejected"
            );
        }
        for past in ["1690000000", "2020-01-01T00:00:00Z", "0s"] {
            assert!(
                matches!(parse_invite_expire_at(now, past), Err(TeleError::Usage(_))),
                "past expire '{past}' should be rejected"
            );
        }
    }

    fn exported_link_fixture() -> tl::types::ChatInviteExported {
        tl::types::ChatInviteExported {
            revoked: false,
            permanent: false,
            request_needed: true,
            link: "https://t.me/+abcdef".to_string(),
            admin_id: 5,
            date: 1_700_000_000,
            start_date: None,
            expire_date: Some(1_790_000_000),
            usage_limit: Some(10),
            usage: Some(3),
            requested: Some(1),
            subscription_expired: None,
            title: Some("Team".to_string()),
            subscription_pricing: None,
        }
    }

    #[test]
    fn invite_link_row_carries_full_shape() {
        let row = exported_invite_row(&tl::enums::ExportedChatInvite::ChatInviteExported(
            exported_link_fixture(),
        ));
        assert_eq!(row["link"], "https://t.me/+abcdef");
        assert_eq!(row["title"], "Team");
        assert_eq!(row["revoked"], false);
        assert_eq!(row["request_needed"], true);
        assert_eq!(row["usage_limit"], 10);
        assert_eq!(row["usage"], 3);
        assert_eq!(row["requested"], 1);
        assert_eq!(row["expire_date"], 1790000000);
        assert_eq!(row["date"], "2023-11-14T22:13:20+00:00");
        let revoked_row =
            exported_invite_row(&tl::enums::ExportedChatInvite::ChatInvitePublicJoinRequests);
        assert_eq!(revoked_row["public_join_requests"], true);
    }

    #[test]
    fn exported_invite_result_rows_handles_replacement() {
        let replaced = tl::enums::messages::ExportedChatInvite::Replaced(
            tl::types::messages::ExportedChatInviteReplaced {
                invite: tl::enums::ExportedChatInvite::ChatInviteExported(exported_link_fixture()),
                new_invite: tl::enums::ExportedChatInvite::ChatInvitePublicJoinRequests,
                users: Vec::new(),
            },
        );
        let rows = exported_invite_result_rows(&replaced);
        assert_eq!(rows.len(), 2);
        let single = tl::enums::messages::ExportedChatInvite::Invite(
            tl::types::messages::ExportedChatInvite {
                invite: tl::enums::ExportedChatInvite::ChatInviteExported(exported_link_fixture()),
                users: Vec::new(),
            },
        );
        assert_eq!(exported_invite_result_rows(&single).len(), 1);
    }

    #[test]
    fn importer_rows_resolve_names_with_numeric_fallback() {
        let client = offline_client();
        let importers = tl::enums::messages::ChatInviteImporters::Importers(
            tl::types::messages::ChatInviteImporters {
                count: 2,
                importers: vec![
                    tl::enums::ChatInviteImporter::Importer(tl::types::ChatInviteImporter {
                        requested: false,
                        via_chatlist: false,
                        user_id: 11,
                        date: 1_700_000_000,
                        about: None,
                        approved_by: Some(5),
                    }),
                    tl::enums::ChatInviteImporter::Importer(tl::types::ChatInviteImporter {
                        requested: true,
                        via_chatlist: false,
                        user_id: 99,
                        date: 1_700_000_100,
                        about: None,
                        approved_by: None,
                    }),
                ],
                users: vec![test_user(11, "alice")],
            },
        );
        let rows = chat_invite_importers_rows(&client, &importers);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], 11);
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[0]["requested"], false);
        assert_eq!(rows[0]["approved_by"], 5);
        assert_eq!(rows[1]["id"], 99);
        assert_eq!(rows[1]["name"], "99");
        assert_eq!(rows[1]["requested"], true);
    }

    #[test]
    fn invite_dry_run_payloads_carry_modes_and_echoes() {
        let target = "@c";
        let mut plan = ValidatedInvite {
            mode: InviteMode::User,
            user: Some("@bob".to_string()),
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(v["user"], serde_json::json!("@bob"));
        assert_eq!(v["would"], serde_json::json!("invite user @bob to chat @c"));

        plan = ValidatedInvite {
            mode: InviteMode::Export,
            title: Some("Weekly".to_string()),
            expire_date: Some(123456),
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(v["mode"], serde_json::json!("export"));
        assert_eq!(v["title"], serde_json::json!("Weekly"));
        assert_eq!(v["expire_date"], serde_json::json!(123456));
        assert!(v.get("usage_limit").is_none());

        plan = ValidatedInvite {
            mode: InviteMode::List,
            revoked: true,
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(v["mode"], serde_json::json!("list"));
        assert_eq!(v["revoked"], serde_json::json!(true));
        assert!(v["would"]
            .as_str()
            .unwrap()
            .contains("revoked invite links"));

        plan = ValidatedInvite {
            mode: InviteMode::List,
            link: Some("https://t.me/+x".to_string()),
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(
            v["would"],
            serde_json::json!("list who joined link https://t.me/+x in chat @c")
        );

        plan = ValidatedInvite {
            mode: InviteMode::Edit,
            link: Some("https://t.me/+x".to_string()),
            revoked: true,
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(v["mode"], serde_json::json!("edit"));
        assert_eq!(v["revoke"], serde_json::json!(true));
        assert!(v["would"].as_str().unwrap().starts_with("revoke"));

        plan = ValidatedInvite {
            mode: InviteMode::DeleteRevoked,
            ..Default::default()
        };
        let v = invite_dry_run_payload(target, &plan);
        assert_eq!(v["mode"], serde_json::json!("delete_revoked"));
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

    #[test]
    fn parse_participant_role_accepts_known_roles() {
        assert_eq!(
            parse_participant_role(Some("admin")).unwrap(),
            Some(ParticipantRole::Admin)
        );
        assert_eq!(
            parse_participant_role(Some("banned")).unwrap(),
            Some(ParticipantRole::Banned)
        );
        assert_eq!(
            parse_participant_role(Some("kicked")).unwrap(),
            Some(ParticipantRole::Kicked)
        );
        assert_eq!(
            parse_participant_role(Some("recent")).unwrap(),
            Some(ParticipantRole::Recent)
        );
        assert_eq!(parse_participant_role(None).unwrap(), None);
    }

    #[test]
    fn parse_participant_role_rejects_unknown_and_case_mismatch() {
        for bad in ["Admin", "ADMIN", "owner", "member", ""] {
            assert!(
                matches!(parse_participant_role(Some(bad)), Err(TeleError::Usage(_))),
                "role {bad} should be rejected"
            );
        }
    }

    #[test]
    fn participant_filter_maps_roles() {
        use tl::enums::ChannelParticipantsFilter as F;
        let role = |r| parse_participant_role(Some(r)).unwrap();
        assert!(matches!(
            participant_filter(role("admin"), None),
            F::ChannelParticipantsAdmins
        ));
        assert!(matches!(
            participant_filter(role("recent"), None),
            F::ChannelParticipantsRecent
        ));
        assert!(matches!(
            participant_filter(None, None),
            F::ChannelParticipantsRecent
        ));
        match participant_filter(role("banned"), Some("spam")) {
            F::ChannelParticipantsBanned(f) => assert_eq!(f.q, "spam"),
            other => panic!("unexpected filter {other:?}"),
        }
        match participant_filter(role("kicked"), None) {
            F::ChannelParticipantsKicked(f) => assert_eq!(f.q, ""),
            other => panic!("unexpected filter {other:?}"),
        }
    }

    #[test]
    fn participant_filter_search_without_role_uses_search_filter() {
        use tl::enums::ChannelParticipantsFilter as F;
        match participant_filter(None, Some("ali")) {
            F::ChannelParticipantsSearch(f) => assert_eq!(f.q, "ali"),
            other => panic!("unexpected filter {other:?}"),
        }
        assert!(matches!(
            participant_filter(Some(ParticipantRole::Recent), Some("")),
            F::ChannelParticipantsRecent
        ));
    }

    #[test]
    fn admin_rights_csv_accepts_new_rights() {
        let rights = AdminRights::from_string("anonymous,other,manage_topics").unwrap();
        assert!(rights.anonymous);
        assert!(rights.other);
        assert!(rights.manage_topics);
        assert!(rights.needs_raw_edit_admin());
        assert!(!rights.change_info);
    }

    #[test]
    fn admin_rights_csv_rejects_unknown_right() {
        let err = AdminRights::from_string("ban,fly").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.to_string().contains("manage_topics"));
    }

    #[test]
    fn admin_presets_carry_new_rights() {
        let admin = AdminRights::all();
        assert!(admin.other && admin.manage_topics && !admin.anonymous);
        assert!(admin.needs_raw_edit_admin());
        let moderator = AdminRights::moderator();
        assert!(moderator.manage_topics && !moderator.other && !moderator.anonymous);
        assert!(moderator.needs_raw_edit_admin());
        let editor = AdminRights::editor();
        assert!(editor.manage_topics && !editor.ban_users);
        assert!(grants_nothing(&AdminRights::none()));
    }

    fn grants_nothing(rights: &AdminRights) -> bool {
        !(rights.change_info
            || rights.post_messages
            || rights.edit_messages
            || rights.delete_messages
            || rights.ban_users
            || rights.invite_users
            || rights.pin_messages
            || rights.add_admins
            || rights.manage_call
            || rights.anonymous
            || rights.other
            || rights.manage_topics)
    }

    #[test]
    fn resolve_admin_rights_demote_grants_nothing() {
        let args = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: false,
            demote: true,
            title: None,
            preset: Some("admin".to_string()),
            rights: Some("ban".to_string()),
        };
        assert!(grants_nothing(&resolve_admin_rights(&args).unwrap()));
    }

    #[test]
    fn ban_duration_parses_secs_and_forever() {
        assert_eq!(parse_ban_duration(None).unwrap(), None);
        assert_eq!(parse_ban_duration(Some("forever")).unwrap(), None);
        assert_eq!(parse_ban_duration(Some("3600")).unwrap(), Some(3600));
        assert_eq!(parse_ban_duration(Some("60")).unwrap(), Some(60));
    }

    #[test]
    fn ban_duration_rejects_garbage_zero_negative_overflow() {
        for bad in ["", "abc", "-5", "0", "1.5", "99999999999"] {
            assert!(
                matches!(parse_ban_duration(Some(bad)), Err(TeleError::Usage(_))),
                "duration {bad} should be rejected"
            );
        }
    }

    #[test]
    fn banned_rights_csv_maps_names_and_values() {
        let entries = parse_banned_rights_csv("send_stickers:false, invite_users:true").unwrap();
        assert_eq!(
            entries,
            vec![
                ("send_stickers".to_string(), false),
                ("invite_users".to_string(), true)
            ]
        );
    }

    #[test]
    fn banned_rights_csv_normalizes_embed_links_alias() {
        let entries = parse_banned_rights_csv("embed_links:false").unwrap();
        assert_eq!(entries, vec![("embed_links".to_string(), false)]);
    }

    #[test]
    fn banned_rights_csv_rejects_bad_entries() {
        for bad in [
            "send_stickers",
            "send_stickers:maybe",
            "fly:false",
            ":false",
            "send_stickers:",
        ] {
            assert!(
                matches!(parse_banned_rights_csv(bad), Err(TeleError::Usage(_))),
                "csv '{bad}' should be rejected"
            );
        }
    }

    #[test]
    fn kick_duration_requires_ban_or_rights() {
        let base = |duration: Option<String>, ban: bool, rights: Option<String>| KickArgs {
            chat: "@c".to_string(),
            user: "@u".to_string(),
            ban,
            duration,
            rights,
        };
        assert!(matches!(
            validate_kick(&base(Some("60".to_string()), false, None)),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_kick(&base(Some("60".to_string()), true, None)).is_ok());
        assert!(validate_kick(&base(
            Some("60".to_string()),
            false,
            Some("send_stickers:false".to_string())
        ))
        .is_ok());
        assert!(validate_kick(&base(None, false, None)).is_ok());
        assert!(validate_kick(&base(None, true, None)).is_ok());
        assert!(matches!(
            validate_kick(&base(Some("nope".to_string()), true, None)),
            Err(TeleError::Usage(_))
        ));
    }

    fn settings_args(chat: &str) -> SettingsArgs {
        SettingsArgs {
            chat: chat.to_string(),
            slow_mode: None,
            noforwards: None,
            signatures: None,
            pre_history: None,
            join_request: None,
        }
    }

    #[test]
    fn on_off_values_parse_strictly() {
        assert_eq!(parse_on_off(None).unwrap(), None);
        assert_eq!(parse_on_off(Some("on")).unwrap(), Some(true));
        assert_eq!(parse_on_off(Some("off")).unwrap(), Some(false));
        for bad in ["", "true", "yes", "On", "OFF"] {
            assert!(
                matches!(parse_on_off(Some(bad)), Err(TeleError::Usage(_))),
                "on/off value {bad} should be rejected"
            );
        }
    }

    #[test]
    fn slow_mode_parses_secs_and_off_with_range_check() {
        assert_eq!(parse_slow_mode(None).unwrap(), None);
        assert_eq!(parse_slow_mode(Some("off")).unwrap(), Some(0));
        assert_eq!(parse_slow_mode(Some("0")).unwrap(), Some(0));
        assert_eq!(parse_slow_mode(Some("3600")).unwrap(), Some(3600));
        for bad in ["", "abc", "-1", "3601", "99999999999", "1.5"] {
            assert!(
                matches!(parse_slow_mode(Some(bad)), Err(TeleError::Usage(_))),
                "slow mode {bad} should be rejected"
            );
        }
    }

    #[test]
    fn settings_validation_rejects_empty_chat_and_bad_values() {
        let mut args = settings_args("");
        assert!(matches!(validate_settings(&args), Err(TeleError::Usage(_))));
        args = settings_args("@chat");
        args.slow_mode = Some("nope".to_string());
        assert!(matches!(validate_settings(&args), Err(TeleError::Usage(_))));
        args.slow_mode = Some("30".to_string());
        assert!(validate_settings(&args).is_ok());
    }

    #[test]
    fn settings_noforwards_toggle_is_rejected_as_unavailable() {
        let mut args = settings_args("@chat");
        args.noforwards = Some("on".to_string());
        let err = validate_settings(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.to_string().contains("--noforwards"));
        args.noforwards = Some("off".to_string());
        assert!(matches!(validate_settings(&args), Err(TeleError::Usage(_))));
        args.noforwards = None;
        assert!(validate_settings(&args).is_ok());
    }

    #[test]
    fn settings_all_toggles_validate_before_connect() {
        let mut args = settings_args("@chat");
        args.signatures = Some("off".to_string());
        args.pre_history = Some("on".to_string());
        args.join_request = Some("on".to_string());
        args.slow_mode = Some("60".to_string());
        assert!(validate_settings(&args).is_ok());
    }

    #[test]
    fn channel_from_chats_matches_by_id_only_for_channels() {
        let chats = vec![
            tl::enums::Chat::Chat(tl::types::Chat {
                creator: false,
                left: false,
                deactivated: false,
                call_active: false,
                call_not_empty: false,
                noforwards: false,
                id: 77,
                title: "basic".to_string(),
                photo: tl::enums::ChatPhoto::Empty,
                participants_count: 0,
                date: 0,
                version: 0,
                migrated_to: None,
                admin_rights: None,
                default_banned_rights: None,
            }),
            tl::enums::Chat::Channel(tl::types::Channel {
                creator: false,
                left: false,
                broadcast: true,
                verified: false,
                megagroup: false,
                restricted: false,
                signatures: true,
                min: false,
                scam: false,
                has_link: false,
                has_geo: false,
                slowmode_enabled: false,
                call_active: false,
                call_not_empty: false,
                fake: false,
                gigagroup: false,
                noforwards: false,
                join_to_send: false,
                join_request: false,
                forum: false,
                stories_hidden: false,
                stories_hidden_min: false,
                stories_unavailable: false,
                signature_profiles: false,
                autotranslation: false,
                broadcast_messages_allowed: false,
                monoforum: false,
                forum_tabs: false,
                id: 42,
                access_hash: None,
                title: "ch".to_string(),
                username: None,
                photo: tl::enums::ChatPhoto::Empty,
                date: 0,
                restriction_reason: None,
                admin_rights: None,
                banned_rights: None,
                default_banned_rights: None,
                participants_count: None,
                usernames: None,
                stories_max_id: None,
                color: None,
                profile_color: None,
                emoji_status: None,
                level: None,
                subscription_until_date: None,
                bot_verification_icon: None,
                send_paid_messages_stars: None,
                linked_monoforum_id: None,
            }),
        ];
        let found = channel_from_chats(&chats, 42).expect("channel 42");
        assert_eq!(found.id, 42);
        assert!(found.signatures);
        assert!(channel_from_chats(&chats, 77).is_none());
        assert!(channel_from_chats(&chats, 999).is_none());
    }

    fn edit_args(chat: &str) -> EditArgs {
        EditArgs {
            chat: chat.to_string(),
            title: None,
            about: None,
            photo: None,
        }
    }

    #[test]
    fn edit_requires_at_least_one_flag_and_valid_chat() {
        let mut args = edit_args("");
        args.title = Some("t".to_string());
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
        args = edit_args("@c");
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
        args.title = Some("New title".to_string());
        assert!(validate_edit(&args).is_ok());
    }

    #[test]
    fn edit_title_rejects_empty_and_over_cap() {
        let mut args = edit_args("@c");
        args.title = Some("   ".to_string());
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
        args.title = Some("x".repeat(CHAT_TITLE_MAX_CHARS));
        assert!(validate_edit(&args).is_ok());
        args.title = Some("x".repeat(CHAT_TITLE_MAX_CHARS + 1));
        let err = validate_edit(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.to_string().contains("--title"));
    }

    #[test]
    fn edit_about_allows_empty_clear_and_enforces_cap() {
        let mut args = edit_args("@c");
        args.about = Some(String::new());
        assert!(validate_edit(&args).is_ok());
        args.about = Some("x".repeat(CHAT_ABOUT_MAX_CHARS));
        assert!(validate_edit(&args).is_ok());
        args.about = Some("x".repeat(CHAT_ABOUT_MAX_CHARS + 1));
        let err = validate_edit(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.to_string().contains("--about"));
    }

    #[test]
    fn edit_photo_accepts_remove_literal_and_rejects_sensitive_paths() {
        let mut args = edit_args("@c");
        args.photo = Some("remove".to_string());
        assert!(validate_edit(&args).is_ok());
        args.photo = Some("/tmp/x/.env".to_string());
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
        args.photo = Some("/tmp/x/account.session".to_string());
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn edit_photo_accepts_regular_existing_file() {
        let dir = std::env::temp_dir().join(format!("telecli-chat-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("photo.jpg");
        std::fs::write(&photo, b"fake").unwrap();
        let mut args = edit_args("@c");
        args.photo = Some(photo.to_string_lossy().into_owned());
        assert!(validate_edit(&args).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_remove_is_honest_usage_error_before_connect() {
        let mut args = LinkArgs {
            chat: "@group".to_string(),
            to: None,
        };
        assert!(validate_link(&args).is_ok());
        args.to = Some("remove".to_string());
        let err = validate_link(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.to_string().contains("setDiscussionGroup"));
        args.to = Some("   ".to_string());
        assert!(matches!(validate_link(&args), Err(TeleError::Usage(_))));
        args.chat = String::new();
        args.to = None;
        assert!(matches!(validate_link(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn chat_photo_input_photo_extracts_ids_or_none() {
        let empty =
            chat_photo_input_photo(&tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 }));
        assert!(empty.is_none());
        let photo = tl::enums::Photo::Photo(tl::types::Photo {
            id: 555,
            access_hash: -7,
            file_reference: vec![1, 2],
            has_stickers: false,
            date: 0,
            sizes: Vec::new(),
            video_sizes: None,
            dc_id: 1,
        });
        let input = chat_photo_input_photo(&photo).expect("input photo");
        match input {
            tl::enums::InputPhoto::Photo(p) => {
                assert_eq!(p.id, 555);
                assert_eq!(p.access_hash, -7);
                assert_eq!(p.file_reference, vec![1, 2]);
            }
            other => panic!("unexpected input photo {other:?}"),
        }
    }

    #[tokio::test]
    async fn discussion_pair_orders_broadcast_first() {
        let client = offline_client();
        let broadcast_chat = tl::enums::Chat::Channel(tl::types::Channel {
            creator: false,
            left: false,
            broadcast: true,
            verified: false,
            megagroup: false,
            restricted: false,
            signatures: false,
            min: false,
            scam: false,
            has_link: false,
            has_geo: false,
            slowmode_enabled: false,
            call_active: false,
            call_not_empty: false,
            fake: false,
            gigagroup: false,
            noforwards: false,
            join_to_send: false,
            join_request: false,
            forum: false,
            stories_hidden: false,
            stories_hidden_min: false,
            stories_unavailable: false,
            signature_profiles: false,
            autotranslation: false,
            broadcast_messages_allowed: false,
            monoforum: false,
            forum_tabs: false,
            id: 1,
            access_hash: None,
            title: "broadcast".to_string(),
            username: None,
            photo: tl::enums::ChatPhoto::Empty,
            date: 0,
            restriction_reason: None,
            admin_rights: None,
            banned_rights: None,
            default_banned_rights: None,
            participants_count: None,
            usernames: None,
            stories_max_id: None,
            color: None,
            profile_color: None,
            emoji_status: None,
            level: None,
            subscription_until_date: None,
            bot_verification_icon: None,
            send_paid_messages_stars: None,
            linked_monoforum_id: None,
        });
        let megagroup_chat = tl::enums::Chat::Channel(tl::types::Channel {
            creator: false,
            left: false,
            broadcast: false,
            verified: false,
            megagroup: true,
            restricted: false,
            signatures: false,
            min: false,
            scam: false,
            has_link: false,
            has_geo: false,
            slowmode_enabled: false,
            call_active: false,
            call_not_empty: false,
            fake: false,
            gigagroup: false,
            noforwards: false,
            join_to_send: false,
            join_request: false,
            forum: false,
            stories_hidden: false,
            stories_hidden_min: false,
            stories_unavailable: false,
            signature_profiles: false,
            autotranslation: false,
            broadcast_messages_allowed: false,
            monoforum: false,
            forum_tabs: false,
            id: 2,
            access_hash: None,
            title: "supergroup".to_string(),
            username: None,
            photo: tl::enums::ChatPhoto::Empty,
            date: 0,
            restriction_reason: None,
            admin_rights: None,
            banned_rights: None,
            default_banned_rights: None,
            participants_count: None,
            usernames: None,
            stories_max_id: None,
            color: None,
            profile_color: None,
            emoji_status: None,
            level: None,
            subscription_until_date: None,
            bot_verification_icon: None,
            send_paid_messages_stars: None,
            linked_monoforum_id: None,
        });
        let broadcast = grammers_client::peer::Peer::from_raw(&client, broadcast_chat.clone());
        let group = grammers_client::peer::Peer::from_raw(&client, megagroup_chat.clone());

        let (b, g) = discussion_pair(group.clone(), broadcast.clone()).expect("ordered pair");
        assert!(matches!(b, grammers_client::peer::Peer::Channel(_)));
        assert!(matches!(g, grammers_client::peer::Peer::Group(ref grp) if grp.is_megagroup()));

        assert!(discussion_pair(broadcast.clone(), broadcast.clone()).is_err());
        let user_peer = grammers_client::peer::Peer::from_raw(
            &client,
            tl::enums::Chat::Chat(tl::types::Chat {
                creator: false,
                left: false,
                deactivated: false,
                call_active: false,
                call_not_empty: false,
                noforwards: false,
                id: 3,
                title: "basic".to_string(),
                photo: tl::enums::ChatPhoto::Empty,
                participants_count: 0,
                date: 0,
                version: 0,
                migrated_to: None,
                admin_rights: None,
                default_banned_rights: None,
            }),
        );
        assert!(discussion_pair(user_peer.clone(), broadcast.clone()).is_err());
    }
}
