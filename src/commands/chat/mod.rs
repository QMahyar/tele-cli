use clap::{Args, Subcommand};

use grammers_client::tl;

use grammers_session::types::PeerInfo;

use grammers_session::Session;

use std::collections::HashMap;

use crate::client::{self, ClientGuard};

use crate::chat_target::ChatTarget;
use crate::commands::credentials::creds_api_id;

use crate::commands::helpers::{peer_id, stats_abs, stats_percent, stats_period};

use crate::entities;

use crate::error::tele_invocation;

use crate::error::{TeleError, TeleResult};

use crate::executor::{run_fanout, GlobalFlags};

use crate::output;

pub mod admin_log;

pub mod invite;

pub mod participants;

pub mod settings;

#[allow(unused_imports)]
pub use admin_log::*;

#[allow(unused_imports)]
pub use invite::*;

#[allow(unused_imports)]
pub use participants::*;

#[allow(unused_imports)]
pub use settings::*;

#[derive(Subcommand)]
pub enum ChatCmd {
    Join(ChatArgs),
    Leave(ChatArgs),
    Invite(InviteArgs),
    Requests(RequestsArgs),
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

#[derive(Clone, Args)]
pub struct InviteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me (not required with --check)"
    )]
    chat: Option<String>,
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
    #[arg(
        long,
        value_name = "LINK",
        help = "preview an invite link without joining (title, members, approval flag)"
    )]
    check: Option<String>,
}

#[derive(Args)]
pub struct RequestsArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, invite link, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        value_name = "USER",
        conflicts_with = "all",
        help = "with --approve/--dismiss: act on this user's request"
    )]
    user: Option<String>,
    #[arg(
        long,
        conflicts_with = "user",
        help = "with --approve/--dismiss: act on every pending request"
    )]
    all: bool,
    #[arg(
        long,
        conflicts_with = "dismiss",
        help = "approve join request(s); mutator, requires --account/--tag and honors --dry-run"
    )]
    approve: bool,
    #[arg(
        long,
        conflicts_with = "approve",
        help = "dismiss join request(s); mutator, requires --account/--tag and honors --dry-run"
    )]
    dismiss: bool,
    #[arg(
        long,
        value_name = "LINK",
        help = "scope list / bulk approve / bulk dismiss to requests arriving via this invite link"
    )]
    link: Option<String>,
    #[arg(long, default_value_t = 100, help = "max requests to list (1-10000)")]
    limit: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum RequestsAction {
    #[default]
    List,
    Approve,
    Dismiss,
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
        ChatCmd::Invite(a) => invite::invite(a, flags).await,
        ChatCmd::Requests(a) => requests(a, flags).await,
        ChatCmd::Participants(a) => participants::participants(a, flags).await,
        ChatCmd::Kick(a) => participants::kick(a, flags).await,
        ChatCmd::Admin(a) => participants::admin(a, flags).await,
        ChatCmd::AdminLog(a) => admin_log::admin_log(a, flags).await,
        ChatCmd::Stats(a) => stats(a, flags).await,
        ChatCmd::Settings(a) => settings::settings(a, flags).await,
        ChatCmd::Edit(a) => settings::edit_chat(a, flags).await,
        ChatCmd::Link(a) => settings::link_chat(a, flags).await,
        ChatCmd::Create(a) => create(a, flags).await,
    }
}

async fn join(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
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
            let accept_url = if grammers_client::Client::parse_invite_link(&normalized).is_some() {
                normalized.clone()
            } else if is_bare_invite_hash(&normalized) {
                let hash = normalized.strip_prefix('+').unwrap_or(&normalized);
                format!("https://t.me/+{hash}")
            } else {
                normalized.clone()
            };
            if grammers_client::Client::parse_invite_link(&accept_url).is_some() {
                let joined = guard
                    .client
                    .accept_invite_link(&accept_url)
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

async fn leave(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
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

pub(crate) const INVITE_LIST_LIMIT: i32 = 100;

#[derive(Debug, Clone, Default)]
struct ValidatedRequests {
    action: RequestsAction,
    user: Option<String>,
    all: bool,
    link: Option<String>,
    limit: u32,
}

fn validate_requests(args: &RequestsArgs) -> TeleResult<ValidatedRequests> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    if args.approve && args.dismiss {
        return Err(TeleError::Usage(
            "--approve and --dismiss are mutually exclusive".to_string(),
        ));
    }
    if args.all && args.user.is_some() {
        return Err(TeleError::Usage(
            "--all and --user are mutually exclusive".to_string(),
        ));
    }
    let action = if args.approve {
        RequestsAction::Approve
    } else if args.dismiss {
        RequestsAction::Dismiss
    } else {
        RequestsAction::List
    };
    if action == RequestsAction::List {
        if args.user.is_some() {
            return Err(TeleError::Usage(
                "--user applies to --approve/--dismiss only".to_string(),
            ));
        }
        if args.all {
            return Err(TeleError::Usage(
                "--all applies to --approve/--dismiss only".to_string(),
            ));
        }
    } else if !args.all && args.user.is_none() {
        let flag = if action == RequestsAction::Approve {
            "--approve"
        } else {
            "--dismiss"
        };
        return Err(TeleError::Usage(format!(
            "{flag} requires --user USER or --all"
        )));
    }
    let user = match &args.user {
        Some(u) => {
            let t = u.trim();
            if t.is_empty() {
                return Err(TeleError::Usage("--user must not be empty".to_string()));
            }
            Some(t.to_string())
        }
        None => None,
    };
    let limit = crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let link = match &args.link {
        Some(l) => Some(normalized_validated_link(l, "--link")?),
        None => None,
    };
    Ok(ValidatedRequests {
        action,
        user,
        all: args.all,
        link,
        limit,
    })
}

async fn requests(args: RequestsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let plan = validate_requests(&args)?;
    if plan.action != RequestsAction::List {
        crate::executor::require_explicit_selection("chat requests --approve/--dismiss", flags)?;
    }
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
                return Ok(requests_dry_run_payload(&target, &plan));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "requests")?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            match plan.action {
                RequestsAction::List => {
                    let mut rows = Vec::new();
                    let mut offset_date = 0i32;
                    let mut offset_user: tl::enums::InputUser = tl::types::InputUserEmpty {}.into();
                    let limit = plan.limit as i32;
                    loop {
                        let remaining = limit - rows.len() as i32;
                        if remaining <= 0 {
                            break;
                        }
                        let r: tl::enums::messages::ChatInviteImporters = guard
                            .client
                            .invoke(&tl::functions::messages::GetChatInviteImporters {
                                requested: true,
                                subscription_expired: false,
                                peer: peer.clone(),
                                link: plan.link.clone(),
                                q: None,
                                offset_date,
                                offset_user: offset_user.clone(),
                                limit: remaining.min(INVITE_LIST_LIMIT),
                            })
                            .await
                            .map_err(tele_invocation)?;
                        let tl::enums::messages::ChatInviteImporters::Importers(ref list) = r;
                        let page_len = list.importers.len();
                        if page_len > 0 {
                            if let Some(tl::enums::ChatInviteImporter::Importer(imp)) =
                                list.importers.last()
                            {
                                offset_date = imp.date;
                                offset_user = tl::types::InputUser {
                                    user_id: imp.user_id,
                                    access_hash: 0,
                                }
                                .into();
                            }
                        }
                        rows.extend(join_request_rows(&guard.client, &r, plan.link.as_deref()));
                        if page_len == 0 {
                            break;
                        }
                    }
                    if !output::machine_mode(json, jsonl) {
                        print_request_table(&name, multi, &rows)?;
                    }
                    Ok(serde_json::json!({"requests": rows}))
                }
                RequestsAction::Approve | RequestsAction::Dismiss => {
                    let approved = plan.action == RequestsAction::Approve;
                    match &plan.user {
                        Some(user) => {
                            let user_peer =
                                entities::resolve_peer(&guard.client, guard.session.as_ref(), user)
                                    .await?;
                            let user_input = entities::input_user(&user_peer)
                                .await
                                .map_err(tele_invocation)?;
                            guard.rate_limiter.acquire().await;
                            guard
                                .client
                                .invoke(&tl::functions::messages::HideChatJoinRequest {
                                    approved,
                                    peer,
                                    user_id: user_input,
                                })
                                .await
                                .map_err(tele_invocation)?;
                            Ok(serde_json::json!({
                                "chat": target,
                                "user": user,
                                "action": if approved { "approved" } else { "dismissed" }}))
                        }
                        None => {
                            guard.rate_limiter.acquire().await;
                            guard
                                .client
                                .invoke(&tl::functions::messages::HideAllChatJoinRequests {
                                    approved,
                                    peer,
                                    link: plan.link.clone(),
                                })
                                .await
                                .map_err(tele_invocation)?;
                            let mut v = serde_json::json!({
                                "chat": target,
                                "all": plan.all,
                                "action": if approved { "approved" } else { "dismissed" }});
                            if let Some(link) = &plan.link {
                                v["link"] = serde_json::json!(link);
                            }
                            Ok(v)
                        }
                    }
                }
            }
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn requests_dry_run_payload(chat: &str, plan: &ValidatedRequests) -> serde_json::Value {
    let action = match plan.action {
        RequestsAction::List => "list",
        RequestsAction::Approve => "approve",
        RequestsAction::Dismiss => "dismiss",
    };
    match plan.action {
        RequestsAction::List => {
            let mut v = serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "action": action,
                "would": format!("list pending join requests of chat {chat}")});
            if let Some(link) = &plan.link {
                v["link"] = serde_json::json!(link);
            }
            v
        }
        RequestsAction::Approve | RequestsAction::Dismiss => match &plan.user {
            Some(user) => {
                let mut v = serde_json::json!({
                    "dry_run": true,
                    "chat": chat,
                    "action": action,
                    "user": user,
                    "would": format!("{action} join request of {user} in chat {chat}")});
                if let Some(link) = &plan.link {
                    v["link"] = serde_json::json!(link);
                }
                v
            }
            None => {
                let would = if let Some(link) = &plan.link {
                    format!("{action} join request of chat {chat} via {link}")
                } else {
                    format!("{action} every pending join request of chat {chat}")
                };
                let mut v = serde_json::json!({
                    "dry_run": true,
                    "chat": chat,
                    "action": action,
                    "all": plan.all,
                    "would": would});
                if let Some(link) = &plan.link {
                    v["link"] = serde_json::json!(link);
                }
                v
            }
        },
    }
}

fn join_request_rows(
    client: &grammers_client::Client,
    result: &tl::enums::messages::ChatInviteImporters,
    link: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut rows = chat_invite_importers_rows(client, result);
    if let Some(link) = link {
        for row in rows.iter_mut() {
            row["link"] = serde_json::json!(link);
        }
    }
    rows
}

