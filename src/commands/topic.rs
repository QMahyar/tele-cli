use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::chat_target::ChatTarget;
use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum TopicCmd {
    Create(CreateArgs),
    List(ListArgs),
    Close(LifecycleArgs),
    Reopen(LifecycleArgs),
    Edit(EditArgs),
    Delete(LifecycleArgs),
    Pin(LifecycleArgs),
}

#[derive(Args, Clone)]
pub struct CreateArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, help = "topic title")]
    title: String,
    #[arg(
        long,
        help = "single-codepoint emoji for topic icon (optional; custom-emoji document IDs not supported)"
    )]
    emoji: Option<String>,
}

#[derive(Args, Clone)]
pub struct ListArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, default_value_t = 20, help = "max topics to list (1-10000)")]
    limit: u32,
}

#[derive(Args, Clone)]
pub struct LifecycleArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, help = "topic id (root message id; see tele topic list)")]
    topic: String,
    #[arg(long, help = "pin: unpin instead of pin")]
    unpin: bool,
}

#[derive(Args, Clone)]
pub struct EditArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, help = "topic id (root message id; see tele topic list)")]
    topic: String,
    #[arg(long, help = "new topic title")]
    title: Option<String>,
    #[arg(long, help = "closed state: true or false")]
    closed: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Close,
    Reopen,
    Delete,
    Pin,
}

impl ActionKind {
    fn name(self) -> &'static str {
        match self {
            ActionKind::Close => "close",
            ActionKind::Reopen => "reopen",
            ActionKind::Delete => "delete",
            ActionKind::Pin => "pin",
        }
    }
}

pub async fn run(cmd: TopicCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        TopicCmd::Create(a) => create(a, flags).await,
        TopicCmd::List(a) => list(a, flags).await,
        TopicCmd::Close(a) => simple_action(a, flags, ActionKind::Close).await,
        TopicCmd::Reopen(a) => simple_action(a, flags, ActionKind::Reopen).await,
        TopicCmd::Edit(a) => edit(a, flags).await,
        TopicCmd::Delete(a) => simple_action(a, flags, ActionKind::Delete).await,
        TopicCmd::Pin(a) => simple_action(a, flags, ActionKind::Pin).await,
    }
}

async fn create(args: CreateArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    validate_emoji(args.emoji.as_deref())?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(create_dry_run_payload(
                    &args.chat,
                    &args.title,
                    args.emoji.as_deref(),
                ));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            topic_create_core(&guard.shares(), CreateParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn create_dry_run_payload(target: &str, title: &str, emoji: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "dry_run": true,
        "chat": target,
        "title": title,
        "would": format!("create topic \"{title}\" in chat {target}")
    });
    if let Some(e) = emoji {
        value["emoji"] = serde_json::json!(e);
    }
    value
}

pub(crate) async fn topic_create_core(
    shares: &crate::client::ServeShares,
    params: CreateParams,
) -> TeleResult<serde_json::Value> {
    let icon_emoji_id = validate_emoji(params.emoji.as_deref())?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    crate::commands::chat::ensure_chat_peer(&chat, "topic create")?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let _: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::messages::CreateForumTopic {
            title_missing: false,
            peer,
            title: params.title.clone(),
            icon_color: None,
            icon_emoji_id,
            random_id: rand_seed(),
            send_as: None,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "chat": params.chat,
        "title": params.title,
        "ok": true}))
}

async fn simple_action(
    args: LifecycleArgs,
    flags: &GlobalFlags,
    kind: ActionKind,
) -> TeleResult<i32> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    parse_topic_id(&args.topic)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return lifecycle_serve_dry_run_kind(&args, kind.name());
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            topic_action_core(&guard.shares(), LifecycleParams::from(&args), kind).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn topic_close_core(
    shares: &crate::client::ServeShares,
    params: LifecycleParams,
) -> TeleResult<serde_json::Value> {
    topic_action_core(shares, params, ActionKind::Close).await
}

pub(crate) async fn topic_reopen_core(
    shares: &crate::client::ServeShares,
    params: LifecycleParams,
) -> TeleResult<serde_json::Value> {
    topic_action_core(shares, params, ActionKind::Reopen).await
}

pub(crate) async fn topic_pin_core(
    shares: &crate::client::ServeShares,
    params: LifecycleParams,
) -> TeleResult<serde_json::Value> {
    topic_action_core(shares, params, ActionKind::Pin).await
}

pub(crate) async fn topic_delete_core(
    shares: &crate::client::ServeShares,
    params: LifecycleParams,
) -> TeleResult<serde_json::Value> {
    topic_action_core(shares, params, ActionKind::Delete).await
}

