use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::require_chat_target;
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

#[derive(Args)]
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

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, default_value_t = 20, help = "max topics to list (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct LifecycleArgs {
    #[arg(long, help = "forum group: @username, numeric ID, +phone, or me")]
    chat: String,
    #[arg(long, help = "topic id (root message id; see tele topic list)")]
    topic: String,
}

#[derive(Args)]
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
    let icon_emoji_id = validate_emoji(args.emoji.as_deref())?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let title = args.title.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "title": title,
                    "would": format!("create topic \"{title}\" in chat {target}")
                }));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let _: tl::enums::Updates = guard
                .client
                .invoke(&tl::functions::messages::CreateForumTopic {
                    title_missing: false,
                    peer,
                    title: title.clone(),
                    icon_color: None,
                    icon_emoji_id,
                    random_id: rand_seed(),
                    send_as: None,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "title": title,
                "ok": true,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn simple_action(
    args: LifecycleArgs,
    flags: &GlobalFlags,
    kind: ActionKind,
) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let topic_id = parse_topic_id(&args.topic)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let chat_target = args.chat.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(lifecycle_dry_run_payload(
                    &chat_target,
                    topic_id,
                    kind.name(),
                ));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            match kind {
                ActionKind::Close | ActionKind::Reopen => {
                    let _: tl::enums::Updates = guard
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
                    let _: tl::enums::messages::AffectedHistory = guard
                        .client
                        .invoke(&tl::functions::messages::DeleteTopicHistory {
                            peer,
                            top_msg_id: topic_id,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
                ActionKind::Pin => {
                    let _: tl::enums::Updates = guard
                        .client
                        .invoke(&tl::functions::messages::UpdatePinnedForumTopic {
                            peer,
                            topic_id,
                            pinned: true,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
            }
            Ok(serde_json::json!({
                "chat": chat_target,
                "topic": topic_id,
                "ok": true,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    require_chat_target(&args.chat, "chat")?;
    let topic_id = parse_topic_id(&args.topic)?;
    validate_edit(args.title.as_deref(), args.closed)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let chat_target = args.chat.clone();
    let title = args.title.clone();
    let closed = args.closed;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        let title = title.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(edit_dry_run_payload(
                    &chat_target,
                    topic_id,
                    title.as_deref(),
                    closed,
                ));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &chat_target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let request = build_edit_request(peer, topic_id, title.as_deref(), closed);
            let _: tl::enums::Updates = guard
                .client
                .invoke(&request)
                .await
                .map_err(tele_invocation)?;
            Ok(edit_report(
                &chat_target,
                topic_id,
                title.as_deref(),
                closed,
            ))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_payload(&target));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let topics = {
                let guard_ref = &guard;
                let peer_ref = &peer;
                collect_forum_topics(limit, move |cursor, page_limit| async move {
                    let results: tl::enums::messages::ForumTopics = guard_ref
                        .client
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
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows
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
            Ok(serde_json::json!({"topics": rows}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
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
    let Some(last) = page.last() else {
        return prev;
    };
    match last {
        tl::enums::ForumTopic::Topic(t) => ForumCursor {
            date: t.date,
            message_id: t.top_message,
            topic_id: t.id,
        },
        tl::enums::ForumTopic::Deleted(t) => ForumCursor {
            date: prev.date,
            message_id: prev.message_id,
            topic_id: t.id,
        },
    }
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
        let remaining = limit.saturating_sub(topics.len() as u32);
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

fn validate_edit(title: Option<&str>, closed: Option<bool>) -> TeleResult<()> {
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
            "pinned": t.pinned,
        })),
        tl::enums::ForumTopic::Deleted(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;

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
        let err = validate_edit(None, None).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("nothing to change"));
    }

    #[test]
    fn edit_rejects_blank_title_but_accepts_valid_combos() {
        assert!(matches!(
            validate_edit(Some("   "), None),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_edit(Some("new"), None).is_ok());
        assert!(validate_edit(None, Some(true)).is_ok());
        assert!(validate_edit(Some("new"), Some(false)).is_ok());
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
                        topic_id: 9,
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
                        topic_id: 8,
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
                        topic_id: 901,
                    },
                    100
                ),
                (
                    ForumCursor {
                        date: 801,
                        message_id: 8010,
                        topic_id: 801,
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
                        date: 0,
                        message_id: 0,
                        topic_id: 9,
                    },
                    3
                ),
            ]
        );
    }
}