fn print_request_table(account: &str, multi: bool, rows: &[serde_json::Value]) -> TeleResult<()> {
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r["id"].to_string(),
                r["name"].as_str().unwrap_or_default().to_string(),
                r["date"].as_str().unwrap_or_default().to_string(),
                r["link"].as_str().unwrap_or_default().to_string(),
            ]
        })
        .collect();
    output::print_account_table(account, multi, &["id", "name", "date", "link"], &table_rows)
}

pub(crate) fn user_display_name(
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

pub(crate) fn rfc3339_or_empty(ts: Option<i32>) -> String {
    ts.and_then(|t| chrono::DateTime::from_timestamp(i64::from(t), 0))
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn stats_dry_run_payload(chat: &str, broadcast: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "broadcast": broadcast,
        "would": format!("show stats of chat {chat}")})
}

async fn stats(args: StatsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
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
            ensure_chat_peer(&chat, "chat")?;
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
                    "recent_posts_interactions": r.recent_posts_interactions.len()})
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
                    "top_inviters": r.top_inviters.len()})
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
    if args.forum && args.kind != "supergroup" {
        return Err(TeleError::Usage(
            "--forum is only supported with --kind supergroup".to_string(),
        ));
    }
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
                    let chat = created_chat(&r.updates)
                        .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
                    let chat_id = chat.id();
                    cache_created_chat(guard.session.as_ref(), Some(chat)).await;
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
                    let chat = created_chat(&r)
                        .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
                    let chat_id = chat.id();
                    cache_created_chat(guard.session.as_ref(), Some(chat)).await;
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
                    let chat = created_chat(&r)
                        .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
                    let chat_id = chat.id();
                    cache_created_chat(guard.session.as_ref(), Some(chat)).await;
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

pub(crate) fn created_chat(r: &tl::enums::Updates) -> Option<&tl::enums::Chat> {
    match r {
        tl::enums::Updates::Updates(u) => u.chats.first(),
        _ => None,
    }
}

pub(crate) async fn cache_created_chat<S: Session>(session: &S, chat: Option<&tl::enums::Chat>)
where
    S::Error: std::fmt::Display,
{
    if let Some(chat) = chat {
        if let Err(e) = entities::cache_chat(session, chat).await {
            log::warn!(
                "failed to cache access_hash for created chat {}: {e}",
                chat.id()
            );
        }
    }
}

pub(crate) async fn cache_joined_chat<S: Session>(session: &S, peer: &grammers_client::peer::Peer)
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

pub(crate) fn ensure_chat_peer(peer: &grammers_client::peer::Peer, action: &str) -> TeleResult<()> {
    if matches!(peer, grammers_client::peer::Peer::User(_)) {
        return Err(TeleError::Usage(format!(
            "{action} requires a chat, got a user"
        )));
    }
    Ok(())
}

fn default_requests_limit() -> u32 {
    100
}

fn default_participants_limit() -> u32 {
    100
}

fn default_adminlog_limit() -> u32 {
    20
}

fn default_create_kind() -> String {
    "group".to_string()
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct JoinParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ChatArgs> for JoinParams {
    fn from(a: &ChatArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            dry_run: false,
        }
    }
}

impl From<&JoinParams> for ChatArgs {
    fn from(p: &JoinParams) -> Self {
        Self {
            chat: p.chat.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct LeaveParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ChatArgs> for LeaveParams {
    fn from(a: &ChatArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            dry_run: false,
        }
    }
}

impl From<&LeaveParams> for ChatArgs {
    fn from(p: &LeaveParams) -> Self {
        Self {
            chat: p.chat.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct CreateServeParams {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default = "default_create_kind")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) forum: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&CreateArgs> for CreateServeParams {
    fn from(a: &CreateArgs) -> Self {
        Self {
            title: a.title.clone(),
            description: a.description.clone(),
            kind: a.kind.clone(),
            forum: a.forum,
            dry_run: false,
        }
    }
}

impl From<&CreateServeParams> for CreateArgs {
    fn from(p: &CreateServeParams) -> Self {
        Self {
            title: p.title.clone(),
            description: p.description.clone(),
            kind: p.kind.clone(),
            forum: p.forum,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SettingsServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) slow_mode: Option<String>,
    #[serde(default)]
    pub(crate) noforwards: Option<String>,
    #[serde(default)]
    pub(crate) signatures: Option<String>,
    #[serde(default)]
    pub(crate) pre_history: Option<String>,
    #[serde(default)]
    pub(crate) join_request: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SettingsArgs> for SettingsServeParams {
    fn from(a: &SettingsArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            slow_mode: a.slow_mode.clone(),
            noforwards: a.noforwards.clone(),
            signatures: a.signatures.clone(),
            pre_history: a.pre_history.clone(),
            join_request: a.join_request.clone(),
            dry_run: false,
        }
    }
}

impl From<&SettingsServeParams> for SettingsArgs {
    fn from(p: &SettingsServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            slow_mode: p.slow_mode.clone(),
            noforwards: p.noforwards.clone(),
            signatures: p.signatures.clone(),
            pre_history: p.pre_history.clone(),
            join_request: p.join_request.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct EditServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) about: Option<String>,
    #[serde(default)]
    pub(crate) photo: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&EditArgs> for EditServeParams {
    fn from(a: &EditArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            title: a.title.clone(),
            about: a.about.clone(),
            photo: a.photo.clone(),
            dry_run: false,
        }
    }
}

impl From<&EditServeParams> for EditArgs {
    fn from(p: &EditServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            title: p.title.clone(),
            about: p.about.clone(),
            photo: p.photo.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct LinkServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) to: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&LinkArgs> for LinkServeParams {
    fn from(a: &LinkArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            to: a.to.clone(),
            dry_run: false,
        }
    }
}

impl From<&LinkServeParams> for LinkArgs {
    fn from(p: &LinkServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            to: p.to.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct KickServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) user: String,
    #[serde(default)]
    pub(crate) ban: bool,
    #[serde(default)]
    pub(crate) duration: Option<String>,
    #[serde(default)]
    pub(crate) rights: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&KickArgs> for KickServeParams {
    fn from(a: &KickArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            user: a.user.clone(),
            ban: a.ban,
            duration: a.duration.clone(),
            rights: a.rights.clone(),
            dry_run: false,
        }
    }
}

impl From<&KickServeParams> for KickArgs {
    fn from(p: &KickServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            user: p.user.clone(),
            ban: p.ban,
            duration: p.duration.clone(),
            rights: p.rights.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct AdminServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) user: String,
    #[serde(default)]
    pub(crate) promote: bool,
    #[serde(default)]
    pub(crate) demote: bool,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) preset: Option<String>,
    #[serde(default)]
    pub(crate) rights: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&AdminArgs> for AdminServeParams {
    fn from(a: &AdminArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            user: a.user.clone(),
            promote: a.promote,
            demote: a.demote,
            title: a.title.clone(),
            preset: a.preset.clone(),
            rights: a.rights.clone(),
            dry_run: false,
        }
    }
}

impl From<&AdminServeParams> for AdminArgs {
    fn from(p: &AdminServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            user: p.user.clone(),
            promote: p.promote,
            demote: p.demote,
            title: p.title.clone(),
            preset: p.preset.clone(),
            rights: p.rights.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct AdminLogServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default = "default_adminlog_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) admin: Option<String>,
    #[serde(default)]
    pub(crate) search: Option<String>,
    #[serde(default)]
    pub(crate) since: Option<String>,
    #[serde(default)]
    pub(crate) until: Option<String>,
    #[serde(default)]
    pub(crate) events: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&AdminLogArgs> for AdminLogServeParams {
    fn from(a: &AdminLogArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            limit: a.limit,
            admin: a.admin.clone(),
            search: a.search.clone(),
            since: a.since.clone(),
            until: a.until.clone(),
            events: a.events.clone(),
            dry_run: false,
        }
    }
}

impl From<&AdminLogServeParams> for AdminLogArgs {
    fn from(p: &AdminLogServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            limit: p.limit,
            admin: p.admin.clone(),
            search: p.search.clone(),
            since: p.since.clone(),
            until: p.until.clone(),
            events: p.events.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct StatsServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) broadcast: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&StatsArgs> for StatsServeParams {
    fn from(a: &StatsArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            broadcast: a.broadcast,
            dry_run: false,
        }
    }
}

impl From<&StatsServeParams> for StatsArgs {
    fn from(p: &StatsServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            broadcast: p.broadcast,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct InviteServeParams {
    #[serde(default)]
    pub(crate) chat: Option<String>,
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) expire: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) usage_limit: Option<u32>,
    #[serde(default)]
    pub(crate) request_approval: Option<String>,
    #[serde(default)]
    pub(crate) list: bool,
    #[serde(default)]
    pub(crate) revoked: bool,
    #[serde(default)]
    pub(crate) importers: Option<String>,
    #[serde(default)]
    pub(crate) edit: Option<String>,
    #[serde(default)]
    pub(crate) revoke: bool,
    #[serde(default)]
    pub(crate) delete_revoked: bool,
    #[serde(default)]
    pub(crate) check: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&InviteArgs> for InviteServeParams {
    fn from(a: &InviteArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            user: a.user.clone(),
            expire: a.expire.clone(),
            title: a.title.clone(),
            usage_limit: a.usage_limit,
            request_approval: a.request_approval.clone(),
            list: a.list,
            revoked: a.revoked,
            importers: a.importers.clone(),
            edit: a.edit.clone(),
            revoke: a.revoke,
            delete_revoked: a.delete_revoked,
            check: a.check.clone(),
            dry_run: false,
        }
    }
}

impl From<&InviteServeParams> for InviteArgs {
    fn from(p: &InviteServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            user: p.user.clone(),
            expire: p.expire.clone(),
            title: p.title.clone(),
            usage_limit: p.usage_limit,
            request_approval: p.request_approval.clone(),
            list: p.list,
            revoked: p.revoked,
            importers: p.importers.clone(),
            edit: p.edit.clone(),
            revoke: p.revoke,
            delete_revoked: p.delete_revoked,
            check: p.check.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct RequestsServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) all: bool,
    #[serde(default)]
    pub(crate) approve: bool,
    #[serde(default)]
    pub(crate) dismiss: bool,
    #[serde(default)]
    pub(crate) link: Option<String>,
    #[serde(default = "default_requests_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&RequestsArgs> for RequestsServeParams {
    fn from(a: &RequestsArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            user: a.user.clone(),
            all: a.all,
            approve: a.approve,
            dismiss: a.dismiss,
            link: a.link.clone(),
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&RequestsServeParams> for RequestsArgs {
    fn from(p: &RequestsServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            user: p.user.clone(),
            all: p.all,
            approve: p.approve,
            dismiss: p.dismiss,
            link: p.link.clone(),
            limit: p.limit,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ParticipantsServeParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) role: Option<String>,
    #[serde(default)]
    pub(crate) search: Option<String>,
    #[serde(default = "default_participants_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ParticipantsArgs> for ParticipantsServeParams {
    fn from(a: &ParticipantsArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            role: a.role.clone(),
            search: a.search.clone(),
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&ParticipantsServeParams> for ParticipantsArgs {
    fn from(p: &ParticipantsServeParams) -> Self {
        Self {
            chat: p.chat.clone(),
            role: p.role.clone(),
            search: p.search.clone(),
            limit: p.limit,
        }
    }
}

fn validate_join(args: &ChatArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat").map(|_| ())
}

fn validate_leave(args: &ChatArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat").map(|_| ())
}

fn validate_stats(args: &StatsArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat").map(|_| ())
}

fn validate_participants(args: &ParticipantsArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    parse_participant_role(args.role.as_deref())?;
    Ok(())
}

fn validate_admin_log(args: &AdminLogArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
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
    parse_admin_events_filter(args.events.as_deref())?;
    Ok(())
}

fn validate_invite_serve(args: &InviteArgs) -> TeleResult<()> {
    validate_invite(args)?;
    Ok(())
}

fn validate_requests_serve(args: &RequestsArgs) -> TeleResult<()> {
    validate_requests(args)?;
    Ok(())
}

fn join_serve_dry_run(args: &ChatArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "would": format!("join chat {}", args.chat)
    }))
}

fn leave_serve_dry_run(args: &ChatArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "would": format!("leave chat {}", args.chat)
    }))
}

fn create_serve_dry_run(args: &CreateArgs) -> TeleResult<serde_json::Value> {
    let mut value = serde_json::json!({
        "dry_run": true,
        "title": args.title,
        "kind": args.kind,
        "forum": args.forum,
        "would": format!("create {} chat \"{}\"", args.kind, args.title)
    });
    if let Some(d) = args.description.as_deref() {
        value["description"] = serde_json::json!(d);
    }
    Ok(value)
}

fn settings_serve_dry_run(args: &SettingsArgs) -> TeleResult<serde_json::Value> {
    let slow_mode = parse_slow_mode(args.slow_mode.as_deref())?;
    let signatures = parse_on_off(args.signatures.as_deref())?;
    let pre_history = parse_on_off(args.pre_history.as_deref())?;
    let join_request = parse_on_off(args.join_request.as_deref())?;
    let has_toggles = slow_mode.is_some()
        || signatures.is_some()
        || pre_history.is_some()
        || join_request.is_some();
    let mut data = serde_json::json!({
    "dry_run": true,
    "chat": args.chat,
    "would": if has_toggles {
        format!("update settings of chat {}", args.chat)
    } else {
        format!("read settings of chat {}", args.chat)
    }});
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
    Ok(data)
}

fn edit_serve_dry_run(args: &EditArgs) -> TeleResult<serde_json::Value> {
    let mut data = serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "would": format!("edit metadata of chat {}", args.chat)});
    if let Some(t) = &args.title {
        data["title"] = serde_json::json!(t.trim());
    }
    if let Some(a) = &args.about {
        data["about"] = serde_json::json!(a.trim());
    }
    if let Some(p) = &args.photo {
        data["photo"] = serde_json::json!(p);
    }
    Ok(data)
}

fn link_serve_dry_run(args: &LinkArgs) -> TeleResult<serde_json::Value> {
    let to_target = parse_link_target(args.to.as_deref())?;
    Ok(match &to_target {
        None => serde_json::json!({
            "dry_run": true,
            "chat": args.chat,
            "would": format!("read discussion link of chat {}", args.chat)}),
        Some(to) => serde_json::json!({
            "dry_run": true,
            "chat": args.chat,
            "to": to,
            "would": format!("link chat {} with discussion group {}", args.chat, to)}),
    })
}

fn kick_serve_dry_run(args: &KickArgs) -> TeleResult<serde_json::Value> {
    let until_secs = parse_ban_duration(args.duration.as_deref())?;
    let rights_entries = match &args.rights {
        Some(csv) => parse_banned_rights_csv(csv)?,
        None => Vec::new(),
    };
    let mut data = serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "user": args.user,
        "ban": args.ban,
        "would": format!("kick user {} from chat {}", args.user, args.chat)
    });
    if let Some(secs) = until_secs {
        data["duration"] = serde_json::json!(secs);
    }
    if !rights_entries.is_empty() {
        data["rights"] = serde_json::json!(rights_entries);
    }
    Ok(data)
}