async fn topic_action_core(
    shares: &crate::client::ServeShares,
    params: LifecycleParams,
    kind: ActionKind,
) -> TeleResult<serde_json::Value> {
    let topic_id = parse_topic_id(&params.topic)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    crate::commands::chat::ensure_chat_peer(&chat, "topic")?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    match kind {
        ActionKind::Close | ActionKind::Reopen => {
            let _: tl::enums::Updates = shares
                .client
                .invoke(&tl::functions::messages::EditForumTopic {
                    peer,
                    topic_id,
                    title: None,
                    icon_emoji_id: None,
                    closed: Some(kind == ActionKind::Close),
                    hidden: None,
                })
                .await
                .map_err(tele_invocation)?;
        }
        ActionKind::Delete => {
            let _: tl::enums::messages::AffectedHistory = shares
                .client
                .invoke(&tl::functions::messages::DeleteTopicHistory {
                    peer,
                    top_msg_id: topic_id,
                })
                .await
                .map_err(tele_invocation)?;
        }
        ActionKind::Pin => {
            let _: tl::enums::Updates = shares
                .client
                .invoke(&tl::functions::messages::UpdatePinnedForumTopic {
                    peer,
                    topic_id,
                    pinned: !params.unpin,
                })
                .await
                .map_err(tele_invocation)?;
        }
    }
    Ok(serde_json::json!({
        "chat": params.chat,
        "topic": topic_id,
        "ok": true}))
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    parse_topic_id(&args.topic)?;
    validate_edit_changes(args.title.as_deref(), args.closed)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return edit_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            topic_edit_core(&guard.shares(), EditParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn topic_edit_core(
    shares: &crate::client::ServeShares,
    params: EditParams,
) -> TeleResult<serde_json::Value> {
    let topic_id = parse_topic_id(&params.topic)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    crate::commands::chat::ensure_chat_peer(&chat, "topic edit")?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let request = build_edit_request(peer, topic_id, params.title.as_deref(), params.closed);
    let _: tl::enums::Updates = shares
        .client
        .invoke(&request)
        .await
        .map_err(tele_invocation)?;
    Ok(edit_report(
        &params.chat,
        topic_id,
        params.title.as_deref(),
        params.closed,
    ))
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_payload(&args.chat));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = topic_list_core(&guard.shares(), ListParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let table_rows: Vec<Vec<String>> = result["topics"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|r| {
                        vec![
                            r["id"].to_string(),
                            r["title"].as_str().unwrap_or_default().to_string(),
                            r["icon_emoji_id"].to_string(),
                            r["closed"].to_string(),
                            r["pinned"].to_string(),
                        ]
                    })
                    .collect();
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "title", "icon_emoji_id", "closed", "pinned"],
                    &table_rows,
                )?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn topic_list_core(
    shares: &crate::client::ServeShares,
    params: ListParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    crate::commands::chat::ensure_chat_peer(&chat, "topic list")?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let topics = {
        let client_ref = &shares.client;
        let peer_ref = &peer;
        collect_forum_topics(params.limit, move |cursor, page_limit| async move {
            let results: tl::enums::messages::ForumTopics = client_ref
                .invoke(&tl::functions::messages::GetForumTopics {
                    peer: (*peer_ref).clone(),
                    q: None,
                    offset_date: cursor.date,
                    offset_id: cursor.message_id,
                    offset_topic: cursor.topic_id,
                    limit: page_limit as i32,
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::messages::ForumTopics::Topics(topics) = results;
            Ok(ForumTopicsPage {
                topics: topics.topics,
            })
        })
    }
    .await?;
    let mut rows = Vec::new();
    for topic in topics {
        if let Some(row) = topic_row(&topic) {
            rows.push(row);
        }
    }
    Ok(serde_json::json!({"topics": rows}))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ForumCursor {
    date: i32,
    message_id: i32,
    topic_id: i32,
}

struct ForumTopicsPage {
    topics: Vec<tl::enums::ForumTopic>,
}

fn next_cursor(prev: ForumCursor, page: &[tl::enums::ForumTopic]) -> ForumCursor {
    for topic in page.iter().rev() {
        if let tl::enums::ForumTopic::Topic(t) = topic {
            return ForumCursor {
                date: t.date,
                message_id: t.top_message,
                topic_id: t.id,
            };
        }
    }
    prev
}

async fn collect_forum_topics<F, Fut>(
    limit: u32,
    mut fetch: F,
) -> TeleResult<Vec<tl::enums::ForumTopic>>
where
    F: FnMut(ForumCursor, u32) -> Fut,
    Fut: std::future::Future<Output = TeleResult<ForumTopicsPage>>,
{
    let mut topics = Vec::new();
    let mut cursor = ForumCursor::default();
    loop {
        let visible = topics
            .iter()
            .filter(|t| matches!(t, tl::enums::ForumTopic::Topic(_)))
            .count() as u32;
        let remaining = limit.saturating_sub(visible);
        if remaining == 0 {
            break;
        }
        let page = fetch(cursor, remaining.min(100)).await?;
        let page_len = page.topics.len();
        cursor = next_cursor(cursor, &page.topics);
        topics.extend(page.topics);
        if page_len == 0 {
            break;
        }
    }
    Ok(topics)
}

fn validate_emoji(emoji: Option<&str>) -> Result<Option<i64>, TeleError> {
    let Some(emoji) = emoji else {
        return Ok(None);
    };
    if emoji.is_empty() {
        return Err(TeleError::Usage(
            "--emoji cannot be empty; only a single-codepoint emoji (4 UTF-8 bytes) is accepted; custom-emoji document IDs are not supported"
                .to_string(),
        ));
    }

    use unicode_segmentation::UnicodeSegmentation;
    let graphemes: Vec<&str> = emoji.graphemes(true).collect();

    if graphemes.len() != 1 {
        return Err(TeleError::Usage(format!(
            "--emoji \"{emoji}\" must be a single grapheme cluster; multi-codepoint emoji (e.g. family emoji, skin tones) are not supported; custom-emoji document IDs are not supported (got {} grapheme clusters)",
            graphemes.len(),
        )));
    }

    let bytes = emoji.as_bytes();
    if bytes.len() != 4 {
        return Err(TeleError::Usage(format!(
            "--emoji \"{emoji}\" must be exactly 4 UTF-8 bytes; multi-codepoint emoji are not supported; custom-emoji document IDs are not supported (got {} bytes)",
            bytes.len(),
        )));
    }

    let c = emoji.chars().next().unwrap();
    if !is_emoji_codepoint(c) {
        return Err(TeleError::Usage(format!(
            "--emoji \"{emoji}\" is not a recognized emoji; only single-codepoint emoji are accepted; custom-emoji document IDs are not supported"
        )));
    }
    Ok(Some(i64::from_be_bytes([
        0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}

fn is_emoji_codepoint(c: char) -> bool {
    matches!(
        c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x2300..=0x23FF
    )
}

fn rand_seed() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    nanos ^ (nanos << 21) ^ (nanos >> 19)
}

fn list_dry_run_payload(target: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": target,
        "would": format!("list topics in chat {target}")
    })
}

fn parse_topic_id(raw: &str) -> TeleResult<i32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TeleError::Usage("--topic must not be empty".to_string()));
    }
    let id = trimmed
        .parse::<i32>()
        .map_err(|_| TeleError::Usage(format!("--topic \"{raw}\" must be an integer topic id")))?;
    if id <= 0 {
        return Err(TeleError::Usage(format!(
            "--topic {id} must be a positive topic id (see tele topic list)"
        )));
    }
    Ok(id)
}

fn validate_edit_changes(title: Option<&str>, closed: Option<bool>) -> TeleResult<()> {
    if title.is_none() && closed.is_none() {
        return Err(TeleError::Usage(
            "nothing to change: pass --title and/or --closed".to_string(),
        ));
    }
    if let Some(t) = title {
        if t.trim().is_empty() {
            return Err(TeleError::Usage("--title must not be empty".to_string()));
        }
    }
    Ok(())
}

fn build_edit_request(
    peer: tl::enums::InputPeer,
    topic_id: i32,
    title: Option<&str>,
    closed: Option<bool>,
) -> tl::functions::messages::EditForumTopic {
    tl::functions::messages::EditForumTopic {
        peer,
        topic_id,
        title: title.map(str::to_string),
        icon_emoji_id: None,
        closed,
        hidden: None,
    }
}

fn lifecycle_dry_run_payload(chat: &str, topic_id: i32, action: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "topic": topic_id,
        "would": format!("{action} topic {topic_id} in chat {chat}")
    })
}

