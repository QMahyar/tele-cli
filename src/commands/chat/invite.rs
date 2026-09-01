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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InviteMode {
    User,
    Export,
    List,
    Edit,
    DeleteRevoked,
    Check,
}

pub(crate) fn validate_invite_link(input: &str) -> TeleResult<()> {
    if grammers_client::Client::parse_invite_link(input).is_some() || is_bare_invite_hash(input) {
        return Ok(());
    }
    Err(TeleError::Usage(format!(
        "not a valid invite link or chat target: \"{input}\""
    )))
}

pub(crate) fn normalize_invite_link(input: &str) -> String {
    let t = input.trim();
    if t.starts_with("t.me/") || t.starts_with("telegram.me/") {
        format!("https://{t}")
    } else {
        t.to_string()
    }
}

pub(crate) fn is_bare_invite_hash(input: &str) -> bool {
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

#[derive(Debug, Clone)]
pub struct ValidatedInvite {
    pub(crate) mode: InviteMode,
    pub(crate) user: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) hash: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) expire_date: Option<i32>,
    pub(crate) usage_limit: Option<i32>,
    pub(crate) request_needed: Option<bool>,
    pub(crate) revoked: bool,
}

impl Default for ValidatedInvite {
    fn default() -> Self {
        Self {
            mode: InviteMode::Export,
            user: None,
            link: None,
            hash: None,
            title: None,
            expire_date: None,
            usage_limit: None,
            request_needed: None,
            revoked: false,
        }
    }
}

pub(crate) async fn invite(args: InviteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let plan = validate_invite(&args)?;
    if !matches!(plan.mode, InviteMode::List | InviteMode::Check) {
        crate::executor::require_explicit_selection("chat invite", flags)?;
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone().unwrap_or_default();
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
                            let mut rows = Vec::new();
                            let mut offset_date = 0i32;
                            let mut offset_user: tl::enums::InputUser =
                                tl::types::InputUserEmpty {}.into();
                            loop {
                                let remaining = INVITE_LIST_LIMIT - rows.len() as i32;
                                if remaining <= 0 {
                                    break;
                                }
                                guard.rate_limiter.acquire().await;
                                let r: tl::enums::messages::ChatInviteImporters = guard
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
                                let tl::enums::messages::ChatInviteImporters::Importers(ref list) =
                                    r;
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
                                rows.extend(chat_invite_importers_rows(&guard.client, &r));
                                if page_len == 0 {
                                    break;
                                }
                            }
                            if !output::machine_mode(json, jsonl) {
                                print_importer_table(&name, multi, &rows)?;
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
                                guard.rate_limiter.acquire().await;
                                let r: tl::enums::messages::ExportedChatInvites = guard
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
                                    if let Some(
                                        tl::enums::ExportedChatInvite::ChatInviteExported(inv),
                                    ) = list.invites.last()
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
                            if !output::machine_mode(json, jsonl) {
                                print_invite_link_table(&name, multi, &rows)?;
                            }
                            Ok(serde_json::json!({"links": rows}))
                        }
                    }
                }
                InviteMode::Check => {
                    let hash = plan.hash.clone().unwrap_or_default();
                    let r: tl::enums::ChatInvite = guard
                        .client
                        .invoke(&tl::functions::messages::CheckChatInvite { hash })
                        .await
                        .map_err(tele_invocation)?;
                    let row = check_invite_row(&r);
                    if !output::machine_mode(json, jsonl) {
                        print_check_table(&name, multi, &row)?;
                    }
                    Ok(serde_json::json!({"invite": row}))
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

pub(crate) fn invite_dry_run_payload(chat: &str, plan: &ValidatedInvite) -> serde_json::Value {
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
                "would": format!("export invite link of chat {chat}")});
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
                "would": would})
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
                "would": format!("{action} invite link {link} in chat {chat}")});
            invite_echo_options(&mut v, plan);
            v
        }
        InviteMode::DeleteRevoked => {
            serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "delete_revoked",
                "would": format!("delete revoked invite links exported from chat {chat}")})
        }
        InviteMode::Check => {
            let link = plan.link.clone().unwrap_or_default();
            serde_json::json!({
                "dry_run": true,
                "chat": chat,
                "mode": "check",
                "link": link,
                "would": format!("preview invite link {link}")})
        }
    }
}