fn admin_serve_dry_run(args: &AdminArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "user": args.user,
        "promote": args.promote,
        "demote": args.demote,
        "would": format!("change admin status of user {} in chat {}", args.user, args.chat)}))
}

fn adminlog_serve_dry_run(args: &AdminLogArgs) -> TeleResult<serde_json::Value> {
    Ok(admin_log_dry_run_payload(
        &args.chat,
        args.search.as_deref().unwrap_or_default(),
        args.events.is_some(),
        args.admin.is_some(),
    ))
}

fn stats_serve_dry_run(args: &StatsArgs) -> TeleResult<serde_json::Value> {
    Ok(stats_dry_run_payload(&args.chat, args.broadcast))
}

fn invite_serve_dry_run(args: &InviteArgs) -> TeleResult<serde_json::Value> {
    let plan = validate_invite(args)?;
    Ok(invite_dry_run_payload(
        args.chat.clone().unwrap_or_default().as_str(),
        &plan,
    ))
}

fn requests_serve_dry_run(args: &RequestsArgs) -> TeleResult<serde_json::Value> {
    let plan = validate_requests(args)?;
    Ok(requests_dry_run_payload(&args.chat, &plan))
}

fn participants_serve_dry_run(args: &ParticipantsArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "would": format!("list participants of chat {}", args.chat)
    }))
}

pub(crate) async fn chat_join_core(
    shares: &crate::client::ServeShares,
    params: JoinParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let normalized = normalize_invite_link(&params.chat);
    let accept_url = if grammers_client::Client::parse_invite_link(&normalized).is_some() {
        normalized.clone()
    } else if is_bare_invite_hash(&normalized) {
        let hash = normalized.strip_prefix('+').unwrap_or(&normalized);
        format!("https://t.me/+{hash}")
    } else {
        normalized.clone()
    };
    if grammers_client::Client::parse_invite_link(&accept_url).is_some() {
        let joined = shares
            .client
            .accept_invite_link(&accept_url)
            .await
            .map_err(tele_invocation)?;
        if let Some(peer) = joined {
            cache_joined_chat(shares.session.as_ref(), &peer).await;
        }
    } else {
        let peer =
            entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
        let chat_ref = entities::peer_ref(&peer).await.map_err(tele_invocation)?;
        let joined = shares
            .client
            .join_chat(chat_ref)
            .await
            .map_err(tele_invocation)?;
        if let Some(peer) = joined {
            cache_joined_chat(shares.session.as_ref(), &peer).await;
        }
    }
    Ok(serde_json::json!({"chat": params.chat, "joined": true}))
}

pub(crate) async fn chat_leave_core(
    shares: &crate::client::ServeShares,
    params: LeaveParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    match &peer {
        grammers_client::peer::Peer::Channel(_) => {
            let channel = entities::input_channel(&peer)
                .await
                .map_err(tele_invocation)?;
            shares
                .client
                .invoke(&tl::functions::channels::LeaveChannel { channel })
                .await
                .map_err(tele_invocation)?;
        }
        grammers_client::peer::Peer::Group(_) if entities::is_channel(&peer) => {
            let channel = entities::input_channel(&peer)
                .await
                .map_err(tele_invocation)?;
            shares
                .client
                .invoke(&tl::functions::channels::LeaveChannel { channel })
                .await
                .map_err(tele_invocation)?;
        }
        grammers_client::peer::Peer::Group(_) => {
            let user_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
            shares
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
    Ok(serde_json::json!({"chat": params.chat, "left": true}))
}

pub(crate) async fn chat_create_core(
    shares: &crate::client::ServeShares,
    params: CreateServeParams,
) -> TeleResult<serde_json::Value> {
    if params.forum && params.kind != "supergroup" {
        return Err(TeleError::Usage(
            "--forum is only supported with --kind supergroup".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    match params.kind.as_str() {
        "group" => {
            let r: tl::enums::messages::InvitedUsers = shares
                .client
                .invoke(&tl::functions::messages::CreateChat {
                    users: Vec::new(),
                    title: params.title.clone(),
                    ttl_period: None,
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::messages::InvitedUsers::Users(r) = r;
            let chat = created_chat(&r.updates)
                .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
            let chat_id = chat.id();
            cache_created_chat(shares.session.as_ref(), Some(chat)).await;
            Ok(serde_json::json!({"kind": "group", "chat_id": chat_id}))
        }
        "supergroup" => {
            let r: tl::enums::Updates = shares
                .client
                .invoke(&tl::functions::channels::CreateChannel {
                    broadcast: false,
                    megagroup: true,
                    for_import: false,
                    forum: params.forum,
                    title: params.title.clone(),
                    about: params.description.clone().unwrap_or_default(),
                    geo_point: None,
                    address: None,
                    ttl_period: None,
                })
                .await
                .map_err(tele_invocation)?;
            let chat = created_chat(&r)
                .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
            let chat_id = chat.id();
            cache_created_chat(shares.session.as_ref(), Some(chat)).await;
            Ok(serde_json::json!({"kind": "supergroup", "forum": params.forum, "chat_id": chat_id}))
        }
        "channel" => {
            let r: tl::enums::Updates = shares
                .client
                .invoke(&tl::functions::channels::CreateChannel {
                    broadcast: true,
                    megagroup: false,
                    for_import: false,
                    forum: false,
                    title: params.title.clone(),
                    about: params.description.clone().unwrap_or_default(),
                    geo_point: None,
                    address: None,
                    ttl_period: None,
                })
                .await
                .map_err(tele_invocation)?;
            let chat = created_chat(&r)
                .ok_or_else(|| TeleError::Other("unexpected response shape".to_string()))?;
            let chat_id = chat.id();
            cache_created_chat(shares.session.as_ref(), Some(chat)).await;
            Ok(serde_json::json!({"kind": "channel", "chat_id": chat_id}))
        }
        other => Err(TeleError::Usage(format!(
            "unknown chat kind {other} (use group, supergroup or channel)"
        ))),
    }
}

pub(crate) async fn chat_settings_core(
    shares: &crate::client::ServeShares,
    params: SettingsServeParams,
) -> TeleResult<serde_json::Value> {
    let slow_mode = parse_slow_mode(params.slow_mode.as_deref())?;
    let signatures = parse_on_off(params.signatures.as_deref())?;
    let pre_history = parse_on_off(params.pre_history.as_deref())?;
    let join_request = parse_on_off(params.join_request.as_deref())?;
    let has_toggles = slow_mode.is_some()
        || signatures.is_some()
        || pre_history.is_some()
        || join_request.is_some();
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "settings")?;
    let is_basic_group =
        matches!(&chat, grammers_client::peer::Peer::Group(_)) && !entities::is_channel(&chat);
    if is_basic_group {
        return Err(TeleError::Usage(
            "chat settings are not supported for basic groups; these toggles apply to channels and supergroups only".to_string(),
        ));
    }
    let input_channel = entities::input_channel(&chat)
        .await
        .map_err(tele_invocation)?;
    if has_toggles {
        let mut applied = Vec::new();
        if let Some(secs) = slow_mode {
            applied.push("slow_mode");
            shares.rate_limiter.acquire().await;
            shares
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
            shares.rate_limiter.acquire().await;
            shares
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
            shares.rate_limiter.acquire().await;
            shares
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
            shares.rate_limiter.acquire().await;
            shares
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
            "chat": params.chat,
            "applied": applied}));
    }
    shares.rate_limiter.acquire().await;
    let full = shares
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
                "settings unavailable: server returned group info for this chat".to_string(),
            ));
        }
    };
    let channel = channel_from_chats(&full.chats, full_chat.id);
    Ok(serde_json::json!({
        "chat": params.chat,
        "slow_mode": full_chat.slowmode_seconds.unwrap_or(0),
        "noforwards": channel.map(|c| c.noforwards),
        "signatures": channel.map(|c| c.signatures),
        "pre_history_hidden": full_chat.hidden_prehistory,
        "join_request": channel.map(|c| c.join_request),
        "linked_chat_id": full_chat.linked_chat_id}))
}