fn edit_dry_run_payload(
    chat: &str,
    topic_id: i32,
    title: Option<&str>,
    closed: Option<bool>,
) -> serde_json::Value {
    let mut value = lifecycle_dry_run_payload(chat, topic_id, "edit");
    if let Some(t) = title {
        value["title"] = serde_json::json!(t);
    }
    if let Some(c) = closed {
        value["closed"] = serde_json::json!(c);
    }
    value
}

fn edit_report(
    chat: &str,
    topic_id: i32,
    title: Option<&str>,
    closed: Option<bool>,
) -> serde_json::Value {
    let mut value = serde_json::json!({"chat": chat, "topic": topic_id, "ok": true});
    if let Some(t) = title {
        value["title"] = serde_json::json!(t);
    }
    if let Some(c) = closed {
        value["closed"] = serde_json::json!(c);
    }
    value
}

fn topic_row(topic: &tl::enums::ForumTopic) -> Option<serde_json::Value> {
    match topic {
        tl::enums::ForumTopic::Topic(t) => Some(serde_json::json!({
            "id": t.id,
            "title": t.title,
            "icon_emoji_id": t.icon_emoji_id,
            "closed": t.closed,
            "pinned": t.pinned})),
        tl::enums::ForumTopic::Deleted(_) => None,
    }
}

fn default_topic_limit() -> u32 {
    20
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct CreateParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) title: String,
    pub(crate) emoji: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&CreateArgs> for CreateParams {
    fn from(a: &CreateArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            title: a.title.clone(),
            emoji: a.emoji.clone(),
            dry_run: false,
        }
    }
}

