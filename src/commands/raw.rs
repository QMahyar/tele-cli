use clap::Args;
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Args)]
pub struct RawArgs {
    #[arg(help = "TL method name (e.g. contacts.Search, messages.GetAllDrafts)")]
    name: String,
    #[arg(long, default_value = "{}", help = "JSON object of method parameters")]
    args: String,
}

pub const REGISTERED: &[&str] = &[
    "account.UpdateProfile",
    "contacts.Search",
    "messages.ExportChatInvite",
    "messages.GetAllDrafts",
    "stats.GetBroadcastStats",
    "stats.GetMegagroupStats",
];

pub async fn run(args: &RawArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let params: serde_json::Value = serde_json::from_str(&args.args)
        .map_err(|e| TeleError::Usage(format!("invalid --args JSON: {e}")))?;
    let name = args.name.clone();
    if !REGISTERED.contains(&name.as_str()) {
        return Err(TeleError::Usage(format!(
            "raw method not in registry; add an arm in src/commands/raw.rs (registered: {REGISTERED:?})"
        )));
    }
    validate_params(&name, &params)?;
    if !flags.dry_run && requires_explicit_account(&name) && flags.account.is_empty() {
        return Err(TeleError::Usage(format!(
            "raw method {name} mutates account data — pass --account explicitly"
        )));
    }
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |account| {
        let config_path = config_path.clone();
        let name = name.clone();
        let params = params.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "method": name,
                    "would": format!("invoke raw method {name}"),
                }));
            }
            let guard =
                ClientGuard::connect(&account, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            dispatch(&guard.client, guard.session.as_ref(), &name, &params)
                .await
                .map_err(tele_invocation)
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        let value = serde_json::to_value(&envelope)?;
        match human_display(&value) {
            HumanView::Lines(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            HumanView::Table(headers, rows) => {
                let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
                crate::output::print_table(&header_refs, &rows);
            }
        }
    }
    crate::executor::finish(flags, &envelope)
}

fn requires_explicit_account(method: &str) -> bool {
    matches!(
        method,
        "account.UpdateProfile" | "messages.ExportChatInvite"
    )
}

enum HumanView {
    Lines(Vec<String>),
    Table(Vec<String>, Vec<Vec<String>>),
}

fn human_display(value: &serde_json::Value) -> HumanView {
    if let serde_json::Value::Array(items) = value {
        if !items.is_empty() && items.iter().all(|i| i.is_object()) {
            return table_view(items);
        }
    }
    HumanView::Lines(match value {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}: {}", render_value(v)))
            .collect(),
        other => vec![render_value(other)],
    })
}

fn table_view(items: &[serde_json::Value]) -> HumanView {
    let mut headers: Vec<String> = Vec::new();
    for item in items {
        for key in item.as_object().expect("object").keys() {
            if !headers.iter().any(|h| h == key) {
                headers.push(key.clone());
            }
        }
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            headers
                .iter()
                .map(|h| {
                    item.as_object()
                        .expect("object")
                        .get(h)
                        .map(render_value)
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    HumanView::Table(headers, rows)
}

fn render_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn validate_params(name: &str, p: &serde_json::Value) -> TeleResult<()> {
    fn req_str(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if p.get(key).and_then(|v| v.as_str()).is_none() {
            return Err(TeleError::Usage(format!(
                "--args field {key} is required (string)"
            )));
        }
        Ok(())
    }
    fn opt_str(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            if !v.is_string() {
                return Err(TeleError::Usage(format!(
                    "--args field {key} must be a string"
                )));
            }
        }
        Ok(())
    }
    fn opt_i32(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            let n = v.as_i64().ok_or_else(|| {
                TeleError::Usage(format!("--args field {key} must be an integer"))
            })?;
            i32::try_from(n)
                .map_err(|_| TeleError::Usage(format!("--args field {key} is out of range")))?;
        }
        Ok(())
    }
    fn opt_bool(p: &serde_json::Value, key: &str) -> TeleResult<()> {
        if let Some(v) = p.get(key) {
            if !v.is_boolean() {
                return Err(TeleError::Usage(format!(
                    "--args field {key} must be a boolean"
                )));
            }
        }
        Ok(())
    }
    if !p.is_object() {
        return Err(TeleError::Usage(
            "--args must be a JSON object of constructor kwargs".to_string(),
        ));
    }
    match name {
        "contacts.Search" => {
            let valid = ["q", "limit", "broadcasts", "bots"];
            reject_unknown_keys(name, p, &valid)?;
            req_str(p, "q")?;
            opt_i32(p, "limit")?;
            opt_bool(p, "broadcasts")?;
            opt_bool(p, "bots")?;
        }
        "messages.ExportChatInvite" => {
            let valid = [
                "chat",
                "request_needed",
                "expire_date",
                "usage_limit",
                "title",
            ];
            reject_unknown_keys(name, p, &valid)?;
            req_str(p, "chat")?;
            opt_bool(p, "request_needed")?;
            opt_i32(p, "expire_date")?;
            opt_i32(p, "usage_limit")?;
            opt_str(p, "title")?;
        }
        "stats.GetBroadcastStats" | "stats.GetMegagroupStats" => {
            let valid = ["channel", "dark"];
            reject_unknown_keys(name, p, &valid)?;
            req_str(p, "channel")?;
            opt_bool(p, "dark")?;
        }
        "account.UpdateProfile" => {
            let valid = ["first_name", "last_name", "about"];
            reject_unknown_keys(name, p, &valid)?;
            opt_str(p, "first_name")?;
            opt_str(p, "last_name")?;
            opt_str(p, "about")?;
        }
        "messages.GetAllDrafts" => {
            reject_unknown_keys(name, p, &[])?;
        }
        _ => {}
    }
    Ok(())
}

fn reject_unknown_keys(name: &str, p: &serde_json::Value, valid: &[&str]) -> TeleResult<()> {
    let Some(obj) = p.as_object() else {
        return Ok(());
    };
    let mut unknown: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !valid.contains(k))
        .collect();
    unknown.sort_unstable();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(TeleError::Usage(format!(
        "unknown --args key(s) {unknown:?} for {name} (valid keys: {valid:?})"
    )))
}