pub(crate) fn invite_echo_options(v: &mut serde_json::Value, plan: &ValidatedInvite) {
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

pub(crate) fn has_any_option(args: &InviteArgs) -> bool {
    args.title.is_some()
        || args.expire.is_some()
        || args.usage_limit.is_some()
        || args.request_approval.is_some()
}

pub(crate) fn validate_invite(args: &InviteArgs) -> TeleResult<ValidatedInvite> {
    if args.check.is_none() {
        ChatTarget::parse_flag(args.chat.as_deref().unwrap_or_default(), "chat")?;
    }
    if args.check.is_some() && args.chat.as_deref().is_some_and(|c| !c.trim().is_empty()) {
        return Err(TeleError::Usage(
            "--check previews a standalone link; --chat is not needed".to_string(),
        ));
    }
    let requested_modes = [
        args.user.is_some(),
        args.list,
        args.edit.is_some(),
        args.delete_revoked,
        args.check.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if requested_modes > 1 {
        return Err(TeleError::Usage(
            "--user, --list, --edit, --check, and --delete-revoked are mutually exclusive"
                .to_string(),
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
    if let Some(link) = &args.check {
        if has_any_option(args) {
            return Err(TeleError::Usage(
                "--title/--expire/--usage-limit/--request-approval configure link export/edit, not --check".to_string(),
            ));
        }
        let normalized = normalized_validated_link(link, "--check")?;
        plan.hash = Some(invite_hash_from_link(&normalized).ok_or_else(|| {
            TeleError::Usage(format!(
                "--check: not a valid invite link or chat target: \"{link}\""
            ))
        })?);
        plan.mode = InviteMode::Check;
        plan.link = Some(normalized);
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
        let limit_i32 = i32::try_from(limit).map_err(|_| {
            TeleError::Usage("--usage-limit exceeds the maximum value of 2147483647".to_string())
        })?;
        plan.usage_limit = Some(limit_i32);
    }
    if let Some(flag) = &args.request_approval {
        plan.request_needed = Some(parse_invite_bool(flag)?);
    }
    Ok(plan)
}

pub(crate) fn normalized_validated_link(input: &str, flag: &str) -> TeleResult<String> {
    let normalized = normalize_invite_link(input);
    validate_invite_link(&normalized)
        .map_err(|e| TeleError::Usage(format!("{flag}: {}", e.message())))?;
    Ok(normalized)
}

pub(crate) fn invite_hash_from_link(link: &str) -> Option<String> {
    if let Some(hash) = grammers_client::Client::parse_invite_link(link) {
        return Some(hash);
    }
    if is_bare_invite_hash(link) {
        return Some(link.strip_prefix('+').unwrap_or(link).to_string());
    }
    None
}

pub(crate) fn parse_invite_bool(value: &str) -> TeleResult<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(TeleError::Usage(format!(
            "--request-approval must be true or false, got \"{other}\""
        ))),
    }
}

pub(crate) fn invite_duration_seconds(value: &str) -> Option<i64> {
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

pub(crate) fn parse_invite_expire_at(now_ts: i64, value: &str) -> TeleResult<i32> {
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

pub(crate) fn parse_invite_expire(value: &str) -> TeleResult<i32> {
    parse_invite_expire_at(chrono::Utc::now().timestamp(), value)
}

pub(crate) fn exported_invite_row(invite: &tl::enums::ExportedChatInvite) -> serde_json::Value {
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
                "date": rfc3339_or_empty(Some(i.date))})
        }
        tl::enums::ExportedChatInvite::ChatInvitePublicJoinRequests => {
            serde_json::json!({"public_join_requests": true})
        }
    }
}

pub(crate) fn exported_invite_result_rows(
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

pub(crate) fn exported_chat_invites_rows(
    result: &tl::enums::messages::ExportedChatInvites,
) -> Vec<serde_json::Value> {
    match result {
        tl::enums::messages::ExportedChatInvites::Invites(list) => {
            list.invites.iter().map(exported_invite_row).collect()
        }
    }
}

pub(crate) fn chat_invite_importers_rows(
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
            let mut row = match users.get(&imp.user_id) {
                Some(user) => crate::serialize::peer_key(&grammers_client::peer::Peer::User(
                    grammers_client::peer::User::from_raw(client, user.clone()),
                )),
                None => serde_json::json!({
                    "id": imp.user_id,
                    "name": user_display_name(client, &users, imp.user_id)}),
            };
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "date".into(),
                    serde_json::json!(rfc3339_or_empty(Some(imp.date))),
                );
                obj.insert("requested".into(), serde_json::json!(imp.requested));
                obj.insert("approved_by".into(), serde_json::json!(imp.approved_by));
            }
            row
        })
        .collect()
}