impl From<&CreateParams> for CreateArgs {
    fn from(p: &CreateParams) -> Self {
        Self {
            chat: p.chat.clone(),
            title: p.title.clone(),
            emoji: p.emoji.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ListParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default = "default_topic_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ListArgs> for ListParams {
    fn from(a: &ListArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&ListParams> for ListArgs {
    fn from(p: &ListParams) -> Self {
        Self {
            chat: p.chat.clone(),
            limit: p.limit,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct LifecycleParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) topic: String,
    #[serde(default)]
    pub(crate) unpin: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&LifecycleArgs> for LifecycleParams {
    fn from(a: &LifecycleArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            topic: a.topic.clone(),
            unpin: a.unpin,
            dry_run: false,
        }
    }
}

impl From<&LifecycleParams> for LifecycleArgs {
    fn from(p: &LifecycleParams) -> Self {
        Self {
            chat: p.chat.clone(),
            topic: p.topic.clone(),
            unpin: p.unpin,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct EditParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) topic: String,
    pub(crate) title: Option<String>,
    pub(crate) closed: Option<bool>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&EditArgs> for EditParams {
    fn from(a: &EditArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            topic: a.topic.clone(),
            title: a.title.clone(),
            closed: a.closed,
            dry_run: false,
        }
    }
}

impl From<&EditParams> for EditArgs {
    fn from(p: &EditParams) -> Self {
        Self {
            chat: p.chat.clone(),
            topic: p.topic.clone(),
            title: p.title.clone(),
            closed: p.closed,
        }
    }
}

pub(crate) fn validate_topic_id(id: i32) -> TeleResult<()> {
    if id <= 0 {
        return Err(TeleError::Usage(format!(
            "--topic {id} must be a positive topic id (see tele topic list)"
        )));
    }
    Ok(())
}

pub(crate) fn validate_create(args: &CreateArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    validate_emoji(args.emoji.as_deref())?;
    Ok(())
}

pub(crate) fn validate_list(args: &ListArgs) -> TeleResult<()> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    Ok(())
}

pub(crate) fn validate_lifecycle(args: &LifecycleArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    validate_topic_id(parse_topic_id(&args.topic)?)?;
    Ok(())
}

pub(crate) fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    ChatTarget::parse_flag(&args.chat, "chat")?;
    validate_topic_id(parse_topic_id(&args.topic)?)?;
    validate_edit_changes(args.title.as_deref(), args.closed)?;
    Ok(())
}

pub(crate) fn create_serve_dry_run(args: &CreateArgs) -> TeleResult<serde_json::Value> {
    Ok(create_dry_run_payload(
        &args.chat,
        &args.title,
        args.emoji.as_deref(),
    ))
}

pub(crate) fn list_serve_dry_run(args: &ListArgs) -> TeleResult<serde_json::Value> {
    Ok(list_dry_run_payload(&args.chat))
}

fn lifecycle_serve_dry_run_kind(
    args: &LifecycleArgs,
    action: &str,
) -> TeleResult<serde_json::Value> {
    Ok(lifecycle_dry_run_payload(
        &args.chat,
        parse_topic_id(&args.topic)?,
        action,
    ))
}

pub(crate) fn close_serve_dry_run(args: &LifecycleArgs) -> TeleResult<serde_json::Value> {
    lifecycle_serve_dry_run_kind(args, "close")
}

pub(crate) fn reopen_serve_dry_run(args: &LifecycleArgs) -> TeleResult<serde_json::Value> {
    lifecycle_serve_dry_run_kind(args, "reopen")
}

pub(crate) fn pin_serve_dry_run(args: &LifecycleArgs) -> TeleResult<serde_json::Value> {
    lifecycle_serve_dry_run_kind(args, "pin")
}

pub(crate) fn delete_serve_dry_run(args: &LifecycleArgs) -> TeleResult<serde_json::Value> {
    lifecycle_serve_dry_run_kind(args, "delete")
}

pub(crate) fn edit_serve_dry_run(args: &EditArgs) -> TeleResult<serde_json::Value> {
    Ok(edit_dry_run_payload(
        &args.chat,
        parse_topic_id(&args.topic)?,
        args.title.as_deref(),
        args.closed,
    ))
}

pub(crate) fn topic_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "topic close",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "close a forum topic",
            LifecycleParams,
            LifecycleArgs,
            validate_lifecycle,
            close_serve_dry_run,
            run_close,
            crate::commands::serve::params_schema::<LifecycleParams>
        ),
        crate::serve_route!(
            "topic create",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "create a forum topic in a channel",
            CreateParams,
            CreateArgs,
            validate_create,
            create_serve_dry_run,
            run_create,
            crate::commands::serve::params_schema::<CreateParams>
        ),
        crate::serve_route!(
            "topic delete",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            true,
            true,
            "delete a forum topic",
            LifecycleParams,
            LifecycleArgs,
            validate_lifecycle,
            delete_serve_dry_run,
            run_delete,
            crate::commands::serve::params_schema::<LifecycleParams>
        ),
        crate::serve_route!(
            "topic edit",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "rename or re-icon a forum topic",
            EditParams,
            EditArgs,
            validate_edit,
            edit_serve_dry_run,
            run_edit,
            crate::commands::serve::params_schema::<EditParams>
        ),
        crate::serve_route!(
            "topic list",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list forum topics in a channel",
            ListParams,
            ListArgs,
            validate_list,
            list_serve_dry_run,
            run_list,
            crate::commands::serve::params_schema::<ListParams>
        ),
        crate::serve_route!(
            "topic pin",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "pin or unpin a forum topic",
            LifecycleParams,
            LifecycleArgs,
            validate_lifecycle,
            pin_serve_dry_run,
            run_pin,
            crate::commands::serve::params_schema::<LifecycleParams>
        ),
        crate::serve_route!(
            "topic reopen",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "reopen a closed forum topic",
            LifecycleParams,
            LifecycleArgs,
            validate_lifecycle,
            reopen_serve_dry_run,
            run_reopen,
            crate::commands::serve::params_schema::<LifecycleParams>
        ),
    ]
}

crate::serve_runner!(run_close, topic_close_core, LifecycleParams);
crate::serve_runner!(run_create, topic_create_core, CreateParams);
crate::serve_runner!(run_delete, topic_delete_core, LifecycleParams);
crate::serve_runner!(run_edit, topic_edit_core, EditParams);
crate::serve_runner!(run_list, topic_list_core, ListParams);
crate::serve_runner!(run_pin, topic_pin_core, LifecycleParams);
crate::serve_runner!(run_reopen, topic_reopen_core, LifecycleParams);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::serve::{Lane, Plan};
    use crate::error::TeleError;
    use crate::error::EXIT_USAGE;

