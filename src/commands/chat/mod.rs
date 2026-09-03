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

use admin_log::*;
use invite::*;
use participants::*;
use settings::*;

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
    crate::executor::require_explicit_selection("chat join", flags)?;
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    let params = JoinParams::from(&args);
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = params.chat.clone();
        let params = params.clone();
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
            chat_join_core(&guard.shares(), params).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn leave(args: ChatArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    crate::executor::require_explicit_selection("chat leave", flags)?;
    let params = LeaveParams::from(&args);
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = params.chat.clone();
        let params = params.clone();
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
            chat_leave_core(&guard.shares(), params).await
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
                        guard.rate_limiter.acquire().await;
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
                r["username"]
                    .as_str()
                    .unwrap_or(r["name"].as_str().unwrap_or_default())
                    .to_string(),
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
    crate::executor::require_explicit_selection("chat create", flags)?;
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
            let chat_id = peer
                .id()
                .bare_id()
                .ok_or_else(|| TeleError::Usage("peer id missing".to_string()))?;
            shares
                .client
                .invoke(&tl::functions::messages::DeleteChatUser {
                    chat_id,
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
    let until_ts = until.map(|u| u.timestamp() as i32);
    let collected = {
        let client_ref = &shares.client;
        let limiter = &shares.rate_limiter;
        let channel_ref = &channel;
        collect_admin_log(params.limit, until_ts, move |max_id, page_limit| {
            let q = search_q.clone();
            let filter = events_filter.clone();
            let admins = admins.clone();
            async move {
                limiter.acquire().await;
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
                        shares.rate_limiter.acquire().await;
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
                        shares.rate_limiter.acquire().await;
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
                shares.rate_limiter.acquire().await;
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
            shares.rate_limiter.acquire().await;
            match iter.next().await.map_err(tele_invocation)? {
                Some(p) => {
                    let peer = grammers_client::peer::Peer::User(p.user);
                    let mut row = crate::serialize::peer_key(&peer);
                    if let Some(obj) = row.as_object_mut() {
                        obj.insert("role".into(), serde_json::json!(role_name(&p.role)));
                    }
                    rows.push(row);
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
            |a: &ChatArgs| ChatTarget::parse_flag(&a.chat, "chat").map(|_| ()),
            |a: &ChatArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "chat": a.chat,
                "would": format!("join chat {}", a.chat)
            })),
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
            |a: &ChatArgs| ChatTarget::parse_flag(&a.chat, "chat").map(|_| ()),
            |a: &ChatArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "chat": a.chat,
                "would": format!("leave chat {}", a.chat)
            })),
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
            |a: &AdminArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "chat": a.chat,
                "user": a.user,
                "promote": a.promote,
                "demote": a.demote,
                "would": format!("change admin status of user {} in chat {}", a.user, a.chat)})),
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
            |a: &AdminLogArgs| Ok::<_, TeleError>(admin_log_dry_run_payload(
                &a.chat,
                a.search.as_deref().unwrap_or_default(),
                a.events.is_some(),
                a.admin.is_some(),
            )),
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
            |a: &StatsArgs| ChatTarget::parse_flag(&a.chat, "chat").map(|_| ()),
            |a: &StatsArgs| Ok::<_, TeleError>(stats_dry_run_payload(&a.chat, a.broadcast)),
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
            |a: &InviteArgs| validate_invite(a).map(|_| ()),
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
            |a: &RequestsArgs| validate_requests(a).map(|_| ()),
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
            |a: &ParticipantsArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "chat": a.chat,
                "would": format!("list participants of chat {}", a.chat)
            })),
            run_participants,
            crate::commands::serve::params_schema::<ParticipantsServeParams>
        ),
    ]
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "tests.rs"]
mod tests;
