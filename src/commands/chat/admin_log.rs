use grammers_client::tl;
use std::collections::HashMap;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

use super::*;
pub(crate) async fn admin_log(args: AdminLogArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
    let events_filter = parse_admin_events_filter(args.events.as_deref())?;
    let search_q = args.search.clone().unwrap_or_default();
    let admin_target = args.admin.clone();
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let multi = if dry_run {
        false
    } else {
        crate::executor::select_accounts(flags)?.len() > 1
    };
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
            let until_ts = until.map(|u| u.timestamp() as i32);
            let collected = {
                let guard_ref = &guard;
                let channel_ref = &channel;
                collect_admin_log(limit, until_ts, move |max_id, page_limit| {
                    let q = search_q.clone();
                    let filter = events_filter.clone();
                    let admins = admins.clone();
                    async move {
                        guard_ref.rate_limiter.acquire().await;
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
                let date = chrono::DateTime::from_timestamp(i64::from(event.date), 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
                rows.push(serde_json::json!({
                    "id": event.id,
                    "date": date,
                    "actor": actor_value(&guard.client, &collected.users, event.user_id),
                    "action": admin_action_summary(&event.action, own_id)}));
            }
            let rows = filter_events_by_range(rows, since, until);
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["date"].as_str().unwrap_or_default().to_string(),
                            r["actor"]["username"]
                                .as_str()
                                .unwrap_or(r["actor"]["name"].as_str().unwrap_or_default())
                                .to_string(),
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

pub(crate) fn admin_log_dry_run_payload(
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
        "would": format!("list admin log of chat {chat}")})
}

pub(crate) fn actor_value(
    client: &grammers_client::Client,
    users: &HashMap<i64, tl::enums::User>,
    user_id: i64,
) -> serde_json::Value {
    match users.get(&user_id) {
        Some(user) => {
            let peer = grammers_client::peer::Peer::User(grammers_client::peer::User::from_raw(
                client,
                user.clone(),
            ));
            crate::serialize::peer_key(&peer)
        }
        None => serde_json::json!({
            "id": user_id,
            "name": user_display_name(client, users, user_id)}),
    }
}

pub(crate) fn filter_events_by_range(
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
                .unwrap_or(false)
        })
        .collect()
}

pub(crate) const ADMIN_LOG_EVENT_FLAGS: &[&str] = &[
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

pub(crate) fn parse_admin_events_filter(
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

pub(crate) struct AdminLogPage {
    pub(crate) events: Vec<tl::enums::ChannelAdminLogEvent>,
    pub(crate) users: Vec<tl::enums::User>,
    pub(crate) max_id: i64,
}

pub(crate) struct CollectedAdminLog {
    pub(crate) events: Vec<tl::enums::ChannelAdminLogEvent>,
    pub(crate) users: HashMap<i64, tl::enums::User>,
}

pub(crate) async fn collect_admin_log<F, Fut>(
    limit: u32,
    until: Option<i32>,
    mut fetch: F,
) -> TeleResult<CollectedAdminLog>
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
        if let Some(until_ts) = until {
            let stopped = events.iter().any(|e| {
                let tl::enums::ChannelAdminLogEvent::Event(ev) = e;
                ev.date <= until_ts
            });
            if stopped {
                break;
            }
        }
    }
    Ok(CollectedAdminLog { events, users })
}

pub(crate) fn admin_action_summary(
    a: &tl::enums::ChannelAdminLogEventAction,
    own_id: i64,
) -> serde_json::Value {
    match a {
        tl::enums::ChannelAdminLogEventAction::ChangeTitle(v) => {
            serde_json::json!({
                "kind": "change_title",
                "title": v.new_value,
                "prev_title": v.prev_value})
        }
        tl::enums::ChannelAdminLogEventAction::ChangeAbout(v) => {
            serde_json::json!({
                "kind": "change_about",
                "text": v.new_value,
                "prev_text": v.prev_value})
        }
        tl::enums::ChannelAdminLogEventAction::ChangeUsername(v) => {
            serde_json::json!({
                "kind": "change_username",
                "username": v.new_value,
                "prev_username": v.prev_value})
        }
        tl::enums::ChannelAdminLogEventAction::SendMessage(v) => {
            message_action_summary("send_message", &v.message)
        }
        tl::enums::ChannelAdminLogEventAction::EditMessage(v) => {
            let mut out = message_action_summary("edit_message", &v.new_message);
            out["prev_text"] = serde_json::json!(message_text(&v.prev_message));
            if let tl::enums::Message::Message(m) = &v.prev_message {
                if let Some(markup) = &m.reply_markup {
                    out["prev_reply_markup"] = crate::serialize::reply_markup_to_json(markup);
                }
            }
            out
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
                "user_id": participant_user_id(&v.participant, own_id)})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantToggleBan(v) => {
            let mut value = serde_json::json!({
                "kind": "toggle_ban",
                "user_id": participant_user_id(&v.new_participant, own_id)});
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
                "user_id": participant_user_id(&v.new_participant, own_id)});
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
                "prev_photo": photo_summary(&v.prev_photo)})
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
                "via_chatlist": v.via_chatlist})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantJoinByRequest(v) => {
            serde_json::json!({
                "kind": "join_by_request",
                "approved_by": v.approved_by,
                "invite_link": invite_link(&v.invite)})
        }
        tl::enums::ChannelAdminLogEventAction::TogglePreHistoryHidden(v) => {
            serde_json::json!({"kind": "toggle_pre_history_hidden", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleSlowMode(v) => {
            serde_json::json!({
                "kind": "toggle_slow_mode",
                "seconds": v.new_value,
                "prev_seconds": v.prev_value})
        }
        tl::enums::ChannelAdminLogEventAction::ToggleNoForwards(v) => {
            serde_json::json!({"kind": "toggle_noforwards", "enabled": v.new_value})
        }
        tl::enums::ChannelAdminLogEventAction::DefaultBannedRights(v) => {
            serde_json::json!({
                "kind": "default_banned_rights",
                "rights": banned_rights_denied(&v.new_banned_rights),
                "until_date": banned_rights_until(&v.new_banned_rights),
                "prev_rights": banned_rights_denied(&v.prev_banned_rights)})
        }
        tl::enums::ChannelAdminLogEventAction::ChangeLinkedChat(v) => {
            serde_json::json!({
                "kind": "change_linked_chat",
                "linked_chat_id": v.new_value,
                "prev_linked_chat_id": v.prev_value})
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteDelete(v) => {
            serde_json::json!({
                "kind": "exported_invite_delete",
                "invite_link": invite_link_from_exported(&v.invite)})
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteRevoke(v) => {
            serde_json::json!({
                "kind": "exported_invite_revoke",
                "invite_link": invite_link_from_exported(&v.invite)})
        }
        tl::enums::ChannelAdminLogEventAction::ExportedInviteEdit(v) => {
            serde_json::json!({
                "kind": "exported_invite_edit",
                "invite_link": invite_link_from_exported(&v.new_invite),
                "prev_invite_link": invite_link_from_exported(&v.prev_invite)})
        }
        tl::enums::ChannelAdminLogEventAction::ParticipantEditRank(v) => {
            serde_json::json!({
                "kind": "edit_rank",
                "user_id": v.user_id,
                "rank": v.new_rank,
                "prev_rank": v.prev_rank})
        }
        _ => serde_json::json!({"kind": "other"}),
    }
}

pub(crate) fn message_id(message: &tl::enums::Message) -> Option<i32> {
    match message {
        tl::enums::Message::Message(m) => Some(m.id),
        _ => None,
    }
}

pub(crate) fn message_text(message: &tl::enums::Message) -> String {
    match message {
        tl::enums::Message::Message(m) => m.message.clone(),
        _ => String::new(),
    }
}

pub(crate) fn invite_link(invite: &tl::enums::ExportedChatInvite) -> serde_json::Value {
    match invite {
        tl::enums::ExportedChatInvite::ChatInviteExported(i) => serde_json::json!(i.link),
        _ => serde_json::Value::Null,
    }
}

pub(crate) fn invite_link_from_exported(
    invite: &tl::enums::ExportedChatInvite,
) -> serde_json::Value {
    invite_link(invite)
}

pub(crate) fn photo_summary(photo: &tl::enums::Photo) -> serde_json::Value {
    match photo {
        tl::enums::Photo::Empty(p) => serde_json::json!({"empty": true, "id": p.id}),
        tl::enums::Photo::Photo(p) => serde_json::json!({
            "id": p.id,
            "date": rfc3339_or_empty(Some(p.date)),
            "sizes": p.sizes.len()}),
    }
}

pub(crate) fn participant_ban_summary(
    participant: &tl::enums::ChannelParticipant,
) -> Option<serde_json::Value> {
    match participant {
        tl::enums::ChannelParticipant::Banned(p) => {
            let mut value = serde_json::json!({
                "left": p.left,
                "denied": banned_rights_denied(&p.banned_rights),
                "until_date": banned_rights_until(&p.banned_rights)});
            if p.rank.as_deref().is_some_and(|r| !r.is_empty()) {
                value["rank"] = serde_json::json!(p.rank);
            }
            Some(value)
        }
        _ => None,
    }
}

pub(crate) fn participant_admin_summary(
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

pub(crate) fn admin_rights_granted(rights: &tl::enums::ChatAdminRights) -> Vec<&'static str> {
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

pub(crate) fn banned_rights_denied(rights: &tl::enums::ChatBannedRights) -> Vec<&'static str> {
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

pub(crate) fn banned_rights_until(rights: &tl::enums::ChatBannedRights) -> Option<i32> {
    let tl::enums::ChatBannedRights::Rights(r) = rights;
    (r.until_date > 0).then_some(r.until_date)
}

pub(crate) fn message_action_summary(
    kind: &str,
    message: &tl::enums::Message,
) -> serde_json::Value {
    match message {
        tl::enums::Message::Message(m) => {
            let mut out = serde_json::json!({"kind": kind, "id": m.id, "text": m.message});
            if let Some(markup) = &m.reply_markup {
                out["reply_markup"] = crate::serialize::reply_markup_to_json(markup);
            }
            out
        }
        _ => serde_json::json!({"kind": kind}),
    }
}

pub(crate) fn admin_action_display(action: &serde_json::Value) -> String {
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