pub(crate) async fn chat_edit_core(
    shares: &crate::client::ServeShares,
    params: EditServeParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "chat edit")?;
    let is_basic_group =
        matches!(&chat, grammers_client::peer::Peer::Group(_)) && !entities::is_channel(&chat);
    let mut applied = Vec::new();
    if let Some(new_title) = &params.title {
        applied.push("title");
        let new_title = new_title.trim().to_string();
        if is_basic_group {
            shares.rate_limiter.acquire().await;
            shares
                .client
                .invoke(&tl::functions::messages::EditChatTitle {
                    chat_id: chat.id().bare_id().unwrap_or_default(),
                    title: new_title,
                })
                .await
                .map_err(tele_invocation)?;
        } else {
            shares.rate_limiter.acquire().await;
            shares
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
    if let Some(new_about) = &params.about {
        applied.push("about");
        let new_about = new_about.trim().to_string();
        let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
        shares.rate_limiter.acquire().await;
        shares
            .client
            .invoke(&tl::functions::messages::EditChatAbout {
                peer,
                about: new_about,
            })
            .await
            .map_err(tele_invocation)?;
    }
    if let Some(photo) = &params.photo {
        applied.push("photo");
        if photo == "remove" {
            let full = fetch_full_chat_info(&shares.client, &chat).await?;
            let tl::enums::messages::ChatFull::Full(full) = full;
            let current: Option<tl::enums::Photo> = match &full.full_chat {
                tl::enums::ChatFull::ChannelFull(f) => Some(f.chat_photo.clone()),
                tl::enums::ChatFull::Full(f) => f.chat_photo.clone(),
            };
            let input_photo = current
                .as_ref()
                .and_then(chat_photo_input_photo)
                .ok_or_else(|| TeleError::Other("chat has no photo to remove".to_string()))?;
            shares.rate_limiter.acquire().await;
            let _: Vec<i64> = shares
                .client
                .invoke(&tl::functions::photos::DeletePhotos {
                    id: vec![input_photo],
                })
                .await
                .map_err(tele_invocation)?;
        } else {
            let uploaded = shares
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
            shares.rate_limiter.acquire().await;
            if is_basic_group {
                shares
                    .client
                    .invoke(&tl::functions::messages::EditChatPhoto {
                        chat_id: chat.id().bare_id().unwrap_or_default(),
                        photo: chat_photo,
                    })
                    .await
                    .map_err(tele_invocation)?;
            } else {
                shares
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
        "chat": params.chat,
        "applied": applied}))
}

pub(crate) async fn chat_link_core(
    shares: &crate::client::ServeShares,
    params: LinkServeParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "link")?;
    let Some(to_target) = params.to.clone() else {
        let full = fetch_full_chat_info(&shares.client, &chat).await?;
        let tl::enums::messages::ChatFull::Full(full) = full;
        let linked = match full.full_chat {
            tl::enums::ChatFull::ChannelFull(f) => f.linked_chat_id,
            tl::enums::ChatFull::Full(_) => None,
        };
        return Ok(serde_json::json!({
            "chat": params.chat,
            "linked_chat_id": linked}));
    };
    let to_peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &to_target).await?;
    ensure_chat_peer(&to_peer, "--to")?;
    let (broadcast, group) = discussion_pair(chat.clone(), to_peer)?;
    shares.rate_limiter.acquire().await;
    shares
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
        "chat": params.chat,
        "to": to_target,
        "linked": true}))
}

pub(crate) async fn chat_kick_core(
    shares: &crate::client::ServeShares,
    params: KickServeParams,
) -> TeleResult<serde_json::Value> {
    let ban = params.ban;
    let until_secs = parse_ban_duration(params.duration.as_deref())?;
    let rights_entries = match &params.rights {
        Some(csv) => parse_banned_rights_csv(csv)?,
        None => Vec::new(),
    };
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "kick")?;
    let user_peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.user).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let user_ref = entities::peer_ref(&user_peer)
        .await
        .map_err(tele_invocation)?;
    if !ban && rights_entries.is_empty() && until_secs.is_none() {
        shares
            .client
            .kick_participant(chat_ref, user_ref)
            .await
            .map_err(tele_invocation)?;
        return Ok(serde_json::json!({"chat": params.chat, "user": params.user, "kicked": true}));
    }
    let mut call = shares.client.set_banned_rights(chat_ref, user_ref);
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
        "chat": params.chat,
        "user": params.user,
        "kicked": true,
        "banned": ban});
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
}

pub(crate) async fn chat_admin_core(
    shares: &crate::client::ServeShares,
    params: AdminServeParams,
) -> TeleResult<serde_json::Value> {
    let args = AdminArgs::from(&params);
    let rights = resolve_admin_rights(&args)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "chat admin")?;
    let user_peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.user).await?;
    if rights.needs_raw_edit_admin() {
        shares
            .client
            .invoke(&tl::functions::channels::EditAdmin {
                channel: entities::input_channel(&chat)
                    .await
                    .map_err(tele_invocation)?,
                user_id: entities::input_user(&user_peer)
                    .await
                    .map_err(tele_invocation)?,
                admin_rights: rights.to_raw(),
                rank: params.title.clone(),
            })
            .await
            .map_err(tele_invocation)?;
        return Ok(serde_json::json!({
            "chat": params.chat,
            "user": params.user,
            "promote": params.promote,
            "demote": params.demote}));
    }
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let user_ref = entities::peer_ref(&user_peer)
        .await
        .map_err(tele_invocation)?;
    let mut builder = shares.client.set_admin_rights(chat_ref, user_ref);
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
    if let Some(t) = &params.title {
        builder = builder.rank(t.clone());
    }
    builder.await.map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "chat": params.chat,
        "user": params.user,
        "promote": params.promote,
        "demote": params.demote}))
}

pub(crate) async fn chat_admin_log_core(
    shares: &crate::client::ServeShares,
    params: AdminLogServeParams,
) -> TeleResult<serde_json::Value> {
    let since = params
        .since
        .as_deref()
        .map(crate::commands::parse_unixtime)
        .transpose()?;
    let until = params
        .until
        .as_deref()
        .map(crate::commands::parse_unixtime)
        .transpose()?;
    let events_filter = parse_admin_events_filter(params.events.as_deref())?;
    let search_q = params.search.clone().unwrap_or_default();
    shares.rate_limiter.acquire().await;
    let own_id = shares
        .client
        .get_me()
        .await
        .map_err(tele_invocation)?
        .id()
        .bare_id()
        .unwrap_or_default();
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "chat admin-log")?;
    let channel = entities::input_channel(&chat)
        .await
        .map_err(tele_invocation)?;
    let admins = match params.admin.as_deref() {
        None => None,
        Some("me") => Some(vec![tl::enums::InputUser::from(
            tl::types::InputUserSelf {},
        )]),
        Some(user) => {
            let peer =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), user).await?;
            Some(vec![entities::input_user(&peer)
                .await
                .map_err(tele_invocation)?])
        }
    };
    let collected = {
        let client_ref = &shares.client;
        let channel_ref = &channel;
        collect_admin_log(params.limit, None, move |max_id, page_limit| {
            let q = search_q.clone();
            let filter = events_filter.clone();
            let admins = admins.clone();
            async move {
                let raw: tl::enums::channels::AdminLogResults = client_ref
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
        let date = chrono::DateTime::from_timestamp(i64::from(event.date), 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default();
        rows.push(serde_json::json!({
            "id": event.id,
            "date": date,
            "actor": actor_value(&shares.client, &collected.users, event.user_id),
            "action": admin_action_summary(&event.action, own_id)}));
    }
    let rows = filter_events_by_range(rows, since, until);
    Ok(serde_json::json!({"events": rows}))
}

pub(crate) async fn chat_stats_core(
    shares: &crate::client::ServeShares,
    params: StatsServeParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let channel = entities::input_channel(&chat)
        .await
        .map_err(tele_invocation)?;
    let raw = if params.broadcast {
        let r: tl::enums::stats::BroadcastStats = shares
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
            "recent_posts_interactions": r.recent_posts_interactions.len()})
    } else {
        let r: tl::enums::stats::MegagroupStats = shares
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
            "top_inviters": r.top_inviters.len()})
    };
    Ok(serde_json::json!({"chat": params.chat, "stats": raw}))
}

