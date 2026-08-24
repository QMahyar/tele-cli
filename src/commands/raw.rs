use clap::Args;
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::helpers::{peer_id, stats_abs, stats_percent, stats_period};
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated_raw.rs"));
}

pub use generated::registry;

#[derive(Args)]
pub struct RawArgs {
    #[arg(help = "TL method name (e.g. contacts.Search, messages.GetAllDrafts)")]
    name: String,
    #[arg(long, default_value = "{}", help = "JSON object of method parameters")]
    args: String,
}

pub const REGISTERED: &[&str] = &[
    "account.GetAuthorizations",
    "account.SetAuthorizationTTL",
    "account.UpdateProfile",
    "channels.GetFullChannel",
    "contacts.DeleteByPhones",
    "contacts.Search",
    "messages.AppendTodoList",
    "messages.ComposeMessageWithAI",
    "messages.ExportChatInvite",
    "messages.GetAllDrafts",
    "messages.GetAvailableEffects",
    "messages.GetDialogUnreadMarks",
    "messages.GetHistory",
    "messages.GetMessagesViews",
    "messages.GetScheduledHistory",
    "messages.ReadMentions",
    "messages.ReadReactions",
    "messages.Search",
    "messages.SendScheduledMessages",
    "messages.ToggleTodoCompleted",
    "messages.TranslateText",
    "messages.TranscribeAudio",
    "stats.GetBroadcastStats",
    "stats.GetMegagroupStats",
    "users.GetUsers",
];

pub async fn run(args: &RawArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let params: serde_json::Value = serde_json::from_str(&args.args)
        .map_err(|e| TeleError::Usage(format!("invalid --args JSON: {e}")))?;
    let name = args.name.clone();
    validate_raw(&RawCall {
        method: name.clone(),
        args: params.clone(),
    })?;
    if !flags.dry_run && generated::requires_explicit_account(&name) && flags.account.is_empty() {
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
                return Ok(raw_dry_run_payload(&name, &params));
            }
            let guard =
                ClientGuard::connect(&account, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            dispatch(&guard.client, guard.session.as_ref(), &name, &params).await
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        let value = serde_json::to_value(&envelope)?;
        match human_display(&value) {
            HumanView::Lines(lines) => {
                for line in lines {
                    crate::output::print_line(&line)?;
                }
            }
            HumanView::Table(headers, rows) => {
                let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
                crate::output::print_table(&header_refs, &rows)?;
            }
        }
    }
    crate::executor::finish(flags, &envelope)
}

fn raw_dry_run_payload(name: &str, params: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "method": name,
        "args": params,
        "would": format!("invoke raw method {name}"),
    })
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawParams {
    method: String,
    #[serde(default = "default_raw_args")]
    args: serde_json::Value,
    #[serde(default)]
    dry_run: bool,
}

fn default_raw_args() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCall {
    pub(crate) method: String,
    #[serde(default = "default_raw_args")]
    pub(crate) args: serde_json::Value,
}

impl From<&RawParams> for RawCall {
    fn from(p: &RawParams) -> Self {
        Self {
            method: p.method.clone(),
            args: p.args.clone(),
        }
    }
}

impl From<&RawCall> for RawParams {
    fn from(c: &RawCall) -> Self {
        Self {
            method: c.method.clone(),
            args: c.args.clone(),
            dry_run: false,
        }
    }
}

pub(crate) fn validate_raw(call: &RawCall) -> TeleResult<()> {
    if registry::lookup(&call.method).is_none() {
        return Err(TeleError::Usage(format!(
            "raw method not in registry: {}; add an arm in src/commands/raw.rs (registered: {REGISTERED:?})",
            call.method
        )));
    }
    generated::validate_params(&call.method, &call.args)
}

pub(crate) fn raw_serve_dry_run(args: &RawCall) -> TeleResult<serde_json::Value> {
    Ok(raw_dry_run_payload(&args.method, &args.args))
}

pub(crate) async fn raw_core(
    shares: &crate::client::ServeShares,
    params: RawParams,
) -> TeleResult<serde_json::Value> {
    let call = RawCall::from(&params);
    validate_raw(&call)?;
    shares.rate_limiter.acquire().await;
    dispatch(
        &shares.client,
        shares.session.as_ref(),
        &call.method,
        &call.args,
    )
    .await
}

pub(crate) fn raw_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::Lane;
    vec![crate::serve_route!(
        "raw",
        Lane::Mutate,
        Some(std::time::Duration::from_secs(120)),
        false,
        false,
        false,
        "invoke one raw TL method by name",
        RawParams,
        RawCall,
        validate_raw,
        raw_serve_dry_run,
        run_invoke
    )]
}

crate::serve_runner!(run_invoke, raw_core, RawParams);