async fn dispatch(
    client: &grammers_client::Client,
    session: &grammers_session::storages::SqliteSession,
    name: &str,
    p: &serde_json::Value,
) -> Result<serde_json::Value, grammers_client::InvocationError> {
    match name {
        "messages.ExportChatInvite" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "chat")?).await?;
            let peer = crate::entities::input_peer(&chat).await?;
            let r: tl::enums::ExportedChatInvite = client
                .invoke(&tl::functions::messages::ExportChatInvite {
                    legacy_revoke_permanent: false,
                    request_needed: bool_field(p, "request_needed")?,
                    peer,
                    expire_date: opt_int_field(p, "expire_date")?,
                    usage_limit: opt_int_field(p, "usage_limit")?,
                    title: opt_str_field(p, "title")?,
                    subscription_pricing: None,
                })
                .await?;
            match r {
                tl::enums::ExportedChatInvite::ChatInviteExported(invite) => {
                    Ok(serde_json::json!({
                        "link": invite.link,
                        "usage_limit": invite.usage_limit,
                        "expire_date": invite.expire_date,
                    }))
                }
                _ => Ok(serde_json::json!({"result": "public_join_requests"})),
            }
        }
        "contacts.Search" => {
            let r: tl::enums::contacts::Found = client
                .invoke(&tl::functions::contacts::Search {
                    broadcasts: bool_field(p, "broadcasts")?,
                    bots: bool_field(p, "bots")?,
                    q: str_field(p, "q")?,
                    limit: int_field(p, "limit")?,
                })
                .await?;
            let tl::enums::contacts::Found::Found(found) = r;
            let my_results = found.my_results.iter().map(peer_id).collect::<Vec<_>>();
            let results = found.results.iter().map(peer_id).collect::<Vec<_>>();
            let users = found
                .users
                .iter()
                .map(|u| match u {
                    tl::enums::User::User(u) => serde_json::json!({
                        "id": u.id,
                        "first_name": u.first_name,
                        "last_name": u.last_name,
                        "username": u.username,
                    }),
                    _ => serde_json::json!({"id": 0}),
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "my_results": my_results,
                "results": results,
                "users": users,
            }))
        }
        "messages.GetAllDrafts" => {
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::GetAllDrafts {})
                .await?;
            let (updates, users, chats) = match r {
                tl::enums::Updates::Updates(u) => (u.updates, u.users.len(), u.chats.len()),
                other => {
                    return Ok(serde_json::json!({
                        "updates": [],
                        "users": 0,
                        "chats": 0,
                        "kind": format!("{other:?}"),
                    }));
                }
            };
            let rows = updates
                .iter()
                .map(update_summary)
                .collect::<Vec<serde_json::Value>>();
            Ok(serde_json::json!({
                "updates": rows,
                "users": users,
                "chats": chats,
            }))
        }
        "stats.GetBroadcastStats" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat).await?;
            let r: tl::enums::stats::BroadcastStats = client
                .invoke(&tl::functions::stats::GetBroadcastStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await?;
            let tl::enums::stats::BroadcastStats::Stats(r) = r;
            Ok(serde_json::json!({
                "period": stats_period(&r.period),
                "followers": stats_abs(&r.followers),
                "views_per_post": stats_abs(&r.views_per_post),
                "shares_per_post": stats_abs(&r.shares_per_post),
                "reactions_per_post": stats_abs(&r.reactions_per_post),
                "enabled_notifications": stats_percent(&r.enabled_notifications),
                "recent_posts_interactions": r.recent_posts_interactions.len(),
            }))
        }
        "stats.GetMegagroupStats" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat).await?;
            let r: tl::enums::stats::MegagroupStats = client
                .invoke(&tl::functions::stats::GetMegagroupStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await?;
            let tl::enums::stats::MegagroupStats::Stats(r) = r;
            Ok(serde_json::json!({
                "period": stats_period(&r.period),
                "members": stats_abs(&r.members),
                "messages": stats_abs(&r.messages),
                "viewers": stats_abs(&r.viewers),
                "posters": stats_abs(&r.posters),
                "top_posters": r.top_posters.len(),
                "top_admins": r.top_admins.len(),
                "top_inviters": r.top_inviters.len(),
            }))
        }
        "account.UpdateProfile" => {
            let r: tl::enums::User = client
                .invoke(&tl::functions::account::UpdateProfile {
                    first_name: opt_str_field(p, "first_name")?,
                    last_name: opt_str_field(p, "last_name")?,
                    about: opt_str_field(p, "about")?,
                })
                .await?;
            let (id, first_name, last_name, username) = match r {
                tl::enums::User::User(u) => (u.id, u.first_name, u.last_name, u.username),
                _ => (0, None, None, None),
            };
            Ok(serde_json::json!({
                "id": id,
                "first_name": first_name,
                "last_name": last_name,
                "username": username,
            }))
        }
        _ => Err(grammers_client::InvocationError::Rpc(
            grammers_client::sender::RpcError {
                code: 400,
                name: "RAW_NOT_REGISTERED".to_string(),
                value: None,
                caused_by: None,
            },
        )),
    }
}