pub(crate) async fn chat_invite_core(
    shares: &crate::client::ServeShares,
    params: InviteServeParams,
) -> TeleResult<serde_json::Value> {
    let args = InviteArgs::from(&params);
    let plan = validate_invite(&args)?;
    let target = params.chat.clone().unwrap_or_default();
    shares.rate_limiter.acquire().await;
    match plan.mode {
        InviteMode::User => {
            let user = plan.user.clone().unwrap_or_default();
            let chat =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), &target).await?;
            let user_peer =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), &user).await?;
            let user_input = entities::input_user(&user_peer)
                .await
                .map_err(tele_invocation)?;
            match &chat {
                grammers_client::peer::Peer::Channel(_) => {
                    let channel = entities::input_channel(&chat)
                        .await
                        .map_err(tele_invocation)?;
                    shares
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
                    shares
                        .client
                        .invoke(&tl::functions::channels::InviteToChannel {
                            channel,
                            users: vec![user_input],
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                grammers_client::peer::Peer::Group(_) => {
                    shares
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
                entities::resolve_peer(&shares.client, shares.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "chat invite")?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let rows = if plan.mode == InviteMode::Export {
                let r: tl::enums::ExportedChatInvite = shares
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
                let r: tl::enums::messages::ExportedChatInvite = shares
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
            Ok(serde_json::json!({"links": rows}))
        }
        InviteMode::List => {
            let chat =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "chat invite")?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let admin_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
            match plan.link.clone() {
                Some(link) => {
                    let mut rows = Vec::new();
                    let mut offset_date = 0i32;
                    let mut offset_user: tl::enums::InputUser = tl::types::InputUserEmpty {}.into();
                    loop {
                        let remaining = INVITE_LIST_LIMIT - rows.len() as i32;
                        if remaining <= 0 {
                            break;
                        }
                        let r: tl::enums::messages::ChatInviteImporters = shares
                            .client
                            .invoke(&tl::functions::messages::GetChatInviteImporters {
                                requested: false,
                                subscription_expired: false,
                                peer: peer.clone(),
                                link: Some(link.clone()),
                                q: None,
                                offset_date,
                                offset_user: offset_user.clone(),
                                limit: remaining.min(INVITE_LIST_LIMIT),
                            })
                            .await
                            .map_err(tele_invocation)?;
                        let tl::enums::messages::ChatInviteImporters::Importers(ref list) = r;
                        let page_len = list.importers.len();
                        if page_len > 0 {
                            if let Some(tl::enums::ChatInviteImporter::Importer(imp)) =
                                list.importers.last()
                            {
                                offset_date = imp.date;
                                offset_user = tl::types::InputUser {
                                    user_id: imp.user_id,
                                    access_hash: 0,
                                }
                                .into();
                            }
                        }
                        rows.extend(chat_invite_importers_rows(&shares.client, &r));
                        if page_len == 0 {
                            break;
                        }
                    }
                    Ok(serde_json::json!({"importers": rows}))
                }
                None => {
                    let mut rows = Vec::new();
                    let mut offset_date: Option<i32> = None;
                    let mut offset_link: Option<String> = None;
                    loop {
                        let remaining = INVITE_LIST_LIMIT - rows.len() as i32;
                        if remaining <= 0 {
                            break;
                        }
                        let r: tl::enums::messages::ExportedChatInvites = shares
                            .client
                            .invoke(&tl::functions::messages::GetExportedChatInvites {
                                revoked: plan.revoked,
                                peer: peer.clone(),
                                admin_id: admin_id.clone(),
                                offset_date,
                                offset_link: offset_link.clone(),
                                limit: remaining.min(INVITE_LIST_LIMIT),
                            })
                            .await
                            .map_err(tele_invocation)?;
                        let tl::enums::messages::ExportedChatInvites::Invites(ref list) = r;
                        let page_len = list.invites.len();
                        if page_len > 0 {
                            if let Some(tl::enums::ExportedChatInvite::ChatInviteExported(inv)) =
                                list.invites.last()
                            {
                                offset_date = Some(inv.date);
                                offset_link = Some(inv.link.clone());
                            }
                        }
                        rows.extend(exported_chat_invites_rows(&r));
                        if page_len == 0 {
                            break;
                        }
                    }
                    Ok(serde_json::json!({"links": rows}))
                }
            }
        }
        InviteMode::Check => {
            let hash = plan.hash.clone().unwrap_or_default();
            let r: tl::enums::ChatInvite = shares
                .client
                .invoke(&tl::functions::messages::CheckChatInvite { hash })
                .await
                .map_err(tele_invocation)?;
            let row = check_invite_row(&r);
            Ok(serde_json::json!({"invite": row}))
        }
        InviteMode::DeleteRevoked => {
            let chat =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "chat invite")?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let admin_id: tl::enums::InputUser = tl::types::InputUserSelf {}.into();
            let deleted: bool = shares
                .client
                .invoke(&tl::functions::messages::DeleteRevokedExportedChatInvites {
                    peer,
                    admin_id,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({"deleted_revoked": deleted}))
        }
    }
}

pub(crate) async fn chat_requests_core(
    shares: &crate::client::ServeShares,
    params: RequestsServeParams,
) -> TeleResult<serde_json::Value> {
    let args = RequestsArgs::from(&params);
    let plan = validate_requests(&args)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "requests")?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    match plan.action {
        RequestsAction::List => {
            let mut rows = Vec::new();
            let mut offset_date = 0i32;
            let mut offset_user: tl::enums::InputUser = tl::types::InputUserEmpty {}.into();
            let limit = plan.limit as i32;
            loop {
                let remaining = limit - rows.len() as i32;
                if remaining <= 0 {
                    break;
                }
                let r: tl::enums::messages::ChatInviteImporters = shares
                    .client
                    .invoke(&tl::functions::messages::GetChatInviteImporters {
                        requested: true,
                        subscription_expired: false,
                        peer: peer.clone(),
                        link: plan.link.clone(),
                        q: None,
                        offset_date,
                        offset_user: offset_user.clone(),
                        limit: remaining.min(INVITE_LIST_LIMIT),
                    })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::messages::ChatInviteImporters::Importers(ref list) = r;
                let page_len = list.importers.len();
                if page_len > 0 {
                    if let Some(tl::enums::ChatInviteImporter::Importer(imp)) =
                        list.importers.last()
                    {
                        offset_date = imp.date;
                        offset_user = tl::types::InputUser {
                            user_id: imp.user_id,
                            access_hash: 0,
                        }
                        .into();
                    }
                }
                rows.extend(join_request_rows(&shares.client, &r, plan.link.as_deref()));
                if page_len == 0 {
                    break;
                }
            }
            Ok(serde_json::json!({"requests": rows}))
        }
        RequestsAction::Approve | RequestsAction::Dismiss => {
            let approved = plan.action == RequestsAction::Approve;
            match &plan.user {
                Some(user) => {
                    let user_peer =
                        entities::resolve_peer(&shares.client, shares.session.as_ref(), user)
                            .await?;
                    let user_input = entities::input_user(&user_peer)
                        .await
                        .map_err(tele_invocation)?;
                    shares.rate_limiter.acquire().await;
                    shares
                        .client
                        .invoke(&tl::functions::messages::HideChatJoinRequest {
                            approved,
                            peer,
                            user_id: user_input,
                        })
                        .await
                        .map_err(tele_invocation)?;
                    Ok(serde_json::json!({
                        "chat": params.chat,
                        "user": user,
                        "action": if approved { "approved" } else { "dismissed" }}))
                }
                None => {
                    shares.rate_limiter.acquire().await;
                    shares
                        .client
                        .invoke(&tl::functions::messages::HideAllChatJoinRequests {
                            approved,
                            peer,
                            link: plan.link.clone(),
                        })
                        .await
                        .map_err(tele_invocation)?;
                    let mut v = serde_json::json!({
                        "chat": params.chat,
                        "all": plan.all,
                        "action": if approved { "approved" } else { "dismissed" }});
                    if let Some(link) = &plan.link {
                        v["link"] = serde_json::json!(link);
                    }
                    Ok(v)
                }
            }
        }
    }
}

pub(crate) async fn chat_participants_core(
    shares: &crate::client::ServeShares,
    params: ParticipantsServeParams,
) -> TeleResult<serde_json::Value> {
    let role = parse_participant_role(params.role.as_deref())?;
    let search = params
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    ensure_chat_peer(&chat, "participants")?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let mut rows = Vec::new();
    if matches!(&chat, grammers_client::peer::Peer::Group(_)) && !entities::is_channel(&chat) {
        if role.is_some() || search.is_some() {
            return Err(TeleError::Usage(
                "--role/--search filters require a channel or supergroup; basic groups list all members".to_string(),
            ));
        }
        let full = shares
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
        let (mut basic_rows, missing) = participant_rows(&shares.client, &users, &participants);
        basic_rows.truncate(params.limit as usize);
        if missing > 0 {
            output::log_line(
                "warn",
                &format!("{missing} participant(s) missing user data were skipped"),
            );
        }
        rows = basic_rows;
    } else {
        let mut count = 0u32;
        let mut iter = shares
            .client
            .iter_participants(chat_ref)
            .filter(participant_filter(role, search.as_deref()));
        while count < params.limit {
            match iter.next().await.map_err(tele_invocation)? {
                Some(p) => {
                    rows.push(serde_json::json!({
                        "id": p.user.id().bare_id().unwrap_or_default(),
                        "name": crate::serialize::peer_name(&grammers_client::peer::Peer::User(p.user)),
                        "role": role_name(&p.role)}));
                    count += 1;
                }
                None => break,
            }
        }
    }
    Ok(serde_json::json!({"participants": rows}))
}

crate::serve_runner!(run_join, chat_join_core, JoinParams);
crate::serve_runner!(run_leave, chat_leave_core, LeaveParams);
crate::serve_runner!(run_create, chat_create_core, CreateServeParams);
crate::serve_runner!(run_settings, chat_settings_core, SettingsServeParams);
crate::serve_runner!(run_edit, chat_edit_core, EditServeParams);
crate::serve_runner!(run_link, chat_link_core, LinkServeParams);
crate::serve_runner!(run_kick, chat_kick_core, KickServeParams);
crate::serve_runner!(run_admin, chat_admin_core, AdminServeParams);
crate::serve_runner!(run_admin_log, chat_admin_log_core, AdminLogServeParams);
crate::serve_runner!(run_stats, chat_stats_core, StatsServeParams);
crate::serve_runner!(run_invite, chat_invite_core, InviteServeParams);
crate::serve_runner!(run_requests, chat_requests_core, RequestsServeParams);
crate::serve_runner!(
    run_participants,
    chat_participants_core,
    ParticipantsServeParams
);

pub(crate) fn chat_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "chat join",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "join a chat by target or invite link",
            JoinParams,
            ChatArgs,
            validate_join,
            join_serve_dry_run,
            run_join,
            crate::commands::serve::params_schema::<JoinParams>
        ),
        crate::serve_route!(
            "chat leave",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            true,
            true,
            "leave a chat or channel",
            LeaveParams,
            ChatArgs,
            validate_leave,
            leave_serve_dry_run,
            run_leave,
            crate::commands::serve::params_schema::<LeaveParams>
        ),
        crate::serve_route!(
            "chat create",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "create a group, supergroup, or channel",
            CreateServeParams,
            CreateArgs,
            validate_create,
            create_serve_dry_run,
            run_create,
            crate::commands::serve::params_schema::<CreateServeParams>
        ),
        crate::serve_route!(
            "chat settings",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "update or read channel settings",
            SettingsServeParams,
            SettingsArgs,
            validate_settings,
            settings_serve_dry_run,
            run_settings,
            crate::commands::serve::params_schema::<SettingsServeParams>
        ),
        crate::serve_route!(
            "chat edit",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "edit chat title, about, or photo",
            EditServeParams,
            EditArgs,
            validate_edit,
            edit_serve_dry_run,
            run_edit,
            crate::commands::serve::params_schema::<EditServeParams>
        ),
        crate::serve_route!(
            "chat link",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "show or set the discussion link of a channel",
            LinkServeParams,
            LinkArgs,
            validate_link,
            link_serve_dry_run,
            run_link,
            crate::commands::serve::params_schema::<LinkServeParams>
        ),
        crate::serve_route!(
            "chat kick",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            true,
            true,
            "kick, ban, or restrict a chat participant",
            KickServeParams,
            KickArgs,
            validate_kick,
            kick_serve_dry_run,
            run_kick,
            crate::commands::serve::params_schema::<KickServeParams>
        ),
        crate::serve_route!(
            "chat admin",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "promote or demote a chat admin",
            AdminServeParams,
            AdminArgs,
            validate_admin,
            admin_serve_dry_run,
            run_admin,
            crate::commands::serve::params_schema::<AdminServeParams>
        ),
        crate::serve_route!(
            "chat admin-log",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list recent chat admin log events",
            AdminLogServeParams,
            AdminLogArgs,
            validate_admin_log,
            adminlog_serve_dry_run,
            run_admin_log,
            crate::commands::serve::params_schema::<AdminLogServeParams>
        ),
        crate::serve_route!(
            "chat stats",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "show broadcast or megagroup stats",
            StatsServeParams,
            StatsArgs,
            validate_stats,
            stats_serve_dry_run,
            run_stats,
            crate::commands::serve::params_schema::<StatsServeParams>
        ),
        crate::serve_route!(
            "chat invite",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "invite users or manage invite links",
            InviteServeParams,
            InviteArgs,
            validate_invite_serve,
            invite_serve_dry_run,
            run_invite,
            crate::commands::serve::params_schema::<InviteServeParams>
        ),
        crate::serve_route!(
            "chat requests",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "list or act on pending join requests",
            RequestsServeParams,
            RequestsArgs,
            validate_requests_serve,
            requests_serve_dry_run,
            run_requests,
            crate::commands::serve::params_schema::<RequestsServeParams>
        ),
        crate::serve_route!(
            "chat participants",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list chat participants",
            ParticipantsServeParams,
            ParticipantsArgs,
            validate_participants,
            participants_serve_dry_run,
            run_participants,
            crate::commands::serve::params_schema::<ParticipantsServeParams>
        ),
    ]
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
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
        let collected = collect_admin_log(2, None, |_max_id, _limit| {
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
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        assert_eq!(filter_events_by_range(rows, strict_since, None).len(), 0);
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
        let collected = collect_admin_log(10, None, |max_id, page_limit| {
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
        let collected = collect_admin_log(5, None, |max_id, page_limit| {
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
        let collected = collect_admin_log(5, None, |max_id, page_limit| {
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
        let collected = collect_admin_log(3, None, |max_id, page_limit| {
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
        let collected = collect_admin_log(250, None, |max_id, page_limit| {
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
                    chat: None,
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
                    check: None
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
                    limit: 10
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
                    rights: None
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
                    events: None
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
                    broadcast: false
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
            chat: Some(chat.to_string()),
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
            check: None,
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
    fn resolve_admin_rights_unknown_preset_is_a_usage_error_not_all() {
        let mut args = AdminArgs {
            chat: "c".to_string(),
            user: "u".to_string(),
            promote: true,
            demote: false,
            title: None,
            preset: Some("modrator".to_string()),
            rights: None,
        };
        assert!(matches!(
            resolve_admin_rights(&args),
            Err(TeleError::Usage(_))
        ));
        args.preset = Some("".to_string());
        assert!(matches!(
            resolve_admin_rights(&args),
            Err(TeleError::Usage(_))
        ));
        args.preset = Some("Admin".to_string());
        assert!(matches!(
            resolve_admin_rights(&args),
            Err(TeleError::Usage(_))
        ));
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

    fn requests_args(chat: &str) -> RequestsArgs {
        RequestsArgs {
            chat: chat.to_string(),
            user: None,
            all: false,
            approve: false,
            dismiss: false,
            link: None,
            limit: 100,
        }
    }

    #[test]
    fn requests_missing_chat_is_usage_error() {
        assert!(matches!(
            validate_requests(&requests_args("")),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_requests(&requests_args("   ")),
            Err(TeleError::Usage(_))
        ));
    }

    #[tokio::test]
    async fn requests_reject_empty_chat_before_connect() {
        let flags = dryrun_flags("chat requests");
        assert!(matches!(
            requests(requests_args(""), &flags).await,
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn requests_mutate_requires_user_or_all() {
        let mut a = requests_args("@c");
        a.approve = true;
        assert!(
            matches!(validate_requests(&a), Err(TeleError::Usage(_))),
            "approve without --user/--all must fail"
        );
        let mut d = requests_args("@c");
        d.dismiss = true;
        assert!(
            matches!(validate_requests(&d), Err(TeleError::Usage(_))),
            "dismiss without --user/--all must fail"
        );
        d.user = Some("@bob".to_string());
        let plan = validate_requests(&d).unwrap();
        assert_eq!(plan.action, RequestsAction::Dismiss);
        assert_eq!(plan.user.as_deref(), Some("@bob"));
        a.user = Some("+989121234567".to_string());
        assert_eq!(
            validate_requests(&a).unwrap().action,
            RequestsAction::Approve
        );
        a.user = None;
        a.all = true;
        let plan = validate_requests(&a).unwrap();
        assert_eq!(plan.action, RequestsAction::Approve);
        assert!(plan.all);
    }

    #[test]
    fn requests_all_conflicts_with_user() {
        let mut a = requests_args("@c");
        a.approve = true;
        a.all = true;
        a.user = Some("@bob".to_string());
        assert!(matches!(validate_requests(&a), Err(TeleError::Usage(_))));
        let mut d = requests_args("@c");
        d.dismiss = true;
        d.all = true;
        d.user = Some("@bob".to_string());
        assert!(matches!(validate_requests(&d), Err(TeleError::Usage(_))));
    }

    #[test]
    fn requests_approve_and_dismiss_conflict() {
        let mut a = requests_args("@c");
        a.approve = true;
        a.dismiss = true;
        a.all = true;
        assert!(matches!(validate_requests(&a), Err(TeleError::Usage(_))));
    }

    #[test]
    fn requests_list_rejects_mutator_only_flags() {
        let mut a = requests_args("@c");
        a.user = Some("@bob".to_string());
        assert!(matches!(validate_requests(&a), Err(TeleError::Usage(_))));
        let mut b = requests_args("@c");
        b.all = true;
        assert!(matches!(validate_requests(&b), Err(TeleError::Usage(_))));
    }

    #[test]
    fn requests_limit_and_link_validate_offline() {
        let mut a = requests_args("@c");
        a.limit = 10_001;
        assert!(matches!(validate_requests(&a), Err(TeleError::Usage(_))));
        a.limit = 200;
        a.link = Some("+abc123".to_string());
        let plan = validate_requests(&a).unwrap();
        assert_eq!(plan.action, RequestsAction::List);
        assert_eq!(plan.link.as_deref(), Some("+abc123"));
        let mut b = requests_args("@c");
        b.link = Some("@notalink".to_string());
        assert!(matches!(validate_requests(&b), Err(TeleError::Usage(_))));
    }

    #[test]
    fn requests_mutators_require_explicit_account_selection() {
        let mut flags = dryrun_flags("chat requests");
        flags.account.clear();
        flags.dry_run = false;
        assert!(crate::executor::require_explicit_selection("chat requests", &flags).is_err());
        flags.account.push("me".to_string());
        assert!(crate::executor::require_explicit_selection("chat requests", &flags).is_ok());
        flags.account.clear();
        flags.tag.push("ops".to_string());
        assert!(crate::executor::require_explicit_selection("chat requests", &flags).is_ok());
    }

    #[test]
    fn requests_dry_run_payloads_echo_action_scope_and_would() {
        let mut plan = ValidatedRequests {
            action: RequestsAction::Approve,
            user: Some("@bob".to_string()),
            ..Default::default()
        };
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["action"], serde_json::json!("approve"));
        assert_eq!(v["user"], serde_json::json!("@bob"));
        assert_eq!(
            v["would"],
            serde_json::json!("approve join request of @bob in chat @c")
        );

        plan.user = None;
        plan.all = true;
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(v["all"], serde_json::json!(true));
        assert_eq!(
            v["would"],
            serde_json::json!("approve every pending join request of chat @c")
        );
        assert!(v.get("link").is_none());

        plan.link = Some("https://t.me/+abc".to_string());
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(v["link"], serde_json::json!("https://t.me/+abc"));

        plan.action = RequestsAction::Dismiss;
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(v["action"], serde_json::json!("dismiss"));

        plan.all = false;
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(
            v["would"],
            serde_json::json!("dismiss join request of chat @c via https://t.me/+abc")
        );

        plan.link = None;
        plan.action = RequestsAction::List;
        let v = requests_dry_run_payload("@c", &plan);
        assert_eq!(v["action"], serde_json::json!("list"));
        assert_eq!(
            v["would"],
            serde_json::json!("list pending join requests of chat @c")
        );
    }

    #[test]
    fn invite_check_normalizes_link_forms() {
        let mut a = InviteArgs {
            chat: None,
            ..invite_args("")
        };
        a.check = Some("t.me/+abc123".to_string());
        let p = validate_invite(&a).unwrap();
        assert_eq!(p.mode, InviteMode::Check);
        assert_eq!(p.link.as_deref(), Some("https://t.me/+abc123"));
        assert_eq!(p.hash.as_deref(), Some("abc123"));

        a.check = Some("+abc-xyz_1".to_string());
        let p = validate_invite(&a).unwrap();
        assert_eq!(p.link.as_deref(), Some("+abc-xyz_1"));
        assert_eq!(p.hash.as_deref(), Some("abc-xyz_1"));

        a.check = Some("  t.me/joinchat/plainhash  ".to_string());
        let p = validate_invite(&a).unwrap();
        assert_eq!(p.hash.as_deref(), Some("plainhash"));

        a.check = Some("https://t.me/+AbC?start=1".to_string());
        let p = validate_invite(&a).unwrap();
        assert_eq!(p.hash.as_deref(), Some("AbC"));
    }

    #[test]
    fn invite_check_works_without_chat_and_rejects_chat_pairing() {
        let a = InviteArgs {
            chat: None,
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
            check: Some("+abc123".to_string()),
        };
        let p = validate_invite(&a).unwrap();
        assert_eq!(p.mode, InviteMode::Check);
        assert_eq!(p.hash.as_deref(), Some("abc123"));

        let mut paired = a.clone();
        paired.chat = Some("@c".to_string());
        assert!(matches!(validate_invite(&paired), Err(TeleError::Usage(_))));

        let mut no_chat = invite_args("");
        assert!(matches!(
            validate_invite(&no_chat),
            Err(TeleError::Usage(_))
        ));
        no_chat.list = true;
        assert!(matches!(
            validate_invite(&no_chat),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn invite_check_rejects_non_links_and_conflicts() {
        let bad = |mutate: &dyn Fn(&mut InviteArgs)| {
            let mut a = invite_args("@c");
            mutate(&mut a);
            assert!(
                matches!(validate_invite(&a), Err(TeleError::Usage(_))),
                "expected Usage error"
            );
        };
        bad(&|a| a.check = Some("@username".into()));
        bad(&|a| a.check = Some("12345".into()));
        bad(&|a| a.check = Some(String::new()));
        bad(&|a| a.check = Some("+989121234567".into()));
        bad(&|a| {
            a.check = Some("+abc".into());
            a.title = Some("t".into());
        });
        bad(&|a| {
            a.check = Some("+abc".into());
            a.list = true;
        });
        bad(&|a| {
            a.check = Some("+abc".into());
            a.user = Some("u".into());
        });
        bad(&|a| {
            a.check = Some("+abc".into());
            a.edit = Some("+x".into());
        });
        bad(&|a| {
            a.check = Some("+abc".into());
            a.delete_revoked = true;
        });
        bad(&|a| {
            a.check = Some("+abc".into());
            a.revoke = true;
        });
    }

    fn preview_channel(id: i64, title: &str) -> tl::types::Channel {
        tl::types::Channel {
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
            id,
            access_hash: None,
            title: title.to_string(),
            username: None,
            photo: tl::enums::ChatPhoto::Empty,
            date: 0,
            restriction_reason: None,
            admin_rights: None,
            banned_rights: None,
            default_banned_rights: None,
            participants_count: Some(4321),
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
        }
    }

    #[test]
    fn check_row_shapes_already_variant_with_resolved_chat() {
        let invite = tl::enums::ChatInvite::Already(tl::types::ChatInviteAlready {
            chat: tl::enums::Chat::Channel(tl::types::Channel {
                broadcast: false,
                megagroup: true,
                participants_count: None,
                ..preview_channel(42, "Dev Group")
            }),
        });
        let row = check_invite_row(&invite);
        assert_eq!(row["kind"], "already");
        assert_eq!(row["id"], 42);
        assert_eq!(row["title"], "Dev Group");
        assert_eq!(row["chat_kind"], "supergroup");
        assert!(row["participants_count"].is_null());
        assert!(row["expires"].is_null());
        assert!(row["request_needed"].is_null());

        let forbidden = tl::enums::ChatInvite::Peek(tl::types::ChatInvitePeek {
            chat: tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
                broadcast: true,
                megagroup: false,
                monoforum: false,
                id: 77,
                access_hash: 1,
                title: "News".to_string(),
                until_date: None,
            }),
            expires: 86_400,
        });
        let row = check_invite_row(&forbidden);
        assert_eq!(row["kind"], "peek");
        assert_eq!(row["id"], 77);
        assert_eq!(row["title"], "News");
        assert_eq!(row["chat_kind"], "channel");
        assert!(row["participants_count"].is_null());
        assert_eq!(row["expires"], 86_400);
        assert!(row["request_needed"].is_null());
    }

    #[test]
    fn check_row_shapes_invite_variant_flags_and_counts() {
        let invite = tl::enums::ChatInvite::Invite(tl::types::ChatInvite {
            channel: true,
            broadcast: false,
            public: false,
            megagroup: true,
            request_needed: true,
            verified: false,
            scam: false,
            fake: false,
            can_refulfill_subscription: false,
            title: "Dev".to_string(),
            about: Some("about text".to_string()),
            photo: tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 }),
            participants_count: 120,
            participants: Some(vec![test_user(1, "alice")]),
            color: 0,
            subscription_pricing: None,
            subscription_form_id: None,
            bot_verification: None,
        });
        let row = check_invite_row(&invite);
        assert_eq!(row["kind"], "invite");
        assert_eq!(row["title"], "Dev");
        assert_eq!(row["about"], "about text");
        assert_eq!(row["participants_count"], 120);
        assert_eq!(row["request_needed"], true);
        assert_eq!(row["megagroup"], true);
        assert_eq!(row["broadcast"], false);
        assert_eq!(row["participants_preview"], 1);
        assert!(row["expires"].is_null());
    }

    #[test]
    fn invite_hash_extracts_from_all_link_forms() {
        for (input, want) in [
            ("https://t.me/+AbC", Some("AbC")),
            ("http://t.me/joinchat/XyZ_-9", Some("XyZ_-9")),
            ("https://telegram.me/+hash1", Some("hash1")),
            ("+bare-hash", Some("bare-hash")),
            ("plain_hash-1", Some("plain_hash-1")),
            ("@username", None),
            ("12345", None),
            ("", None),
        ] {
            assert_eq!(invite_hash_from_link(input).as_deref(), want, "for {input}");
        }
    }

    #[test]
    fn request_rows_carry_link_echo_only_when_filtered() {
        let client = offline_client();
        let importers = tl::enums::messages::ChatInviteImporters::Importers(
            tl::types::messages::ChatInviteImporters {
                count: 2,
                importers: vec![
                    tl::enums::ChatInviteImporter::Importer(tl::types::ChatInviteImporter {
                        requested: true,
                        via_chatlist: false,
                        user_id: 11,
                        date: 1_700_000_000,
                        about: Some("hi".to_string()),
                        approved_by: None,
                    }),
                    tl::enums::ChatInviteImporter::Importer(tl::types::ChatInviteImporter {
                        requested: true,
                        via_chatlist: false,
                        user_id: 12,
                        date: 1_700_000_050,
                        about: None,
                        approved_by: None,
                    }),
                ],
                users: vec![test_user(11, "carol")],
            },
        );
        let rows = join_request_rows(&client, &importers, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], 11);
        assert_eq!(rows[0]["name"], "carol");
        assert_eq!(rows[0]["date"], "2023-11-14T22:13:20+00:00");
        assert!(rows[0].get("link").is_none());

        let rows = join_request_rows(&client, &importers, Some("https://t.me/+abc"));
        assert_eq!(rows[0]["link"], "https://t.me/+abc");
        assert_eq!(rows[1]["link"], "https://t.me/+abc");

        print_request_table("acc", false, &rows).unwrap();
    }
}

#[cfg(test)]
mod chat_serve_tests {
    use super::*;
    use crate::commands::serve::{Lane, Plan};

    fn plan_chat_op(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
        let route = chat_serve_routes()
            .into_iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    fn serve_error(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "ServeError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn usage_error(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "UsageError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn expect_execute(plan: Plan, raw: &serde_json::Value) {
        match plan {
            Plan::Execute(passed) => assert_eq!(&passed, raw),
            other => panic!("expected execute plan, got {other:?}"),
        }
    }

    fn expect_dry_run(plan: Plan, expected: serde_json::Value) {
        match plan {
            Plan::DryRun(data) => assert_eq!(data, expected),
            other => panic!("expected dry run plan, got {other:?}"),
        }
    }

    #[test]
    fn every_chat_route_rejects_unknown_fields() {
        let cases: &[(&str, serde_json::Value)] = &[
            ("chat join", serde_json::json!({"chat": "@room"})),
            ("chat leave", serde_json::json!({"chat": "@room"})),
            ("chat create", serde_json::json!({"title": "T"})),
            (
                "chat settings",
                serde_json::json!({"chat": "@room", "slow_mode": "30"}),
            ),
            (
                "chat edit",
                serde_json::json!({"chat": "@room", "title": "n"}),
            ),
            ("chat link", serde_json::json!({"chat": "@room"})),
            (
                "chat kick",
                serde_json::json!({"chat": "@room", "user": "@u"}),
            ),
            (
                "chat admin",
                serde_json::json!({"chat": "@room", "user": "@u", "promote": true}),
            ),
            ("chat admin-log", serde_json::json!({"chat": "@room"})),
            ("chat stats", serde_json::json!({"chat": "@room"})),
            (
                "chat invite",
                serde_json::json!({"chat": "@room", "list": true}),
            ),
            ("chat requests", serde_json::json!({"chat": "@room"})),
            ("chat participants", serde_json::json!({"chat": "@room"})),
        ];
        for (op, base) in cases {
            let mut bad = base.clone();
            bad["bogus"] = serde_json::json!(1);
            let msg = serve_error(plan_chat_op(op, bad).unwrap_err());
            assert!(msg.contains("unknown field"), "{op}: {msg}");
            assert!(msg.contains("bogus"), "{op}: {msg}");
        }
    }

    #[test]
    fn chat_join_plan_matrix() {
        let msg = usage_error(plan_chat_op("chat join", serde_json::json!({})).unwrap_err());
        assert!(msg.contains("--chat must not be empty"), "{msg}");

        let msg =
            serve_error(plan_chat_op("chat join", serde_json::json!({"chat": 5})).unwrap_err());
        assert!(msg.contains("string"), "{msg}");

        let msg =
            usage_error(plan_chat_op("chat join", serde_json::json!({"chat": ""})).unwrap_err());
        assert!(!msg.is_empty(), "{msg}");

        let plan = plan_chat_op(
            "chat join",
            serde_json::json!({"chat": "@room", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "@room",
                "would": "join chat @room"
            }),
        );

        let raw = serde_json::json!({"chat": "@room"});
        let plan = plan_chat_op("chat join", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_leave_plan_matrix() {
        let msg = usage_error(plan_chat_op("chat leave", serde_json::json!({})).unwrap_err());
        assert!(msg.contains("--chat must not be empty"), "{msg}");

        let msg =
            serve_error(plan_chat_op("chat leave", serde_json::json!({"chat": 9})).unwrap_err());
        assert!(msg.contains("string"), "{msg}");

        let plan = plan_chat_op(
            "chat leave",
            serde_json::json!({"chat": "@room", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "@room",
                "would": "leave chat @room"
            }),
        );

        let raw = serde_json::json!({"chat": "@room"});
        let plan = plan_chat_op("chat leave", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_create_plan_matrix() {
        let msg = serve_error(plan_chat_op("chat create", serde_json::json!({})).unwrap_err());
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("title"), "{msg}");

        let msg =
            serve_error(plan_chat_op("chat create", serde_json::json!({"title": 5})).unwrap_err());
        assert!(msg.contains("string"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat create",
                serde_json::json!({"title": "T", "kind": "guild"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("unknown chat kind"), "{msg}");

        let plan = plan_chat_op(
            "chat create",
            serde_json::json!({"title": "T", "kind": "supergroup", "forum": true, "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "title": "T",
                "kind": "supergroup",
                "forum": true,
                "would": "create supergroup chat \"T\""
            }),
        );

        let raw = serde_json::json!({"title": "T"});
        let plan = plan_chat_op("chat create", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_settings_plan_matrix() {
        let msg = serve_error(
            plan_chat_op(
                "chat settings",
                serde_json::json!({"chat": "work", "slow_mode": 30}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("string"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat settings",
                serde_json::json!({"chat": "work", "noforwards": "on"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("cannot be applied"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat settings",
                serde_json::json!({"chat": "work", "slow_mode": "9999"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--slow-mode"), "{msg}");

        let plan = plan_chat_op(
            "chat settings",
            serde_json::json!({"chat": "work", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "would": "read settings of chat work"
            }),
        );

        let plan = plan_chat_op(
            "chat settings",
            serde_json::json!({"chat": "work", "slow_mode": "60", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "would": "update settings of chat work",
                "slow_mode": 60
            }),
        );

        let raw = serde_json::json!({"chat": "work", "slow_mode": "60"});
        let plan = plan_chat_op("chat settings", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_edit_plan_matrix() {
        let msg = usage_error(
            plan_chat_op("chat edit", serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("at least one of"), "{msg}");

        let msg = serve_error(
            plan_chat_op("chat edit", serde_json::json!({"chat": "work", "photo": 5})).unwrap_err(),
        );
        assert!(msg.contains("string"), "{msg}");

        let plan = plan_chat_op(
            "chat edit",
            serde_json::json!({"chat": "work", "title": " New ", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "would": "edit metadata of chat work",
                "title": "New"
            }),
        );

        let raw = serde_json::json!({"chat": "work", "about": "hello"});
        let plan = plan_chat_op("chat edit", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_link_plan_matrix() {
        let msg = usage_error(
            plan_chat_op(
                "chat link",
                serde_json::json!({"chat": "work", "to": "remove"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("no unlink method"), "{msg}");

        let msg = serve_error(
            plan_chat_op("chat link", serde_json::json!({"chat": "work", "to": 5})).unwrap_err(),
        );
        assert!(msg.contains("string"), "{msg}");

        let plan = plan_chat_op(
            "chat link",
            serde_json::json!({"chat": "work", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "would": "read discussion link of chat work"
            }),
        );

        let plan = plan_chat_op(
            "chat link",
            serde_json::json!({"chat": "work", "to": "@discuss", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "to": "@discuss",
                "would": "link chat work with discussion group @discuss"
            }),
        );

        let raw = serde_json::json!({"chat": "work", "to": "@discuss"});
        let plan = plan_chat_op("chat link", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_kick_plan_matrix() {
        let msg = serve_error(
            plan_chat_op("chat kick", serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("user"), "{msg}");

        let msg = serve_error(
            plan_chat_op(
                "chat kick",
                serde_json::json!({"chat": "work", "user": "@u", "ban": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat kick",
                serde_json::json!({"chat": "work", "user": "@u", "duration": "abc"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--duration"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat kick",
                serde_json::json!({"chat": "work", "user": "@u", "rights": "nope:true"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("unknown right"), "{msg}");

        let plan = plan_chat_op(
            "chat kick",
            serde_json::json!({"chat": "work", "user": "@u", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "user": "@u",
                "ban": false,
                "would": "kick user @u from chat work"
            }),
        );

        let plan = plan_chat_op(
            "chat kick",
            serde_json::json!({
                "chat": "work",
                "user": "@u",
                "ban": true,
                "duration": "300",
                "rights": "view_messages:false",
                "dry_run": true
            }),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "user": "@u",
                "ban": true,
                "would": "kick user @u from chat work",
                "duration": 300,
                "rights": [["view_messages", false]]
            }),
        );

        let raw = serde_json::json!({"chat": "work", "user": "@u", "ban": true});
        let plan = plan_chat_op("chat kick", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_admin_plan_matrix() {
        let msg = serve_error(
            plan_chat_op("chat admin", serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("user"), "{msg}");

        let msg = serve_error(
            plan_chat_op(
                "chat admin",
                serde_json::json!({"chat": "work", "user": "@u", "promote": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin",
                serde_json::json!({"chat": "work", "user": "@u"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--promote or --demote required"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin",
                serde_json::json!({"chat": "work", "user": "@u", "promote": true, "demote": true}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("mutually exclusive"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin",
                serde_json::json!({"chat": "work", "user": "@u", "demote": true, "preset": "boss"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("unknown preset"), "{msg}");

        let plan = plan_chat_op(
            "chat admin",
            serde_json::json!({"chat": "work", "user": "@u", "demote": true, "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "user": "@u",
                "promote": false,
                "demote": true,
                "would": "change admin status of user @u in chat work"
            }),
        );

        let raw = serde_json::json!({"chat": "work", "user": "@u", "promote": true});
        let plan = plan_chat_op("chat admin", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_adminlog_plan_matrix() {
        let msg = serve_error(
            plan_chat_op(
                "chat admin-log",
                serde_json::json!({"chat": "work", "limit": "many"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin-log",
                serde_json::json!({"chat": "work", "events": "hugs"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("unknown --events flag"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin-log",
                serde_json::json!({"chat": "work", "since": "not-a-date"}),
            )
            .unwrap_err(),
        );
        assert!(!msg.is_empty(), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat admin-log",
                serde_json::json!({"chat": "work", "since": "200", "until": "100"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--since must not be after --until"), "{msg}");

        let plan = plan_chat_op(
            "chat admin-log",
            serde_json::json!({"chat": "work", "search": "rank", "admin": "me", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(plan, admin_log_dry_run_payload("work", "rank", false, true));

        let raw = serde_json::json!({"chat": "work"});
        let plan = plan_chat_op("chat admin-log", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_stats_plan_matrix() {
        let msg = serve_error(
            plan_chat_op(
                "chat stats",
                serde_json::json!({"chat": "work", "broadcast": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg =
            usage_error(plan_chat_op("chat stats", serde_json::json!({"chat": ""})).unwrap_err());
        assert!(!msg.is_empty(), "{msg}");

        let plan = plan_chat_op(
            "chat stats",
            serde_json::json!({"chat": "work", "broadcast": true, "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(plan, stats_dry_run_payload("work", true));

        let raw = serde_json::json!({"chat": "work"});
        let plan = plan_chat_op("chat stats", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_invite_plan_matrix() {
        let msg = usage_error(
            plan_chat_op(
                "chat invite",
                serde_json::json!({"chat": "work", "user": "@u", "list": true}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("mutually exclusive"), "{msg}");

        let msg = serve_error(
            plan_chat_op(
                "chat invite",
                serde_json::json!({"chat": "work", "usage_limit": "5"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error(
            plan_chat_op("chat invite", serde_json::json!({"chat": "", "list": true})).unwrap_err(),
        );
        assert!(!msg.is_empty(), "{msg}");

        let plan = plan_chat_op(
            "chat invite",
            serde_json::json!({"chat": "work", "list": true, "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "mode": "list",
                "revoked": false,
                "importers": null,
                "would": "list active invite links of chat work"
            }),
        );

        let plan = plan_chat_op(
            "chat invite",
            serde_json::json!({"check": "t.me/+abc123", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => {
                assert_eq!(data["mode"], serde_json::json!("check"));
                assert_eq!(
                    data["would"],
                    serde_json::json!("preview invite link https://t.me/+abc123")
                );
            }
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "work", "list": true});
        let plan = plan_chat_op("chat invite", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_requests_plan_matrix() {
        let msg = usage_error(
            plan_chat_op(
                "chat requests",
                serde_json::json!({"chat": "work", "approve": true}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("requires --user USER or --all"), "{msg}");

        let msg = serve_error(
            plan_chat_op(
                "chat requests",
                serde_json::json!({"chat": "work", "limit": "many"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat requests",
                serde_json::json!({"chat": "work", "all": true}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--all applies to"), "{msg}");

        let plan = plan_chat_op(
            "chat requests",
            serde_json::json!({"chat": "work", "approve": true, "user": "@u", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "action": "approve",
                "user": "@u",
                "would": "approve join request of @u in chat work"
            }),
        );

        let raw = serde_json::json!({"chat": "work", "dismiss": true, "all": true});
        let plan = plan_chat_op("chat requests", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_participants_plan_matrix() {
        let msg = serve_error(
            plan_chat_op(
                "chat participants",
                serde_json::json!({"chat": "work", "limit": "many"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat participants",
                serde_json::json!({"chat": "work", "role": "boss"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("unknown role"), "{msg}");

        let msg = usage_error(
            plan_chat_op(
                "chat participants",
                serde_json::json!({"chat": "work", "limit": 10_001}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("--limit"), "{msg}");

        let plan = plan_chat_op(
            "chat participants",
            serde_json::json!({"chat": "work", "role": "admin", "dry_run": true}),
        )
        .unwrap();
        expect_dry_run(
            plan,
            serde_json::json!({
                "dry_run": true,
                "chat": "work",
                "would": "list participants of chat work"
            }),
        );

        let raw = serde_json::json!({"chat": "work", "role": "admin"});
        let plan = plan_chat_op("chat participants", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn chat_serve_lane_and_hints_table_is_locked() {
        let expected: &[(&str, Lane, u64, bool, bool, bool)] = &[
            ("chat admin", Lane::Mutate, 30, false, false, true),
            ("chat admin-log", Lane::Read, 120, true, false, true),
            ("chat create", Lane::Mutate, 30, false, false, true),
            ("chat edit", Lane::Mutate, 30, false, false, true),
            ("chat invite", Lane::Mutate, 30, false, false, true),
            ("chat join", Lane::Mutate, 30, false, false, true),
            ("chat kick", Lane::Mutate, 30, false, true, true),
            ("chat leave", Lane::Mutate, 30, false, true, true),
            ("chat link", Lane::Mutate, 30, false, false, true),
            ("chat participants", Lane::Read, 120, true, false, true),
            ("chat requests", Lane::Mutate, 30, false, false, true),
            ("chat settings", Lane::Mutate, 30, false, false, true),
            ("chat stats", Lane::Read, 120, true, false, true),
        ];
        let routes = chat_serve_routes();
        assert_eq!(routes.len(), expected.len());
        let mut destructive: Vec<&str> = Vec::new();
        for (op, lane, secs, read_only, is_destructive, retry_safe) in expected {
            let route = routes
                .iter()
                .find(|r| r.op == *op)
                .unwrap_or_else(|| panic!("route missing for {op}"));
            assert_eq!(route.lane, *lane, "lane for {op}");
            assert_eq!(
                route.timeout,
                Some(std::time::Duration::from_secs(*secs)),
                "timeout for {op}"
            );
            assert_eq!(route.read_only, *read_only, "read_only for {op}");
            assert_eq!(route.destructive, *is_destructive, "destructive for {op}");
            assert_eq!(route.retry_safe, *retry_safe, "retry_safe for {op}");
            if *is_destructive {
                destructive.push(op);
            }
        }
        assert_eq!(destructive, vec!["chat kick", "chat leave"]);
    }

    #[test]
    fn create_schema_declares_required_title_only() {
        let s = crate::commands::serve::params_schema::<CreateServeParams>();
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], serde_json::Value::Bool(false));
        for prop in ["title", "description", "kind", "forum", "dry_run"] {
            assert!(s["properties"][prop].is_object(), "{prop}");
        }
        let required: Vec<&str> = s["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert_eq!(required, vec!["title"]);
    }

    #[test]
    fn kick_schema_declares_required_user_only() {
        let s = crate::commands::serve::params_schema::<KickServeParams>();
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], serde_json::Value::Bool(false));
        for prop in ["chat", "user", "ban", "duration", "rights", "dry_run"] {
            assert!(s["properties"][prop].is_object(), "{prop}");
        }
        let required: Vec<&str> = s["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert_eq!(required, vec!["user"]);
    }
}