#[cfg(test)]
fn requires_explicit_account(method: &str) -> bool {
    generated::requires_explicit_account(method)
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

#[cfg(test)]
fn validate_params(name: &str, p: &serde_json::Value) -> TeleResult<()> {
    generated::validate_params(name, p)
}

async fn dispatch(
    client: &grammers_client::Client,
    session: &grammers_session::storages::SqliteSession,
    name: &str,
    p: &serde_json::Value,
) -> TeleResult<serde_json::Value> {
    match name {
        "messages.ExportChatInvite" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "chat")?).await?;
            let peer = crate::entities::input_peer(&chat)
                .await
                .map_err(tele_invocation)?;
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
                .await
                .map_err(tele_invocation)?;
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
                .await
                .map_err(tele_invocation)?;
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
                .await
                .map_err(tele_invocation)?;
            Ok(updates_summary(&r))
        }
        "stats.GetBroadcastStats" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let r: tl::enums::stats::BroadcastStats = client
                .invoke(&tl::functions::stats::GetBroadcastStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await
                .map_err(tele_invocation)?;
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
            let channel = crate::entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let r: tl::enums::stats::MegagroupStats = client
                .invoke(&tl::functions::stats::GetMegagroupStats {
                    channel,
                    dark: bool_field(p, "dark")?,
                })
                .await
                .map_err(tele_invocation)?;
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
                .await
                .map_err(tele_invocation)?;
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
        "channels.GetFullChannel" => {
            let chat =
                crate::entities::resolve_peer(client, session, &str_field(p, "channel")?).await?;
            let channel = crate::entities::input_channel(&chat)
                .await
                .map_err(tele_invocation)?;
            let r: tl::enums::messages::ChatFull = client
                .invoke(&tl::functions::channels::GetFullChannel { channel })
                .await
                .map_err(tele_invocation)?;
            Ok(full_channel_summary(&r))
        }
        "users.GetUsers" => {
            let ids = peer_targets(p, "id")?;
            let mut input_users: Vec<tl::enums::InputUser> = Vec::with_capacity(ids.len());
            for target in &ids {
                let chat = crate::entities::resolve_peer(client, session, target).await?;
                input_users.push(
                    crate::entities::input_user(&chat)
                        .await
                        .map_err(tele_invocation)?,
                );
            }
            let users: Vec<tl::enums::User> = client
                .invoke(&tl::functions::users::GetUsers { id: input_users })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "users": users.iter().map(user_summary).collect::<Vec<_>>(),
            }))
        }
        "messages.GetHistory" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let r: tl::enums::messages::Messages = client
                .invoke(&tl::functions::messages::GetHistory {
                    peer,
                    offset_id: opt_int_field(p, "offset_id")?.unwrap_or(0),
                    offset_date: opt_int_field(p, "offset_date")?.unwrap_or(0),
                    add_offset: opt_int_field(p, "add_offset")?.unwrap_or(0),
                    limit: opt_int_field(p, "limit")?.unwrap_or(10),
                    max_id: opt_int_field(p, "max_id")?.unwrap_or(0),
                    min_id: opt_int_field(p, "min_id")?.unwrap_or(0),
                    hash: long_field(p, "hash"),
                })
                .await
                .map_err(tele_invocation)?;
            Ok(messages_messages_summary(&r))
        }
        "messages.Search" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let from_id = match opt_str_field(p, "from_id")? {
                Some(target) => {
                    let chat = crate::entities::resolve_peer(client, session, &target).await?;
                    Some(
                        crate::entities::input_peer(&chat)
                            .await
                            .map_err(tele_invocation)?,
                    )
                }
                None => None,
            };
            let filter = search_filter(&str_field(p, "filter")?)?;
            let r: tl::enums::messages::Messages = client
                .invoke(&tl::functions::messages::Search {
                    peer,
                    q: str_field(p, "q")?,
                    from_id,
                    saved_peer_id: None,
                    saved_reaction: None,
                    top_msg_id: opt_int_field(p, "top_msg_id")?,
                    filter,
                    min_date: opt_int_field(p, "min_date")?.unwrap_or(0),
                    max_date: opt_int_field(p, "max_date")?.unwrap_or(0),
                    offset_id: opt_int_field(p, "offset_id")?.unwrap_or(0),
                    add_offset: opt_int_field(p, "add_offset")?.unwrap_or(0),
                    limit: opt_int_field(p, "limit")?.unwrap_or(10),
                    max_id: opt_int_field(p, "max_id")?.unwrap_or(0),
                    min_id: opt_int_field(p, "min_id")?.unwrap_or(0),
                    hash: long_field(p, "hash"),
                })
                .await
                .map_err(tele_invocation)?;
            Ok(messages_messages_summary(&r))
        }
        "messages.GetScheduledHistory" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let r: tl::enums::messages::Messages = client
                .invoke(&tl::functions::messages::GetScheduledHistory {
                    peer,
                    hash: long_field(p, "hash"),
                })
                .await
                .map_err(tele_invocation)?;
            Ok(messages_messages_summary(&r))
        }
        "messages.GetMessagesViews" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let ids = int_list_field(p, "id")?;
            if ids.is_empty() {
                return Err(TeleError::Usage(
                    "--args field \"id\" must be a non-empty array of message ids".to_string(),
                ));
            }
            let increment = bool_field(p, "increment")?;
            let r: tl::enums::messages::MessageViews = client
                .invoke(&tl::functions::messages::GetMessagesViews {
                    peer,
                    id: ids,
                    increment,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(match r {
                tl::enums::messages::MessageViews::Views(v) => message_views_summary(&v.views),
            })
        }
        "messages.ReadReactions" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let saved_peer_id = match opt_str_field(p, "saved_peer_id")? {
                Some(target) => {
                    let chat = crate::entities::resolve_peer(client, session, &target).await?;
                    Some(
                        crate::entities::input_peer(&chat)
                            .await
                            .map_err(tele_invocation)?,
                    )
                }
                None => None,
            };
            let r: tl::enums::messages::AffectedHistory = client
                .invoke(&tl::functions::messages::ReadReactions {
                    peer,
                    top_msg_id: opt_int_field(p, "top_msg_id")?,
                    saved_peer_id,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(affected_history_summary(&r))
        }
        "messages.ReadMentions" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let r: tl::enums::messages::AffectedHistory = client
                .invoke(&tl::functions::messages::ReadMentions {
                    peer,
                    top_msg_id: opt_int_field(p, "top_msg_id")?,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(affected_history_summary(&r))
        }
        "messages.GetDialogUnreadMarks" => {
            let parent_peer = match opt_str_field(p, "parent_peer")? {
                Some(target) => {
                    let chat = crate::entities::resolve_peer(client, session, &target).await?;
                    Some(
                        crate::entities::input_peer(&chat)
                            .await
                            .map_err(tele_invocation)?,
                    )
                }
                None => None,
            };
            let marks: Vec<tl::enums::DialogPeer> = client
                .invoke(&tl::functions::messages::GetDialogUnreadMarks { parent_peer })
                .await
                .map_err(tele_invocation)?;
            Ok(dialog_unread_marks_summary(&marks))
        }
        "messages.AppendTodoList" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let msg_id = req_int_field(p, "msg_id")?;
            let list = todo_items_field(p, "list")?;
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::AppendTodoList { peer, msg_id, list })
                .await
                .map_err(tele_invocation)?;
            Ok(updates_summary(&r))
        }
        "messages.ComposeMessageWithAI" => {
            let text = tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: str_field(p, "text")?,
                entities: Vec::new(),
            });
            let tone = compose_tone_field(p)?;
            let r: tl::enums::messages::ComposedMessageWithAi = client
                .invoke(&tl::functions::messages::ComposeMessageWithAi {
                    proofread: bool_field(p, "proofread")?,
                    emojify: bool_field(p, "emojify")?,
                    text,
                    translate_to_lang: opt_str_field(p, "translate_to_lang")?,
                    tone,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(match r {
                tl::enums::messages::ComposedMessageWithAi::Ai(c) => composed_message_summary(&c),
            })
        }
        "messages.GetAvailableEffects" => {
            let r: tl::enums::messages::AvailableEffects = client
                .invoke(&tl::functions::messages::GetAvailableEffects {
                    hash: opt_int_field(p, "hash")?.unwrap_or(0),
                })
                .await
                .map_err(tele_invocation)?;
            Ok(available_effects_summary(&r))
        }
        "messages.SendScheduledMessages" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let ids = int_list_field(p, "id")?;
            if ids.is_empty() {
                return Err(TeleError::Usage(
                    "--args field \"id\" must be a non-empty array of message ids".to_string(),
                ));
            }
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::SendScheduledMessages { peer, id: ids })
                .await
                .map_err(tele_invocation)?;
            Ok(updates_summary(&r))
        }
        "messages.ToggleTodoCompleted" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let msg_id = req_int_field(p, "msg_id")?;
            let completed = int_list_field(p, "completed")?;
            let incompleted = int_list_field(p, "incompleted")?;
            let r: tl::enums::Updates = client
                .invoke(&tl::functions::messages::ToggleTodoCompleted {
                    peer,
                    msg_id,
                    completed,
                    incompleted,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(updates_summary(&r))
        }
        "messages.TranslateText" => {
            let peer = match opt_str_field(p, "chat")? {
                Some(target) => {
                    let chat = crate::entities::resolve_peer(client, session, &target).await?;
                    Some(
                        crate::entities::input_peer(&chat)
                            .await
                            .map_err(tele_invocation)?,
                    )
                }
                None => None,
            };
            let id = match p.get("id") {
                Some(_) => Some(int_list_field(p, "id")?),
                None => None,
            };
            let text = match p.get("text") {
                Some(_) => Some(string_list_field(p, "text")?),
                None => None,
            };
            if id.is_none() && text.is_none() {
                return Err(TeleError::Usage(
                    "--args field \"text\" (array of strings) or \"id\" (array of message ids) is required".to_string(),
                ));
            }
            let r: tl::enums::messages::TranslatedText = client
                .invoke(&tl::functions::messages::TranslateText {
                    peer,
                    id,
                    text: text.map(|items| {
                        items
                            .iter()
                            .map(|t| {
                                tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                                    text: t.clone(),
                                    entities: Vec::new(),
                                })
                            })
                            .collect()
                    }),
                    to_lang: str_field(p, "to_lang")?,
                    tone: opt_str_field(p, "tone")?,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(translated_text_summary(&r))
        }
        "messages.TranscribeAudio" => {
            let peer = resolved_peer(client, session, p, "chat").await?;
            let msg_id = req_int_field(p, "msg_id")?;
            let r: tl::enums::messages::TranscribedAudio = client
                .invoke(&tl::functions::messages::TranscribeAudio { peer, msg_id })
                .await
                .map_err(tele_invocation)?;
            Ok(transcribed_audio_summary(&r))
        }
        "account.GetAuthorizations" => {
            let r: tl::enums::account::Authorizations = client
                .invoke(&tl::functions::account::GetAuthorizations {})
                .await
                .map_err(tele_invocation)?;
            match r {
                tl::enums::account::Authorizations::Authorizations(a) => Ok(serde_json::json!({
                    "authorization_ttl_days": a.authorization_ttl_days,
                    "authorizations": a.authorizations.iter().map(authorization_row).collect::<Vec<_>>(),
                })),
            }
        }
        "account.SetAuthorizationTTL" => {
            let days = match p.get("authorization_ttl_days").and_then(|v| v.as_i64()) {
                Some(d) => {
                    i32::try_from(d).map_err(|_| invalid_int("authorization_ttl_days", d))?
                }
                None => {
                    return Err(TeleError::Usage(
                        "--args field \"authorization_ttl_days\" is required (integer days)"
                            .to_string(),
                    ))
                }
            };
            let ok: bool = client
                .invoke(&tl::functions::account::SetAuthorizationTtl {
                    authorization_ttl_days: days,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(bool_result(ok))
        }
        "contacts.DeleteByPhones" => {
            let phones = string_list_field(p, "phones")?;
            if phones.is_empty() {
                return Err(TeleError::Usage(
                    "--args field \"phones\" must be a non-empty array of phone numbers"
                        .to_string(),
                ));
            }
            let ok: bool = client
                .invoke(&tl::functions::contacts::DeleteByPhones { phones })
                .await
                .map_err(tele_invocation)?;
            Ok(bool_result(ok))
        }
        _ => Err(crate::error::invocation_error(
            grammers_client::InvocationError::Rpc(grammers_client::sender::RpcError {
                code: 400,
                name: "RAW_NOT_REGISTERED".to_string(),
                value: None,
                caused_by: None,
            }),
        )),
    }
}

fn str_field(p: &serde_json::Value, key: &str) -> TeleResult<String> {
    Ok(p.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

fn updates_summary(r: &tl::enums::Updates) -> serde_json::Value {
    match r {
        tl::enums::Updates::Updates(u) => serde_json::json!({
            "updates": u.updates.iter().map(update_summary).collect::<Vec<_>>(),
            "users": u.users.len(),
            "chats": u.chats.len(),
        }),
        tl::enums::Updates::Combined(c) => serde_json::json!({
            "updates": c.updates.iter().map(update_summary).collect::<Vec<_>>(),
            "users": c.users.len(),
            "chats": c.chats.len(),
            "kind": "Combined",
        }),
        _ => serde_json::json!({
            "updates": [],
            "users": 0,
            "chats": 0,
            "kind": "other",
        }),
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

fn opt_str_field(p: &serde_json::Value, key: &str) -> TeleResult<Option<String>> {
    Ok(p.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn int_field(p: &serde_json::Value, key: &str) -> TeleResult<i32> {
    p.get(key)
        .and_then(|v| v.as_i64())
        .map(|v| i32::try_from(v).map_err(|_| invalid_int(key, v)))
        .transpose()
        .map(|v| v.unwrap_or(10))
}

fn opt_int_field(p: &serde_json::Value, key: &str) -> TeleResult<Option<i32>> {
    p.get(key)
        .and_then(|v| v.as_i64())
        .map(|v| i32::try_from(v).map_err(|_| invalid_int(key, v)))
        .transpose()
}

fn req_int_field(p: &serde_json::Value, key: &str) -> TeleResult<i32> {
    match p.get(key).and_then(|v| v.as_i64()) {
        Some(v) => i32::try_from(v).map_err(|_| invalid_int(key, v)),
        None => Err(TeleError::Usage(format!(
            "--args field {key:?} is required (integer)"
        ))),
    }
}

fn todo_items_field(p: &serde_json::Value, key: &str) -> TeleResult<Vec<tl::enums::TodoItem>> {
    fn item_error(key: &str, detail: &str) -> TeleError {
        TeleError::Usage(format!("--args field {key:?} item {detail}"))
    }
    match p.get(key) {
        Some(serde_json::Value::Array(items)) if !items.is_empty() => items
            .iter()
            .map(|v| {
                let obj = v
                    .as_object()
                    .ok_or_else(|| item_error(key, "must be an object with \"id\" and \"text\""))?;
                let id = obj
                    .get("id")
                    .and_then(|n| n.as_i64())
                    .ok_or_else(|| item_error(key, "\"id\" must be an integer"))?;
                let id = i32::try_from(id).map_err(|_| invalid_int("list.id", id))?;
                let text = obj
                    .get("text")
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| item_error(key, "\"text\" must be a string"))?;
                Ok(tl::enums::TodoItem::Item(tl::types::TodoItem {
                    id,
                    title: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: text.to_string(),
                        entities: Vec::new(),
                    }),
                }))
            })
            .collect(),
        _ => Err(TeleError::Usage(format!(
            "--args field {key:?} must be a non-empty array of {{\"id\", \"text\"}} objects"
        ))),
    }
}

fn compose_tone_field(p: &serde_json::Value) -> TeleResult<Option<tl::enums::InputAiComposeTone>> {
    match p.get("tone") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            Ok(Some(tl::enums::InputAiComposeTone::Default(
                tl::types::InputAiComposeToneDefault { tone: s.clone() },
            )))
        }
        Some(serde_json::Value::Object(map)) => {
            let slug = map.get("slug").and_then(|s| s.as_str());
            let named = map.get("tone").and_then(|s| s.as_str());
            match (slug, named) {
                (Some(slug), _) => Ok(Some(tl::enums::InputAiComposeTone::Slug(
                    tl::types::InputAiComposeToneSlug {
                        slug: slug.to_string(),
                    },
                ))),
                (None, Some(named)) => Ok(Some(tl::enums::InputAiComposeTone::Default(
                    tl::types::InputAiComposeToneDefault {
                        tone: named.to_string(),
                    },
                ))),
                _ => Err(TeleError::Usage(
                    "--args field \"tone\" must be a string or {\"slug\": ...} or {\"tone\": ...}"
                        .to_string(),
                )),
            }
        }
        Some(_) => Err(TeleError::Usage(
            "--args field \"tone\" must be a string or {\"slug\": ...} or {\"tone\": ...}"
                .to_string(),
        )),
    }
}

fn invalid_int(key: &str, v: i64) -> TeleError {
    TeleError::Usage(format!(
        "invalid --args value for {key:?}: {v} is outside the 32-bit integer range"
    ))
}

fn bool_field(p: &serde_json::Value, key: &str) -> TeleResult<bool> {
    Ok(p.get(key).and_then(|v| v.as_bool()).unwrap_or(false))
}

async fn resolved_peer(
    client: &grammers_client::Client,
    session: &grammers_session::storages::SqliteSession,
    p: &serde_json::Value,
    key: &str,
) -> TeleResult<tl::enums::InputPeer> {
    let chat = crate::entities::resolve_peer(client, session, &str_field(p, key)?).await?;
    crate::entities::input_peer(&chat)
        .await
        .map_err(tele_invocation)
}

fn json_target_string(v: &serde_json::Value) -> TeleResult<String> {
    match v {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err(TeleError::Usage(
            "--args peer targets must be strings or numeric ids".to_string(),
        )),
    }
}

fn peer_targets(p: &serde_json::Value, key: &str) -> TeleResult<Vec<String>> {
    match p.get(key) {
        Some(serde_json::Value::Array(items)) if !items.is_empty() => items
            .iter()
            .map(json_target_string)
            .collect::<TeleResult<Vec<_>>>(),
        _ => Err(TeleError::Usage(format!(
            "--args field {key:?} must be a non-empty array of peer targets"
        ))),
    }
}

fn int_list_field(p: &serde_json::Value, key: &str) -> TeleResult<Vec<i32>> {
    match p.get(key) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| match v.as_i64() {
                Some(n) => i32::try_from(n).map_err(|_| invalid_int(key, n)),
                None => Err(TeleError::Usage(format!(
                    "--args field {key:?} must contain only integers"
                ))),
            })
            .collect(),
        _ => Err(TeleError::Usage(format!(
            "--args field {key:?} must be an array of integers"
        ))),
    }
}