fn str_field(p: &serde_json::Value, key: &str) -> Result<String, grammers_client::InvocationError> {
    Ok(p.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(p) => p.user_id,
        tl::enums::Peer::Chat(p) => p.chat_id,
        tl::enums::Peer::Channel(p) => p.channel_id,
    }
}

fn update_summary(u: &tl::enums::Update) -> serde_json::Value {
    match u {
        tl::enums::Update::NewMessage(m) => match &m.message {
            tl::enums::Message::Message(msg) => serde_json::json!({
                "type": "new_message",
                "id": msg.id,
                "peer_id": peer_id(&msg.peer_id),
                "out": msg.out,
                "text": msg.message,
            }),
            _ => serde_json::json!({"type": "new_message"}),
        },
        tl::enums::Update::EditMessage(m) => match &m.message {
            tl::enums::Message::Message(msg) => serde_json::json!({
                "type": "edit_message",
                "id": msg.id,
                "peer_id": peer_id(&msg.peer_id),
                "text": msg.message,
            }),
            _ => serde_json::json!({"type": "edit_message"}),
        },
        tl::enums::Update::DraftMessage(d) => {
            let text = match &d.draft {
                tl::enums::DraftMessage::Message(draft) => draft.message.clone(),
                tl::enums::DraftMessage::Empty(_) => String::new(),
            };
            serde_json::json!({"type": "draft_message", "peer_id": peer_id(&d.peer), "text": text})
        }
        _ => serde_json::json!({"type": "other"}),
    }
}

fn stats_period(v: &tl::enums::StatsDateRangeDays) -> serde_json::Value {
    match v {
        tl::enums::StatsDateRangeDays::Days(d) => {
            serde_json::json!({"min_date": d.min_date, "max_date": d.max_date})
        }
    }
}

fn stats_abs(v: &tl::enums::StatsAbsValueAndPrev) -> serde_json::Value {
    match v {
        tl::enums::StatsAbsValueAndPrev::Prev(p) => {
            serde_json::json!({"current": p.current, "previous": p.previous})
        }
    }
}

fn stats_percent(v: &tl::enums::StatsPercentValue) -> serde_json::Value {
    match v {
        tl::enums::StatsPercentValue::Value(p) => {
            serde_json::json!({"part": p.part, "total": p.total})
        }
    }
}

fn opt_str_field(
    p: &serde_json::Value,
    key: &str,
) -> Result<Option<String>, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn int_field(p: &serde_json::Value, key: &str) -> Result<i32, grammers_client::InvocationError> {
    Ok(p.get(key)
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(10))
}

fn opt_int_field(
    p: &serde_json::Value,
    key: &str,
) -> Result<Option<i32>, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_i64()).map(|v| v as i32))
}