    fn plan_topic_op(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
        let route = topic_serve_routes()
            .into_iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    fn serve_error_message(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "ServeError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn usage_error_message(err: serde_json::Value) -> String {
        assert_eq!(err["type"], "UsageError", "{err}");
        err["message"].as_str().unwrap().to_string()
    }

    fn expect_execute(plan: Plan, raw: &serde_json::Value) {
        match plan {
            Plan::Execute(passed) => assert_eq!(&passed, raw),
            other => panic!("expected execute plan, got {other:?}"),
        }
    }

    fn lifecycle_matrix(op: &str, action: &str) {
        let msg = serve_error_message(
            plan_topic_op(op, serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("topic"), "{msg}");

        let msg = serve_error_message(
            plan_topic_op(op, serde_json::json!({"chat": "work", "topic": 5})).unwrap_err(),
        );
        assert!(msg.contains("string"), "{msg}");

        let msg = usage_error_message(
            plan_topic_op(op, serde_json::json!({"chat": "work", "topic": "0"})).unwrap_err(),
        );
        assert!(msg.contains("positive"), "{msg}");

        let plan = plan_topic_op(
            op,
            serde_json::json!({"chat": "work", "topic": "5", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, lifecycle_dry_run_payload("work", 5, action)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "work", "topic": "5"});
        let plan = plan_topic_op(op, raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_topic_create_plan_matrix() {
        let msg = serve_error_message(
            plan_topic_op("topic create", serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("title"), "{msg}");

        let msg = serve_error_message(
            plan_topic_op(
                "topic create",
                serde_json::json!({"chat": "work", "title": "T", "emoji": 5}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("string"), "{msg}");

        let msg = usage_error_message(
            plan_topic_op(
                "topic create",
                serde_json::json!({"chat": "work", "title": "T", "emoji": "ab"}),
            )
            .unwrap_err(),
        );
        assert!(
            msg.contains("custom-emoji document IDs are not supported"),
            "{msg}"
        );

        let plan = plan_topic_op(
            "topic create",
            serde_json::json!({"chat": "work", "title": "T", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, create_dry_run_payload("work", "T", None)),
            other => panic!("expected dry run plan, got {other:?}"),
        }
        let plan = plan_topic_op(
            "topic create",
            serde_json::json!({"chat": "work", "title": "T", "emoji": "\u{1F600}", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data["dry_run"], serde_json::json!(true)),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "work", "title": "T"});
        let plan = plan_topic_op("topic create", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_topic_list_plan_matrix() {
        let msg = serve_error_message(
            plan_topic_op("topic list", serde_json::json!({"limit": "many"})).unwrap_err(),
        );
        assert!(msg.contains("u32"), "{msg}");

        let msg = usage_error_message(
            plan_topic_op("topic list", serde_json::json!({"limit": 10_001})).unwrap_err(),
        );
        assert!(msg.contains("--limit"), "{msg}");

        let plan = plan_topic_op(
            "topic list",
            serde_json::json!({"chat": "work", "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => assert_eq!(data, list_dry_run_payload("work")),
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "work"});
        let plan = plan_topic_op("topic list", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_topic_edit_plan_matrix() {
        let msg = serve_error_message(
            plan_topic_op("topic edit", serde_json::json!({"chat": "work"})).unwrap_err(),
        );
        assert!(msg.contains("missing field"), "{msg}");
        assert!(msg.contains("topic"), "{msg}");

        let msg = serve_error_message(
            plan_topic_op(
                "topic edit",
                serde_json::json!({"chat": "work", "topic": "5", "closed": "yes"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("boolean"), "{msg}");

        let msg = usage_error_message(
            plan_topic_op(
                "topic edit",
                serde_json::json!({"chat": "work", "topic": "5"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("nothing to change"), "{msg}");
        let msg = usage_error_message(
            plan_topic_op(
                "topic edit",
                serde_json::json!({"chat": "work", "topic": "-1", "title": "n"}),
            )
            .unwrap_err(),
        );
        assert!(msg.contains("positive"), "{msg}");

        let plan = plan_topic_op(
            "topic edit",
            serde_json::json!({"chat": "work", "topic": "5", "closed": true, "dry_run": true}),
        )
        .unwrap();
        match plan {
            Plan::DryRun(data) => {
                assert_eq!(data, edit_dry_run_payload("work", 5, None, Some(true)))
            }
            other => panic!("expected dry run plan, got {other:?}"),
        }

        let raw = serde_json::json!({"chat": "work", "topic": "5", "title": "n"});
        let plan = plan_topic_op("topic edit", raw.clone()).unwrap();
        expect_execute(plan, &raw);
    }

    #[test]
    fn serve_topic_close_plan_matrix() {
        lifecycle_matrix("topic close", "close");
    }

    #[test]
    fn serve_topic_reopen_plan_matrix() {
        lifecycle_matrix("topic reopen", "reopen");
    }

    #[test]
    fn serve_topic_pin_plan_matrix() {
        lifecycle_matrix("topic pin", "pin");
    }

    #[test]
    fn serve_topic_delete_plan_matrix() {
        lifecycle_matrix("topic delete", "delete");
    }

    #[test]
    fn topic_serve_lane_and_timeout_table_is_locked() {
        let expected: &[(&str, Lane, Option<u64>)] = &[
            ("topic close", Lane::Mutate, Some(30)),
            ("topic create", Lane::Mutate, Some(30)),
            ("topic delete", Lane::Mutate, Some(30)),
            ("topic edit", Lane::Mutate, Some(30)),
            ("topic list", Lane::Read, Some(120)),
            ("topic pin", Lane::Mutate, Some(30)),
            ("topic reopen", Lane::Mutate, Some(30)),
        ];
        let routes = topic_serve_routes();
        assert_eq!(routes.len(), expected.len());
        for (op, lane, secs) in expected {
            let route = routes
                .iter()
                .find(|r| r.op == *op)
                .unwrap_or_else(|| panic!("route missing for {op}"));
            assert_eq!(route.lane, *lane, "lane for {op}");
            assert_eq!(
                route.timeout,
                secs.map(std::time::Duration::from_secs),
                "timeout for {op}"
            );
        }
    }

    #[test]
    fn list_dry_run_payload_marks_dry_run_with_chat() {
        let v = list_dry_run_payload("work");
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["chat"], serde_json::json!("work"));
    }

    #[test]
    fn parse_topic_id_accepts_positive_integer() {
        assert_eq!(parse_topic_id(" 42 ").unwrap(), 42);
    }

    #[test]
    fn parse_topic_id_rejects_empty_and_non_numeric() {
        for bad in ["", "  ", "abc", "12x", "9223372036854775808"] {
            let err = parse_topic_id(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert_eq!(err.exit_code(), EXIT_USAGE);
        }
    }

    #[test]
    fn parse_topic_id_rejects_zero_and_negative() {
        for bad in ["0", "-1"] {
            let err = parse_topic_id(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert!(err.message().contains("positive"), "for {bad}");
        }
    }

    #[test]
    fn edit_requires_title_or_closed() {
        let err = validate_edit_changes(None, None).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("nothing to change"));
    }

    #[test]
    fn edit_rejects_blank_title_but_accepts_valid_combos() {
        assert!(matches!(
            validate_edit_changes(Some("   "), None),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_edit_changes(Some("new"), None).is_ok());
        assert!(validate_edit_changes(None, Some(true)).is_ok());
        assert!(validate_edit_changes(Some("new"), Some(false)).is_ok());
    }

    fn empty_peer() -> tl::enums::InputPeer {
        tl::enums::InputPeer::Empty
    }

    #[test]
    fn edit_request_maps_title_and_closed_only() {
        let req = build_edit_request(empty_peer(), 7, Some("renamed"), Some(false));
        assert_eq!(req.peer, empty_peer());
        assert_eq!(req.topic_id, 7);
        assert_eq!(req.title.as_deref(), Some("renamed"));
        assert_eq!(req.closed, Some(false));
        assert_eq!(req.icon_emoji_id, None);
        assert_eq!(req.hidden, None);
    }

    #[test]
    fn close_and_reopen_shape_only_the_closed_flag() {
        let close_req = build_edit_request(empty_peer(), 9, None, Some(true));
        assert_eq!(close_req.topic_id, 9);
        assert_eq!(close_req.title, None);
        assert_eq!(close_req.closed, Some(true));
        assert_eq!(close_req.hidden, None);

        let reopen_req = build_edit_request(empty_peer(), 9, None, Some(false));
        assert_eq!(reopen_req.title, None);
        assert_eq!(reopen_req.closed, Some(false));
    }

    #[test]
    fn action_kind_names_match_subcommands() {
        assert_eq!(ActionKind::Close.name(), "close");
        assert_eq!(ActionKind::Reopen.name(), "reopen");
        assert_eq!(ActionKind::Delete.name(), "delete");
        assert_eq!(ActionKind::Pin.name(), "pin");
    }

    #[test]
    fn lifecycle_dry_run_names_the_action() {
        let v = lifecycle_dry_run_payload("work", 5, "close");
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["chat"], serde_json::json!("work"));
        assert_eq!(v["topic"], serde_json::json!(5));
        assert_eq!(v["would"], serde_json::json!("close topic 5 in chat work"));
    }

    #[test]
    fn edit_dry_run_and_report_carry_requested_changes_only() {
        let v = edit_dry_run_payload("work", 5, None, None);
        assert_eq!(v["would"], serde_json::json!("edit topic 5 in chat work"));
        assert!(v.get("title").is_none());
        assert!(v.get("closed").is_none());

        let v = edit_dry_run_payload("work", 5, Some("t"), Some(true));
        assert_eq!(v["title"], serde_json::json!("t"));
        assert_eq!(v["closed"], serde_json::json!(true));

        let v = edit_report("work", 5, None, Some(false));
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["topic"], serde_json::json!(5));
        assert_eq!(v["closed"], serde_json::json!(false));
        assert!(v.get("title").is_none());

        let v = edit_report("work", 6, Some("x"), None);
        assert_eq!(v["title"], serde_json::json!("x"));
        assert!(v.get("closed").is_none());
    }

    fn topic_with_flags(id: i32, closed: bool, pinned: bool) -> tl::enums::ForumTopic {
        match fake_topic(id) {
            tl::enums::ForumTopic::Topic(mut t) => {
                t.closed = closed;
                t.pinned = pinned;
                tl::enums::ForumTopic::Topic(t)
            }
            other => other,
        }
    }

    #[test]
    fn topic_row_adds_closed_and_pinned_fields() {
        let row = topic_row(&topic_with_flags(3, true, true)).unwrap();
        assert_eq!(row["id"], serde_json::json!(3));
        assert_eq!(row["closed"], serde_json::json!(true));
        assert_eq!(row["pinned"], serde_json::json!(true));

        let row = topic_row(&fake_topic(4)).unwrap();
        assert_eq!(row["closed"], serde_json::json!(false));
        assert_eq!(row["pinned"], serde_json::json!(false));

        assert!(topic_row(&tl::enums::ForumTopic::Deleted(
            tl::types::ForumTopicDeleted { id: 5 }
        ))
        .is_none());
    }

    #[test]
    fn emoji_absent_means_no_icon() {
        assert_eq!(validate_emoji(None).unwrap(), None);
    }

    #[test]
    fn single_codepoint_emoji_packs_four_bytes() {
        let grin = validate_emoji(Some("😀")).unwrap().unwrap();
        assert_eq!(
            grin,
            i64::from_be_bytes([0, 0, 0, 0, 0xF0, 0x9F, 0x98, 0x80])
        );
        let thumb = validate_emoji(Some("👍")).unwrap().unwrap();
        assert_eq!(
            thumb,
            i64::from_be_bytes([0, 0, 0, 0, 0xF0, 0x9F, 0x91, 0x8D])
        );
    }

    #[test]
    fn empty_emoji_is_a_usage_error() {
        let err = validate_emoji(Some("")).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err
            .message()
            .contains("custom-emoji document IDs are not supported"));
    }

    #[test]
    fn multi_codepoint_and_oversized_emoji_rejected() {
        for bad in ["👨👩👧", "🇺🇸", "1️⃣", "ab", "test", "©"] {
            let err = validate_emoji(Some(bad)).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert!(
                err.message()
                    .contains("custom-emoji document IDs are not supported"),
                "for {bad}"
            );
        }
    }

    #[test]
    fn non_emoji_single_codepoint_rejected() {
        let err = validate_emoji(Some("\u{20000}")).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("not a recognized emoji"));
    }

    #[test]
    fn three_byte_emoji_rejected() {
        for bad in ["☕", "❤"] {
            let err = validate_emoji(Some(bad)).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert!(
                err.message()
                    .contains("custom-emoji document IDs are not supported"),
                "for {bad}"
            );
        }
    }

    fn fake_topic(id: i32) -> tl::enums::ForumTopic {
        fake_topic_at(id, id, id * 10)
    }

    fn fake_topic_at(id: i32, date: i32, top_message: i32) -> tl::enums::ForumTopic {
        tl::enums::ForumTopic::Topic(tl::types::ForumTopic {
            my: false,
            closed: false,
            pinned: false,
            short: false,
            hidden: false,
            title_missing: false,
            id,
            date,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 0 }),
            title: String::new(),
            icon_color: 0,
            icon_emoji_id: None,
            top_message,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            unread_count: 0,
            unread_mentions_count: 0,
            unread_reactions_count: 0,
            unread_poll_votes_count: 0,
            from_id: tl::enums::Peer::User(tl::types::PeerUser { user_id: 0 }),
            notify_settings: tl::enums::PeerNotifySettings::Settings(
                tl::types::PeerNotifySettings {
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
                },
            ),
            draft: None,
        })
    }

    #[tokio::test]
    async fn collect_forum_topics_stops_on_empty_page() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(10, |cursor, page_limit| {
            calls.push((cursor, page_limit));
            async move { Ok(ForumTopicsPage { topics: Vec::new() }) }
        })
        .await
        .unwrap();
        assert!(topics.is_empty());
        assert_eq!(calls, vec![(ForumCursor::default(), 10)]);
    }

    #[tokio::test]
    async fn collect_forum_topics_probes_after_partial_page() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(5, |cursor, page_limit| {
            let page_index = calls.len();
            calls.push((cursor, page_limit));
            async move {
                if page_index == 0 {
                    Ok(ForumTopicsPage {
                        topics: vec![fake_topic(10), fake_topic(9)],
                    })
                } else {
                    Ok(ForumTopicsPage { topics: Vec::new() })
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(topics.len(), 2);
        assert_eq!(
            calls,
            vec![
                (ForumCursor::default(), 5),
                (
                    ForumCursor {
                        date: 9,
                        message_id: 90,
                        topic_id: 9
                    },
                    3
                ),
            ]
        );
    }

    #[tokio::test]
    async fn collect_forum_topics_paginates_until_limit() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(5, |cursor, page_limit| {
            let page_index = calls.len();
            calls.push((cursor, page_limit));
            async move {
                if page_index == 0 {
                    Ok(ForumTopicsPage {
                        topics: vec![fake_topic(10), fake_topic(9), fake_topic(8)],
                    })
                } else {
                    Ok(ForumTopicsPage {
                        topics: vec![fake_topic(7), fake_topic(6)],
                    })
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(topics.len(), 5);
        assert_eq!(
            calls,
            vec![
                (ForumCursor::default(), 5),
                (
                    ForumCursor {
                        date: 8,
                        message_id: 80,
                        topic_id: 8
                    },
                    2
                ),
            ]
        );
    }

    #[tokio::test]
    async fn collect_forum_topics_stops_when_limit_reached_exactly() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(3, |cursor, page_limit| {
            calls.push((cursor, page_limit));
            async move {
                Ok(ForumTopicsPage {
                    topics: vec![fake_topic(7), fake_topic(6), fake_topic(5)],
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(topics.len(), 3);
        assert_eq!(calls, vec![(ForumCursor::default(), 3)]);
    }

    #[tokio::test]
    async fn collect_forum_topics_page_size_capped_at_100() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(250, |cursor, page_limit| {
            let page_index = calls.len();
            calls.push((cursor, page_limit));
            async move {
                let ids: Vec<i32> = (0..page_limit)
                    .map(|i| 1000 - page_index as i32 * 100 - i as i32)
                    .collect();
                Ok(ForumTopicsPage {
                    topics: ids.into_iter().map(fake_topic).collect(),
                })
            }
        })
        .await
        .unwrap();
        assert_eq!(topics.len(), 250);
        assert_eq!(
            calls,
            vec![
                (ForumCursor::default(), 100),
                (
                    ForumCursor {
                        date: 901,
                        message_id: 9010,
                        topic_id: 901
                    },
                    100
                ),
                (
                    ForumCursor {
                        date: 801,
                        message_id: 8010,
                        topic_id: 801
                    },
                    50
                ),
            ]
        );
    }

    #[tokio::test]
    async fn collect_forum_topics_next_offset_uses_last_topic_even_if_deleted() {
        let mut calls = Vec::new();
        let topics = collect_forum_topics(5, |cursor, page_limit| {
            let page_index = calls.len();
            calls.push((cursor, page_limit));
            async move {
                if page_index == 0 {
                    Ok(ForumTopicsPage {
                        topics: vec![
                            fake_topic(10),
                            tl::enums::ForumTopic::Deleted(tl::types::ForumTopicDeleted { id: 9 }),
                        ],
                    })
                } else {
                    Ok(ForumTopicsPage { topics: Vec::new() })
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(topics.len(), 2);
        assert_eq!(
            calls,
            vec![
                (ForumCursor::default(), 5),
                (
                    ForumCursor {
                        date: 10,
                        message_id: 100,
                        topic_id: 10
                    },
                    4
                ),
            ]
        );
    }

    #[test]
    fn serve_schemas_are_real_objects() {
        for schema in [
            crate::commands::serve::params_schema::<CreateParams>(),
            crate::commands::serve::params_schema::<LifecycleParams>(),
            crate::commands::serve::params_schema::<EditParams>(),
        ] {
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], serde_json::json!(false));
            assert!(schema["properties"].as_object().is_some());
        }
        let lifecycle = crate::commands::serve::params_schema::<LifecycleParams>();
        assert!(lifecycle["properties"]["topic"].is_object());
    }
}