fn string_list_field(p: &serde_json::Value, key: &str) -> TeleResult<Vec<String>> {
    match p.get(key) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| match v.as_str() {
                Some(s) => Ok(s.to_string()),
                None => Err(TeleError::Usage(format!(
                    "--args field {key:?} must contain only strings"
                ))),
            })
            .collect(),
        _ => Err(TeleError::Usage(format!(
            "--args field {key:?} must be an array of strings"
        ))),
    }
}

fn long_field(p: &serde_json::Value, key: &str) -> i64 {
    p.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn search_filter(name: &str) -> TeleResult<tl::enums::MessagesFilter> {
    let valid = [
        "empty",
        "photos",
        "video",
        "gif",
        "documents",
        "urls",
        "audio",
        "voice",
    ];
    let lowered = name.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "" | "empty" => Ok(tl::enums::MessagesFilter::InputMessagesFilterEmpty),
        "photos" => Ok(tl::enums::MessagesFilter::InputMessagesFilterPhotos),
        "video" | "videos" => Ok(tl::enums::MessagesFilter::InputMessagesFilterVideo),
        "gif" => Ok(tl::enums::MessagesFilter::InputMessagesFilterGif),
        "documents" | "docs" => Ok(tl::enums::MessagesFilter::InputMessagesFilterDocument),
        "url" | "urls" => Ok(tl::enums::MessagesFilter::InputMessagesFilterUrl),
        "audio" | "music" => Ok(tl::enums::MessagesFilter::InputMessagesFilterMusic),
        "voice" | "voicenotes" => Ok(tl::enums::MessagesFilter::InputMessagesFilterVoice),
        other => Err(TeleError::Usage(format!(
            "--args field \"filter\": unknown filter {other:?} (valid names: {valid:?})"
        ))),
    }
}