fn bool_field(p: &serde_json::Value, key: &str) -> Result<bool, grammers_client::InvocationError> {
    Ok(p.get(key).and_then(|v| v.as_bool()).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_missing_required_field_fails() {
        assert!(matches!(
            validate_params("contacts.Search", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("messages.ExportChatInvite", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("stats.GetBroadcastStats", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_wrong_type_fails() {
        assert!(matches!(
            validate_params("contacts.Search", &serde_json::json!({"q": 42})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "contacts.Search",
                &serde_json::json!({"q": "a", "limit": "5"})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_out_of_range_int_fails() {
        assert!(matches!(
            validate_params(
                "contacts.Search",
                &serde_json::json!({"q": "a", "limit": 9_999_999_999i64})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_non_object_fails() {
        assert!(matches!(
            validate_params("messages.GetAllDrafts", &serde_json::json!([])),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_valid_pass() {
        assert!(validate_params(
            "contacts.Search",
            &serde_json::json!({"q": "alice", "limit": 5, "broadcasts": true})
        )
        .is_ok());
        assert!(validate_params(
            "stats.GetMegagroupStats",
            &serde_json::json!({"channel": "@x"})
        )
        .is_ok());
        assert!(validate_params("messages.GetAllDrafts", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn params_unknown_key_fails() {
        let err = validate_params(
            "contacts.Search",
            &serde_json::json!({"q": "a", "invite_policy": true}),
        )
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("invite_policy"));
        assert!(err.message().contains("valid"));
        assert!(matches!(
            validate_params("messages.GetAllDrafts", &serde_json::json!({"nope": 1})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("account.UpdateProfile", &serde_json::json!({"bio": "dev"})),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn params_all_known_keys_pass() {
        assert!(validate_params(
            "contacts.Search",
            &serde_json::json!({"q": "a", "limit": 5, "broadcasts": true, "bots": false})
        )
        .is_ok());
        assert!(validate_params(
            "messages.ExportChatInvite",
            &serde_json::json!({
                "chat": "@x",
                "request_needed": true,
                "expire_date": 100,
                "usage_limit": 10,
                "title": "t"
            })
        )
        .is_ok());
        assert!(validate_params(
            "stats.GetBroadcastStats",
            &serde_json::json!({"channel": "@x", "dark": true})
        )
        .is_ok());
        assert!(validate_params(
            "account.UpdateProfile",
            &serde_json::json!({"first_name": "a", "last_name": "b", "about": "c"})
        )
        .is_ok());
    }

    #[test]
    fn human_display_object_renders_key_value_lines() {
        let value = serde_json::json!({"name": "alice", "id": 7, "tags": ["a", "b"]});
        match human_display(&value) {
            HumanView::Lines(lines) => {
                assert_eq!(lines.len(), 3);
                assert!(lines.iter().any(|l| l == "name: alice"));
                assert!(lines.iter().any(|l| l == "id: 7"));
                assert!(lines.iter().any(|l| l == "tags: [\"a\",\"b\"]"));
            }
            _ => panic!("object should render as lines"),
        }
    }

    #[test]
    fn human_display_array_of_objects_renders_table() {
        let value = serde_json::json!([
            {"name": "alice", "id": 7},
            {"name": "bob", "extra": true}
        ]);
        match human_display(&value) {
            HumanView::Table(headers, rows) => {
                assert_eq!(headers, vec!["id", "name", "extra"]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec!["7", "alice", ""]);
                assert_eq!(rows[1], vec!["", "bob", "true"]);
            }
            _ => panic!("array of objects should render as a table"),
        }
    }

    #[test]
    fn human_display_empty_value_renders_lines() {
        match human_display(&serde_json::Value::Null) {
            HumanView::Lines(lines) => assert_eq!(lines, vec!["null"]),
            _ => panic!("null should render as a line"),
        }
        match human_display(&serde_json::json!([])) {
            HumanView::Lines(lines) => assert_eq!(lines, vec!["[]"]),
            _ => panic!("empty array should render as a line"),
        }
    }

    #[test]
    fn mutating_methods_require_explicit_account() {
        assert!(requires_explicit_account("account.UpdateProfile"));
        assert!(requires_explicit_account("messages.ExportChatInvite"));
        assert!(!requires_explicit_account("messages.GetAllDrafts"));
        assert!(!requires_explicit_account("contacts.Search"));
    }

    #[test]
    fn registered_mutators_are_gated() {
        for name in REGISTERED {
            let mutating = matches!(*name, "account.UpdateProfile" | "messages.ExportChatInvite");
            assert_eq!(requires_explicit_account(name), mutating, "{name}");
        }
    }
}
