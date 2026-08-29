#![allow(unused_imports)]
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

use super::*;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ParticipantRole {
    Admin,
    Banned,
    Kicked,
    Recent,
}

pub(crate) fn parse_participant_role(role: Option<&str>) -> TeleResult<Option<ParticipantRole>> {
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

pub(crate) fn participant_filter(
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

pub(crate) async fn participants(args: ParticipantsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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

pub(crate) const BANNED_RIGHT_NAMES: &[&str] = &[
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

pub(crate) fn parse_ban_duration(duration: Option<&str>) -> TeleResult<Option<u32>> {
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

pub(crate) fn parse_banned_rights_csv(csv: &str) -> TeleResult<Vec<(String, bool)>> {
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

pub(crate) fn validate_kick(args: &KickArgs) -> TeleResult<()> {
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

pub(crate) async fn kick(args: KickArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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

pub(crate) fn validate_admin(args: &AdminArgs) -> TeleResult<()> {
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
pub(crate) struct AdminRights {
    pub(crate) change_info: bool,
    pub(crate) post_messages: bool,
    pub(crate) edit_messages: bool,
    pub(crate) delete_messages: bool,
    pub(crate) ban_users: bool,
    pub(crate) invite_users: bool,
    pub(crate) pin_messages: bool,
    pub(crate) add_admins: bool,
    pub(crate) manage_call: bool,
    pub(crate) anonymous: bool,
    pub(crate) other: bool,
    pub(crate) manage_topics: bool,
}

impl AdminRights {
    pub(crate) fn none() -> Self {
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

    pub(crate) fn all() -> Self {
        Self::none().with_all_set()
    }

    pub(crate) fn with_all_set(mut self) -> Self {
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

    pub(crate) fn moderator() -> Self {
        Self {
            delete_messages: true,
            ban_users: true,
            invite_users: true,
            pin_messages: true,
            manage_topics: true,
            ..Self::none()
        }
    }

    pub(crate) fn editor() -> Self {
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

    pub(crate) fn from_string(s: &str) -> TeleResult<Self> {
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

    pub(crate) fn to_raw(self) -> tl::enums::ChatAdminRights {
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

    pub(crate) fn needs_raw_edit_admin(self) -> bool {
        self.other || self.manage_topics
    }
}

pub(crate) fn resolve_admin_rights(args: &AdminArgs) -> TeleResult<AdminRights> {
    if args.demote {
        return Ok(AdminRights::none());
    }
    if let Some(preset) = &args.preset {
        return Ok(match preset.as_str() {
            "moderator" => AdminRights::moderator(),
            "editor" => AdminRights::editor(),
            "admin" => AdminRights::all(),
            other => {
                return Err(TeleError::Usage(format!(
                    "unknown admin preset '{other}': use moderator, editor, or admin"
                )))
            }
        });
    }
    if let Some(rights_str) = &args.rights {
        return AdminRights::from_string(rights_str);
    }
    Ok(AdminRights::all())
}

pub(crate) async fn admin(args: AdminArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
            ensure_chat_peer(&chat, "chat")?;
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

pub(crate) fn role_name(role: &grammers_client::peer::Role) -> &'static str {
    match role {
        grammers_client::peer::Role::Creator(_) => "creator",
        grammers_client::peer::Role::Admin(_) => "admin",
        grammers_client::peer::Role::Banned(_) => "banned",
        grammers_client::peer::Role::User(_) => "member",
        grammers_client::peer::Role::Left(_) => "left",
        _ => "unknown",
    }
}

pub(crate) fn participant_rows(
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