fn messages_messages_summary(r: &tl::enums::messages::Messages) -> serde_json::Value {
    match r {
        tl::enums::messages::Messages::Messages(m) => {
            messages_collection_summary(None, &m.messages, m.chats.len(), m.users.len())
        }
        tl::enums::messages::Messages::Slice(s) => {
            messages_collection_summary(Some(s.count), &s.messages, s.chats.len(), s.users.len())
        }
        tl::enums::messages::Messages::ChannelMessages(c) => {
            messages_collection_summary(Some(c.count), &c.messages, c.chats.len(), c.users.len())
        }
        tl::enums::messages::Messages::NotModified(n) => {
            serde_json::json!({
                "count": n.count,
                "not_modified": true,
            })
        }
    }
}

fn messages_collection_summary(
    count: Option<i32>,
    messages: &[tl::enums::Message],
    chats: usize,
    users: usize,
) -> serde_json::Value {
    serde_json::json!({
        "count": count.unwrap_or(messages.len() as i32),
        "messages": messages.iter().map(message_row).collect::<Vec<_>>(),
        "chats": chats,
        "users": users,
    })
}

fn message_row(m: &tl::enums::Message) -> serde_json::Value {
    match m {
        tl::enums::Message::Message(msg) => serde_json::json!({
            "id": msg.id,
            "date": msg.date,
            "out": msg.out,
            "text": msg.message,
            "peer_id": peer_id(&msg.peer_id),
        }),
        tl::enums::Message::Service(msg) => serde_json::json!({
            "id": msg.id,
            "date": msg.date,
            "service": true,
        }),
        tl::enums::Message::Empty(msg) => serde_json::json!({
            "id": msg.id,
            "empty": true,
        }),
    }
}

fn affected_history_summary(r: &tl::enums::messages::AffectedHistory) -> serde_json::Value {
    match r {
        tl::enums::messages::AffectedHistory::History(h) => serde_json::json!({
            "pts": h.pts,
            "pts_count": h.pts_count,
            "offset": h.offset,
        }),
    }
}

fn available_effects_summary(r: &tl::enums::messages::AvailableEffects) -> serde_json::Value {
    match r {
        tl::enums::messages::AvailableEffects::Effects(e) => serde_json::json!({
            "hash": e.hash,
            "effects": e.effects.iter().map(effect_row).collect::<Vec<_>>(),
            "documents": e.documents.len(),
        }),
        tl::enums::messages::AvailableEffects::NotModified => {
            serde_json::json!({ "not_modified": true })
        }
    }
}

fn effect_row(e: &tl::enums::AvailableEffect) -> serde_json::Value {
    match e {
        tl::enums::AvailableEffect::Effect(a) => serde_json::json!({
            "id": a.id,
            "emoticon": a.emoticon,
            "premium_required": a.premium_required,
            "effect_sticker_id": a.effect_sticker_id,
        }),
    }
}

fn transcribed_audio_summary(r: &tl::enums::messages::TranscribedAudio) -> serde_json::Value {
    match r {
        tl::enums::messages::TranscribedAudio::Audio(a) => serde_json::json!({
            "pending": a.pending,
            "transcription_id": a.transcription_id,
            "text": a.text,
            "trial_remains_num": a.trial_remains_num,
            "trial_remains_until_date": a.trial_remains_until_date,
        }),
    }
}

fn translated_text_summary(r: &tl::enums::messages::TranslatedText) -> serde_json::Value {
    match r {
        tl::enums::messages::TranslatedText::TranslateResult(res) => {
            let translations = res
                .result
                .iter()
                .map(text_with_entities_text)
                .collect::<Vec<_>>();
            serde_json::json!({ "count": translations.len(), "translations": translations })
        }
    }
}

fn composed_message_summary(c: &tl::types::messages::ComposedMessageWithAi) -> serde_json::Value {
    serde_json::json!({
        "result_text": text_with_entities_text(&c.result_text),
        "diff_text": c.diff_text.as_ref().map(text_with_entities_text),
    })
}

fn text_with_entities_text(t: &tl::enums::TextWithEntities) -> String {
    match t {
        tl::enums::TextWithEntities::Entities(x) => x.text.clone(),
    }
}

fn authorization_row(auth: &tl::enums::Authorization) -> serde_json::Value {
    let tl::enums::Authorization::Authorization(a) = auth;
    serde_json::json!({
        "hash": a.hash,
        "current": a.current,
        "official_app": a.official_app,
        "password_pending": a.password_pending,
        "unconfirmed": a.unconfirmed,
        "device_model": a.device_model,
        "platform": a.platform,
        "system_version": a.system_version,
        "api_id": a.api_id,
        "app_name": a.app_name,
        "app_version": a.app_version,
        "date_created": a.date_created,
        "date_active": a.date_active,
        "ip": a.ip,
        "country": a.country,
    })
}

fn full_channel_summary(r: &tl::enums::messages::ChatFull) -> serde_json::Value {
    match r {
        tl::enums::messages::ChatFull::Full(full) => match &full.full_chat {
            tl::enums::ChatFull::ChannelFull(c) => serde_json::json!({
                "id": c.id,
                "about": c.about,
                "participants_count": c.participants_count,
                "admins_count": c.admins_count,
                "kicked_count": c.kicked_count,
                "banned_count": c.banned_count,
                "online_count": c.online_count,
                "slowmode_seconds": c.slowmode_seconds,
                "linked_chat_id": c.linked_chat_id,
                "pinned_msg_id": c.pinned_msg_id,
                "can_view_stats": c.can_view_stats,
                "hidden_prehistory": c.hidden_prehistory,
                "has_scheduled": c.has_scheduled,
            }),
            tl::enums::ChatFull::Full(f) => serde_json::json!({
                "id": f.id,
                "about": f.about,
                "participants_count": chat_participants_count(&f.participants),
                "pinned_msg_id": f.pinned_msg_id,
                "has_scheduled": f.has_scheduled,
            }),
        },
    }
}

fn chat_participants_count(participants: &tl::enums::ChatParticipants) -> serde_json::Value {
    match participants {
        tl::enums::ChatParticipants::Participants(parts) => {
            serde_json::json!(parts.participants.len())
        }
        tl::enums::ChatParticipants::Forbidden(_) => serde_json::Value::Null,
    }
}