pub(crate) fn chat_row(chat: &tl::enums::Chat) -> serde_json::Value {
    match chat {
        tl::enums::Chat::Empty(e) => {
            serde_json::json!({"id": e.id, "title": null, "chat_kind": "unknown"})
        }
        tl::enums::Chat::Chat(c) => serde_json::json!({
            "id": c.id,
            "title": c.title,
            "chat_kind": "group",
            "participants_count": c.participants_count}),
        tl::enums::Chat::Forbidden(f) => serde_json::json!({
            "id": f.id, "title": f.title, "chat_kind": "forbidden"}),
        tl::enums::Chat::Channel(c) => serde_json::json!({
            "id": c.id,
            "title": c.title,
            "chat_kind": if c.broadcast { "channel" } else { "supergroup" },
            "participants_count": c.participants_count}),
        tl::enums::Chat::ChannelForbidden(f) => serde_json::json!({
            "id": f.id,
            "title": f.title,
            "chat_kind": if f.broadcast { "channel" } else { "supergroup" }}),
    }
}

pub(crate) fn check_invite_row(invite: &tl::enums::ChatInvite) -> serde_json::Value {
    match invite {
        tl::enums::ChatInvite::Already(w) => {
            let mut v = chat_row(&w.chat);
            v["kind"] = serde_json::json!("already");
            v["request_needed"] = serde_json::Value::Null;
            v["expires"] = serde_json::Value::Null;
            v
        }
        tl::enums::ChatInvite::Peek(w) => {
            let mut v = chat_row(&w.chat);
            v["kind"] = serde_json::json!("peek");
            v["request_needed"] = serde_json::Value::Null;
            v["expires"] = serde_json::json!(w.expires);
            v
        }
        tl::enums::ChatInvite::Invite(w) => serde_json::json!({
            "kind": "invite",
            "id": null,
            "title": w.title,
            "chat_kind": if w.channel { if w.broadcast { "channel" } else { "supergroup" } } else { "group" },
            "participants_count": w.participants_count,
            "about": w.about,
            "request_needed": w.request_needed,
            "broadcast": w.broadcast,
            "megagroup": w.megagroup,
            "public": w.public,
            "verified": w.verified,
            "scam": w.scam,
            "fake": w.fake,
            "participants_preview": w.participants.as_ref().map(|u| u.len()),
            "expires": null}),
    }
}

pub(crate) fn print_check_table(
    account: &str,
    multi: bool,
    row: &serde_json::Value,
) -> TeleResult<()> {
    let approval = match row["request_needed"] {
        serde_json::Value::Bool(true) => "required".to_string(),
        serde_json::Value::Bool(false) => "open".to_string(),
        _ => String::new(),
    };
    output::print_account_table(
        account,
        multi,
        &["kind", "id", "title", "members", "approval", "expires"],
        &[vec![
            row["kind"].as_str().unwrap_or_default().to_string(),
            row.get("id")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default(),
            row["title"].as_str().unwrap_or_default().to_string(),
            row.get("participants_count")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default(),
            approval,
            row.get("expires")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ]],
    )
}

pub(crate) fn print_invite_link_table(
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

pub(crate) fn truncate_cell(expire_date: Option<i64>) -> String {
    match expire_date {
        Some(0) | None => String::new(),
        Some(ts) => chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| ts.to_string()),
    }
}

pub(crate) fn print_importer_table(
    account: &str,
    multi: bool,
    rows: &[serde_json::Value],
) -> TeleResult<()> {
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