fn message_views_summary(views: &[tl::enums::MessageViews]) -> serde_json::Value {
    let rows = views
        .iter()
        .map(|v| match v {
            tl::enums::MessageViews::Views(mv) => serde_json::json!({
                "views": mv.views,
                "forwards": mv.forwards,
            }),
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "views": rows })
}

fn dialog_unread_marks_summary(marks: &[tl::enums::DialogPeer]) -> serde_json::Value {
    let mut peers = Vec::new();
    let mut folders = Vec::new();
    for mark in marks {
        match mark {
            tl::enums::DialogPeer::Peer(dp) => peers.push(peer_id(&dp.peer)),
            tl::enums::DialogPeer::Folder(f) => folders.push(f.folder_id),
        }
    }
    serde_json::json!({ "peers": peers, "folders": folders })
}

fn user_summary(u: &tl::enums::User) -> serde_json::Value {
    match u {
        tl::enums::User::User(u) => serde_json::json!({
            "id": u.id,
            "first_name": u.first_name,
            "last_name": u.last_name,
            "username": u.username,
        }),
        _ => serde_json::json!({}),
    }
}

fn bool_result(ok: bool) -> serde_json::Value {
    serde_json::json!({ "ok": ok })
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
    fn validate_params_rejects_empty_required_chat() {
        assert!(matches!(
            validate_params(
                "messages.ExportChatInvite",
                &serde_json::json!({"chat": ""})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn validate_params_rejects_whitespace_channel() {
        assert!(matches!(
            validate_params(
                "stats.GetBroadcastStats",
                &serde_json::json!({"channel": "   "})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn validate_params_rejects_missing_required_ints_pre_connect() {
        for (method, field, extra) in [
            ("contacts.Search", "limit", serde_json::json!({"q": "x"})),
            (
                "messages.AppendTodoList",
                "msg_id",
                serde_json::json!({"chat": "@ok"}),
            ),
            (
                "messages.ToggleTodoCompleted",
                "msg_id",
                serde_json::json!({"chat": "@ok"}),
            ),
            (
                "messages.TranscribeAudio",
                "msg_id",
                serde_json::json!({"chat": "@ok"}),
            ),
        ] {
            let err = validate_params(method, &extra).expect_err("missing required int must fail");
            assert!(matches!(err, TeleError::Usage(_)), "{method}: {err}");
            assert!(
                err.message().contains(field) && err.message().contains("required"),
                "{method}: {err}"
            );
        }
    }

    #[test]
    fn validate_params_allows_defaultable_ints_to_stay_absent() {
        assert!(
            validate_params("messages.GetHistory", &serde_json::json!({"chat": "@ok"})).is_ok()
        );
        assert!(validate_params("messages.GetAvailableEffects", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn validate_params_accepts_valid_chat() {
        assert!(validate_params(
            "messages.ExportChatInvite",
            &serde_json::json!({"chat": "me"})
        )
        .is_ok());
    }

    #[test]
    fn validate_params_missing_key_still_usage() {
        assert!(matches!(
            validate_params("messages.ExportChatInvite", &serde_json::json!({})),
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
        assert!(requires_explicit_account("account.SetAuthorizationTTL"));
        assert!(requires_explicit_account("contacts.DeleteByPhones"));
        assert!(requires_explicit_account("messages.ExportChatInvite"));
        assert!(requires_explicit_account("messages.AppendTodoList"));
        assert!(requires_explicit_account("messages.SendScheduledMessages"));
        assert!(requires_explicit_account("messages.ToggleTodoCompleted"));
        assert!(!requires_explicit_account("messages.GetAllDrafts"));
        assert!(!requires_explicit_account("contacts.Search"));
        assert!(!requires_explicit_account("account.GetAuthorizations"));
        assert!(!requires_explicit_account("messages.GetAvailableEffects"));
        assert!(!requires_explicit_account("messages.TranslateText"));
        assert!(!requires_explicit_account("messages.TranscribeAudio"));
        assert!(!requires_explicit_account("messages.ComposeMessageWithAI"));
    }

    #[test]
    fn registered_mutators_are_gated() {
        for name in REGISTERED {
            let mutating = matches!(
                *name,
                "account.UpdateProfile"
                    | "account.SetAuthorizationTTL"
                    | "contacts.DeleteByPhones"
                    | "messages.ExportChatInvite"
                    | "messages.AppendTodoList"
                    | "messages.SendScheduledMessages"
                    | "messages.ToggleTodoCompleted"
            );
            assert_eq!(requires_explicit_account(name), mutating, "{name}");
        }
    }

    #[test]
    fn new_registry_entries_have_validation_and_args_shapes() {
        assert!(validate_params(
            "channels.GetFullChannel",
            &serde_json::json!({"channel": "@x"})
        )
        .is_ok());
        assert!(matches!(
            validate_params("channels.GetFullChannel", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params("users.GetUsers", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(
            validate_params("users.GetUsers", &serde_json::json!({"id": ["me", 12345]})).is_ok()
        );
        assert!(matches!(
            validate_params("messages.Search", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params("messages.GetHistory", &serde_json::json!({"chat": "@x"})).is_ok());
        assert!(validate_params(
            "messages.GetScheduledHistory",
            &serde_json::json!({"chat": "@x"})
        )
        .is_ok());
        assert!(validate_params(
            "messages.GetMessagesViews",
            &serde_json::json!({"chat": "@x", "id": [1, 2], "increment": true})
        )
        .is_ok());
        assert!(
            validate_params("messages.ReadReactions", &serde_json::json!({"chat": "@x"})).is_ok()
        );
        assert!(validate_params(
            "messages.ReadMentions",
            &serde_json::json!({"chat": "@x", "top_msg_id": 5})
        )
        .is_ok());
        assert!(validate_params("account.GetAuthorizations", &serde_json::json!({})).is_ok());
        assert!(validate_params("account.SetAuthorizationTTL", &serde_json::json!({})).is_ok());
        assert!(validate_params("messages.GetDialogUnreadMarks", &serde_json::json!({})).is_ok());
        assert!(matches!(
            validate_params("contacts.DeleteByPhones", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn search_filter_maps_known_names_and_rejects_unknown() {
        assert!(search_filter("").is_ok());
        assert!(search_filter(" photos ").is_ok());
        assert!(search_filter("docs").is_ok());
        assert!(search_filter("voice").is_ok());
        let err = search_filter("stickers").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("valid names"));
    }

    #[test]
    fn list_field_helpers_validate_shapes() {
        assert_eq!(
            int_list_field(&serde_json::json!({"id": [1, 2]}), "id").unwrap(),
            vec![1, 2]
        );
        assert!(int_list_field(&serde_json::json!({"id": ["a"]}), "id").is_err());
        assert!(int_list_field(&serde_json::json!({}), "id").is_err());
        assert_eq!(
            string_list_field(&serde_json::json!({"phones": ["+1555"]}), "phones").unwrap(),
            vec!["+1555"]
        );
        assert!(string_list_field(&serde_json::json!({"phones": [1]}), "phones").is_err());
        assert_eq!(
            peer_targets(&serde_json::json!({"id": ["@a", 42]}), "id").unwrap(),
            vec!["@a".to_string(), "42".to_string()]
        );
        assert!(peer_targets(&serde_json::json!({"id": []}), "id").is_err());
        assert!(peer_targets(&serde_json::json!({"id": [true]}), "id").is_err());
        assert_eq!(long_field(&serde_json::json!({"hash": 7}), "hash"), 7);
        assert_eq!(long_field(&serde_json::json!({}), "hash"), 0);
    }

    #[test]
    fn messages_messages_summary_covers_all_variants_offline() {
        use tl::enums::Message;
        use tl::types::messages::MessagesSlice;
        use tl::types::MessageEmpty;

        let slice = tl::enums::messages::Messages::Slice(MessagesSlice {
            inexact: false,
            count: 2,
            next_rate: None,
            offset_id_offset: None,
            search_flood: None,
            messages: vec![Message::Empty(MessageEmpty {
                id: 9,
                peer_id: None,
            })],
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        });
        let v = messages_messages_summary(&slice);
        assert_eq!(v["count"], serde_json::json!(2));
        assert_eq!(v["chats"], serde_json::json!(0));
        assert_eq!(v["users"], serde_json::json!(0));
        assert_eq!(v["messages"][0]["id"], serde_json::json!(9));
        assert_eq!(v["messages"][0]["empty"], serde_json::json!(true));

        let not_modified =
            tl::enums::messages::Messages::NotModified(tl::types::messages::MessagesNotModified {
                count: 4,
            });
        let v = messages_messages_summary(&not_modified);
        assert_eq!(v["not_modified"], serde_json::json!(true));
        assert_eq!(v["count"], serde_json::json!(4));

        let plain = tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
            messages: Vec::new(),
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        });
        let v = messages_messages_summary(&plain);
        assert_eq!(v["count"], serde_json::json!(0));
    }

    #[test]
    fn affected_history_summary_exposes_pts_fields() {
        let r =
            tl::enums::messages::AffectedHistory::History(tl::types::messages::AffectedHistory {
                pts: 10,
                pts_count: 2,
                offset: 1,
            });
        let v = affected_history_summary(&r);
        assert_eq!(v["pts"], serde_json::json!(10));
        assert_eq!(v["pts_count"], serde_json::json!(2));
        assert_eq!(v["offset"], serde_json::json!(1));
    }

    #[test]
    fn authorizations_summary_carries_ttl_and_rows_without_secrets() {
        let auths = tl::enums::account::Authorizations::Authorizations(
            tl::types::account::Authorizations {
                authorization_ttl_days: 182,
                authorizations: vec![tl::enums::Authorization::Authorization(
                    tl::types::Authorization {
                        current: true,
                        official_app: false,
                        password_pending: false,
                        encrypted_requests_disabled: false,
                        call_requests_disabled: false,
                        unconfirmed: false,
                        hash: 123456789,
                        device_model: "PC".to_string(),
                        platform: "Windows".to_string(),
                        system_version: "11".to_string(),
                        api_id: 42,
                        app_name: "tele".to_string(),
                        app_version: "1.0".to_string(),
                        date_created: 1700000000,
                        date_active: 1700000001,
                        ip: "203.0.113.7".to_string(),
                        country: "US".to_string(),
                        region: String::new(),
                    },
                )],
            },
        );
        match auths {
            tl::enums::account::Authorizations::Authorizations(a) => {
                let v = serde_json::json!({
                    "authorization_ttl_days": a.authorization_ttl_days,
                    "authorizations":
                        a.authorizations.iter().map(authorization_row).collect::<Vec<_>>(),
                });
                assert_eq!(v["authorization_ttl_days"], serde_json::json!(182));
                assert_eq!(
                    v["authorizations"][0]["device_model"],
                    serde_json::json!("PC")
                );
                assert_eq!(v["authorizations"][0]["current"], serde_json::json!(true));
            }
        }
    }

    #[test]
    fn full_channel_summary_handles_channel_and_basic_group_variants() {
        let channel = tl::enums::messages::ChatFull::Full(tl::types::messages::ChatFull {
            full_chat: tl::enums::ChatFull::ChannelFull(tl::types::ChannelFull {
                can_view_participants: false,
                can_set_username: false,
                can_set_stickers: false,
                hidden_prehistory: false,
                can_set_location: false,
                has_scheduled: true,
                can_view_stats: true,
                blocked: false,
                can_delete_channel: false,
                antispam: false,
                participants_hidden: false,
                translations_disabled: false,
                stories_pinned_available: false,
                view_forum_as_messages: false,
                restricted_sponsored: false,
                can_view_revenue: false,
                paid_media_allowed: false,
                can_view_stars_revenue: false,
                paid_reactions_available: false,
                stargifts_available: false,
                paid_messages_available: false,
                id: 100,
                about: "about text".to_string(),
                participants_count: Some(500),
                admins_count: None,
                kicked_count: None,
                banned_count: None,
                online_count: Some(12),
                read_inbox_max_id: 0,
                read_outbox_max_id: 0,
                unread_count: 0,
                chat_photo: tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 }),
                notify_settings: notify_settings_fixture(),
                exported_invite: None,
                bot_info: Vec::new(),
                migrated_from_chat_id: None,
                migrated_from_max_id: None,
                pinned_msg_id: Some(77),
                stickerset: None,
                available_min_id: None,
                folder_id: None,
                linked_chat_id: Some(200),
                location: None,
                slowmode_seconds: Some(30),
                slowmode_next_send_date: None,
                stats_dc: None,
                pts: 0,
                call: None,
                ttl_period: None,
                pending_suggestions: None,
                groupcall_default_join_as: None,
                theme_emoticon: None,
                requests_pending: None,
                recent_requesters: None,
                default_send_as: None,
                available_reactions: None,
                reactions_limit: None,
                stories: None,
                wallpaper: None,
                boosts_applied: None,
                boosts_unrestrict: None,
                emojiset: None,
                bot_verification: None,
                stargifts_count: None,
                send_paid_messages_stars: None,
                main_tab: None,
                guard_bot_id: None,
            }),
            chats: Vec::new(),
            users: Vec::new(),
        });
        let v = full_channel_summary(&channel);
        assert_eq!(v["participants_count"], serde_json::json!(500));
        assert_eq!(v["slowmode_seconds"], serde_json::json!(30));
        assert_eq!(v["linked_chat_id"], serde_json::json!(200));
        assert_eq!(v["pinned_msg_id"], serde_json::json!(77));
        assert_eq!(v["can_view_stats"], serde_json::json!(true));
        assert_eq!(v["has_scheduled"], serde_json::json!(true));

        let basic = tl::enums::messages::ChatFull::Full(tl::types::messages::ChatFull {
            full_chat: tl::enums::ChatFull::Full(tl::types::ChatFull {
                can_set_username: false,
                has_scheduled: false,
                translations_disabled: false,
                id: 300,
                about: "group".to_string(),
                participants: tl::enums::ChatParticipants::Forbidden(
                    tl::types::ChatParticipantsForbidden {
                        chat_id: 300,
                        self_participant: None,
                    },
                ),
                chat_photo: Some(tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 })),
                notify_settings: notify_settings_fixture(),
                exported_invite: None,
                bot_info: None,
                pinned_msg_id: None,
                folder_id: None,
                call: None,
                ttl_period: None,
                groupcall_default_join_as: None,
                theme_emoticon: None,
                requests_pending: None,
                recent_requesters: None,
                available_reactions: None,
                reactions_limit: None,
            }),
            chats: Vec::new(),
            users: Vec::new(),
        });
        let v = full_channel_summary(&basic);
        assert_eq!(v["about"], serde_json::json!("group"));
        assert_eq!(v["participants_count"], serde_json::Value::Null);
    }

    #[cfg(test)]
    fn notify_settings_fixture() -> tl::enums::PeerNotifySettings {
        tl::enums::PeerNotifySettings::Settings(tl::types::PeerNotifySettings {
            show_previews: None,
            silent: None,
            mute_until: None,
            ios_sound: None,
            android_sound: None,
            other_sound: None,
            stories_muted: None,
            stories_hide_sender: None,
            stories_ios_sound: None,
            stories_android_sound: None,
            stories_other_sound: None,
        })
    }

    #[test]
    fn message_views_summary_shapes_rows_offline() {
        let views = vec![
            tl::enums::MessageViews::Views(tl::types::MessageViews {
                views: Some(120),
                forwards: Some(3),
                replies: None,
            }),
            tl::enums::MessageViews::Views(tl::types::MessageViews {
                views: None,
                forwards: None,
                replies: None,
            }),
        ];
        let v = message_views_summary(&views);
        assert_eq!(v["views"].as_array().map(Vec::len), Some(2));
        assert_eq!(v["views"][0]["views"], serde_json::json!(120));
        assert_eq!(v["views"][0]["forwards"], serde_json::json!(3));
        assert!(v["views"][1]["views"].is_null());

        let empty = message_views_summary(&[]);
        assert_eq!(empty["views"], serde_json::json!([]));
    }

    #[test]
    fn dialog_unread_marks_summary_splits_peers_and_folders() {
        let marks = vec![
            tl::enums::DialogPeer::Peer(tl::types::DialogPeer {
                peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 55 }),
            }),
            tl::enums::DialogPeer::Folder(tl::types::DialogPeerFolder { folder_id: 1 }),
        ];
        let v = dialog_unread_marks_summary(&marks);
        assert_eq!(v["peers"], serde_json::json!([55]));
        assert_eq!(v["folders"], serde_json::json!([1]));

        let empty = dialog_unread_marks_summary(&[]);
        assert_eq!(empty["peers"], serde_json::json!([]));
        assert_eq!(empty["folders"], serde_json::json!([]));
    }

    #[test]
    fn user_summary_and_bool_result_shape_verbatim_payloads() {
        let named = tl::enums::User::User(tl::types::User {
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
            apply_min_photo: true,
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
            id: 7,
            access_hash: None,
            first_name: Some("Ada".to_string()),
            last_name: None,
            username: Some("ada".to_string()),
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
        });
        let v = user_summary(&named);
        assert_eq!(v["id"], serde_json::json!(7));
        assert_eq!(v["first_name"], serde_json::json!("Ada"));
        assert_eq!(v["username"], serde_json::json!("ada"));

        let empty = tl::enums::User::Empty(tl::types::UserEmpty { id: 3 });
        let v = user_summary(&empty);
        assert_eq!(v, serde_json::json!({}));

        assert_eq!(bool_result(true), serde_json::json!({"ok": true}));
        assert_eq!(bool_result(false), serde_json::json!({"ok": false}));
    }

    #[test]
    fn raw_dry_run_carries_argument_keys() {
        let params = serde_json::json!({"q": "alice", "limit": 5});
        let value = raw_dry_run_payload("contacts.Search", &params);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["method"], serde_json::json!("contacts.Search"));
        assert_eq!(value["args"], params);
        assert_eq!(
            value["would"],
            serde_json::json!("invoke raw method contacts.Search")
        );
    }

    #[test]
    fn updates_summary_never_emits_debug_strings() {
        let too_long = tl::enums::Updates::TooLong;
        let v = updates_summary(&too_long);
        assert_eq!(v["kind"], serde_json::json!("other"));
        assert!(!v.to_string().contains("UpdatesTooLong"));

        let combined = tl::enums::Updates::Combined(tl::types::UpdatesCombined {
            updates: Vec::new(),
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq_start: 0,
            seq: 0,
        });
        let v = updates_summary(&combined);
        assert_eq!(v["kind"], serde_json::json!("Combined"));
        assert!(!v.to_string().contains("UpdatesCombined"));
    }

    #[test]
    fn new_batch_validate_params_happy_and_sad_paths() {
        assert!(matches!(
            validate_params("messages.SendScheduledMessages", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "messages.SendScheduledMessages",
                &serde_json::json!({"chat": "@x"})
            ),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.SendScheduledMessages",
            &serde_json::json!({"chat": "@x", "id": [1, 2]})
        )
        .is_ok());
        assert!(matches!(
            validate_params("messages.AppendTodoList", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.AppendTodoList",
            &serde_json::json!({"chat": "@x", "msg_id": 5, "list": [{"id": 1, "text": "a"}]})
        )
        .is_ok());
        assert!(matches!(
            validate_params(
                "messages.ToggleTodoCompleted",
                &serde_json::json!({"chat": "@x", "msg_id": 5})
            ),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.ToggleTodoCompleted",
            &serde_json::json!({"chat": "@x", "msg_id": 5, "completed": [1], "incompleted": []})
        )
        .is_ok());
        assert!(validate_params("messages.GetAvailableEffects", &serde_json::json!({})).is_ok());
        assert!(validate_params(
            "messages.GetAvailableEffects",
            &serde_json::json!({"hash": 5})
        )
        .is_ok());
        assert!(matches!(
            validate_params("messages.TranscribeAudio", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.TranscribeAudio",
            &serde_json::json!({"chat": "@x", "msg_id": 7})
        )
        .is_ok());
        assert!(matches!(
            validate_params("messages.TranslateText", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "messages.TranslateText",
                &serde_json::json!({"to_lang": ""})
            ),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.TranslateText",
            &serde_json::json!({"to_lang": "fa", "text": ["hi"]})
        )
        .is_ok());
        assert!(validate_params(
            "messages.TranslateText",
            &serde_json::json!({"to_lang": "fa", "id": [3], "tone": "formal"})
        )
        .is_ok());
        assert!(matches!(
            validate_params("messages.ComposeMessageWithAI", &serde_json::json!({})),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "messages.ComposeMessageWithAI",
                &serde_json::json!({"proofread": true})
            ),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_params(
            "messages.ComposeMessageWithAI",
            &serde_json::json!({"text": "draft text", "proofread": true, "emojify": false})
        )
        .is_ok());
        assert!(validate_params(
            "messages.ComposeMessageWithAI",
            &serde_json::json!({
                "text": "draft text",
                "translate_to_lang": "en",
                "tone": {"slug": "formal"}
            })
        )
        .is_ok());
    }

    #[test]
    fn new_batch_rejects_unknown_keys() {
        assert!(matches!(
            validate_params(
                "messages.SendScheduledMessages",
                &serde_json::json!({"chat": "@x", "id": [1], "schedule": "daily"})
            ),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_params(
                "messages.ComposeMessageWithAI",
                &serde_json::json!({"text": "x", "send": true})
            ),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn todo_items_field_parses_objects_and_rejects_bad_shapes() {
        let items = todo_items_field(
            &serde_json::json!({"list": [
                {"id": 1, "text": "buy milk"},
                {"id": 2, "text": "ship it"}
            ]}),
            "list",
        )
        .unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            tl::enums::TodoItem::Item(item) => {
                assert_eq!(item.id, 1);
                match &item.title {
                    tl::enums::TextWithEntities::Entities(t) => {
                        assert_eq!(t.text, "buy milk");
                        assert!(t.entities.is_empty());
                    }
                }
            }
        }
        assert!(todo_items_field(&serde_json::json!({"list": []}), "list").is_err());
        assert!(todo_items_field(&serde_json::json!({}), "list").is_err());
        assert!(todo_items_field(&serde_json::json!({"list": ["x"]}), "list").is_err());
        assert!(todo_items_field(&serde_json::json!({"list": [{"text": "x"}]}), "list").is_err());
        assert!(todo_items_field(&serde_json::json!({"list": [{"id": 1}]}), "list").is_err());
        assert!(todo_items_field(
            &serde_json::json!({"list": [{"id": "1", "text": "x"}]}),
            "list"
        )
        .is_err());
    }

    #[test]
    fn compose_tone_field_maps_string_slug_object_and_rejects_garbage() {
        assert!(compose_tone_field(&serde_json::json!({}))
            .unwrap()
            .is_none());
        match compose_tone_field(&serde_json::json!({"tone": "friendly"})).unwrap() {
            Some(tl::enums::InputAiComposeTone::Default(t)) => assert_eq!(t.tone, "friendly"),
            other => panic!("string tone should map to Default, got {other:?}"),
        }
        match compose_tone_field(&serde_json::json!({"tone": {"slug": "formal"}})).unwrap() {
            Some(tl::enums::InputAiComposeTone::Slug(t)) => assert_eq!(t.slug, "formal"),
            other => panic!("slug object should map to Slug, got {other:?}"),
        }
        match compose_tone_field(&serde_json::json!({"tone": {"tone": "calm"}})).unwrap() {
            Some(tl::enums::InputAiComposeTone::Default(t)) => assert_eq!(t.tone, "calm"),
            other => panic!("named object should map to Default, got {other:?}"),
        }
        assert!(compose_tone_field(&serde_json::json!({"tone": {}})).is_err());
        assert!(compose_tone_field(&serde_json::json!({"tone": 5})).is_err());
    }

    #[test]
    fn available_effects_summary_shapes_effects_and_not_modified() {
        let effects =
            tl::enums::messages::AvailableEffects::Effects(tl::types::messages::AvailableEffects {
                hash: 3,
                effects: vec![tl::enums::AvailableEffect::Effect(
                    tl::types::AvailableEffect {
                        premium_required: true,
                        id: 7,
                        emoticon: "fire".to_string(),
                        static_icon_id: None,
                        effect_sticker_id: 99,
                        effect_animation_id: Some(5),
                    },
                )],
                documents: Vec::new(),
            });
        let v = available_effects_summary(&effects);
        assert_eq!(v["hash"], serde_json::json!(3));
        assert_eq!(v["effects"][0]["id"], serde_json::json!(7));
        assert_eq!(v["effects"][0]["emoticon"], serde_json::json!("fire"));
        assert_eq!(v["effects"][0]["premium_required"], serde_json::json!(true));
        assert_eq!(v["documents"], serde_json::json!(0));

        let not_modified = tl::enums::messages::AvailableEffects::NotModified;
        let v = available_effects_summary(&not_modified);
        assert_eq!(v["not_modified"], serde_json::json!(true));
    }

    #[test]
    fn transcribed_audio_summary_carries_transcription_fields() {
        let audio =
            tl::enums::messages::TranscribedAudio::Audio(tl::types::messages::TranscribedAudio {
                pending: true,
                transcription_id: 1_234_567_890_123,
                text: "transcribed words".to_string(),
                trial_remains_num: Some(2),
                trial_remains_until_date: Some(1_700_000_000),
            });
        let v = transcribed_audio_summary(&audio);
        assert_eq!(v["pending"], serde_json::json!(true));
        assert_eq!(
            v["transcription_id"],
            serde_json::json!(1_234_567_890_123i64)
        );
        assert_eq!(v["text"], serde_json::json!("transcribed words"));
        assert_eq!(v["trial_remains_num"], serde_json::json!(2));
        assert_eq!(
            v["trial_remains_until_date"],
            serde_json::json!(1_700_000_000)
        );

        let plain =
            tl::enums::messages::TranscribedAudio::Audio(tl::types::messages::TranscribedAudio {
                pending: false,
                transcription_id: 9,
                text: String::new(),
                trial_remains_num: None,
                trial_remains_until_date: None,
            });
        let v = transcribed_audio_summary(&plain);
        assert_eq!(v["pending"], serde_json::json!(false));
        assert!(v["trial_remains_num"].is_null());
    }

    #[test]
    fn translated_text_summary_extracts_plain_strings() {
        let result = tl::enums::messages::TranslatedText::TranslateResult(
            tl::types::messages::TranslateResult {
                result: vec![
                    tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: "hola".to_string(),
                        entities: Vec::new(),
                    }),
                    tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: "mundo".to_string(),
                        entities: Vec::new(),
                    }),
                ],
            },
        );
        let v = translated_text_summary(&result);
        assert_eq!(v["count"], serde_json::json!(2));
        assert_eq!(v["translations"], serde_json::json!(["hola", "mundo"]));
    }

    #[test]
    fn composed_message_summary_includes_optional_diff() {
        let with_diff = tl::types::messages::ComposedMessageWithAi {
            result_text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: "polished draft".to_string(),
                entities: Vec::new(),
            }),
            diff_text: Some(tl::enums::TextWithEntities::Entities(
                tl::types::TextWithEntities {
                    text: "- old\n+ polished draft".to_string(),
                    entities: Vec::new(),
                },
            )),
        };
        let v = composed_message_summary(&with_diff);
        assert_eq!(v["result_text"], serde_json::json!("polished draft"));
        assert_eq!(v["diff_text"], serde_json::json!("- old\n+ polished draft"));

        let no_diff = tl::types::messages::ComposedMessageWithAi {
            result_text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: "plain".to_string(),
                entities: Vec::new(),
            }),
            diff_text: None,
        };
        let v = composed_message_summary(&no_diff);
        assert_eq!(v["result_text"], serde_json::json!("plain"));
        assert!(v["diff_text"].is_null());
    }

    fn plan_for(
        op: &str,
        params: serde_json::Value,
    ) -> Result<crate::commands::serve::Plan, serde_json::Value> {
        let routes = raw_serve_routes();
        let route = routes
            .iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    #[test]
    fn raw_serve_route_is_single_mutate_op_with_long_timeout() {
        use crate::commands::serve::Lane;
        let routes = raw_serve_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].op, "raw");
        assert_eq!(routes[0].lane, Lane::Mutate);
        assert_eq!(routes[0].timeout, Some(std::time::Duration::from_secs(120)));
    }

    #[test]
    fn missing_method_and_unknown_fields_yield_serve_error() {
        let err = plan_for("raw", serde_json::json!({})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("method"), "{msg}");

        let err = plan_for(
            "raw",
            serde_json::json!({"method": "contacts.Search", "methd": "typo"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err["message"].as_str().unwrap().contains("methd"));
    }

    #[test]
    fn wrong_typed_method_yields_serve_error() {
        let err = plan_for("raw", serde_json::json!({"method": 5})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("invalid type"), "{msg}");
    }

    #[test]
    fn unknown_method_yields_usage_error_naming_it() {
        let err = plan_for(
            "raw",
            serde_json::json!({"method": "messages.FooBar", "args": {}}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("messages.FooBar"), "{msg}");
        assert!(msg.contains("not in registry"), "{msg}");
    }

    #[test]
    fn missing_args_fields_yield_usage_error_from_generated_gates() {
        for call in [
            serde_json::json!({"method": "contacts.Search"}),
            serde_json::json!({"method": "contacts.Search", "args": {}}),
            serde_json::json!({"method": "stats.GetBroadcastStats", "args": {"channel": "  "}}),
        ] {
            let err = plan_for("raw", call.clone()).unwrap_err();
            assert_eq!(err["type"], "UsageError", "{call}");
            let msg = err["message"].as_str().unwrap().to_string();
            assert!(
                msg.contains("--args") || msg.contains("--chat") || msg.contains("--channel"),
                "{call}: {msg}"
            );
        }
    }

    #[test]
    fn non_object_args_yield_usage_error() {
        let err = plan_for(
            "raw",
            serde_json::json!({"method": "messages.GetAllDrafts", "args": [1, 2]}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("JSON object"));
    }

    #[test]
    fn registered_read_only_method_passes_all_gates_and_dry_runs_like_cli() {
        let plan = plan_for(
            "raw",
            serde_json::json!({"method": "messages.GetAllDrafts", "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(data) = plan else {
            panic!("expected dry run");
        };
        assert_eq!(
            data,
            raw_dry_run_payload("messages.GetAllDrafts", &serde_json::json!({}))
        );
        assert_eq!(
            data["would"],
            serde_json::json!("invoke raw method messages.GetAllDrafts")
        );

        let raw = serde_json::json!({"method": "messages.GetAllDrafts"});
        let plan = plan_for("raw", raw.clone()).unwrap();
        match plan {
            crate::commands::serve::Plan::Execute(passed) => assert_eq!(passed, raw),
            other => panic!("expected execute plan, got {other:?}"),
        }

        let with_args = serde_json::json!({"q": "ducks", "limit": 10});
        let plan = plan_for(
            "raw",
            serde_json::json!({"method": "contacts.Search", "args": with_args, "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(data) = plan else {
            panic!("expected dry run");
        };
        assert_eq!(data["args"], with_args);
    }
}
