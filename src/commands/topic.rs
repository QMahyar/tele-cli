use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum TopicCmd {
    Create(CreateArgs),
    List(ListArgs),
}

#[derive(Args)]
pub struct CreateArgs {
    #[arg(long, help = "forum group: @username, numeric ID, or me")]
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
    #[arg(long, help = "forum group: @username, numeric ID, or me")]
    chat: String,
    #[arg(long, default_value_t = 20, help = "max topics to list (1-10000)")]
    limit: u32,
}

pub async fn run(cmd: TopicCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        TopicCmd::Create(a) => create(a, flags).await,
        TopicCmd::List(a) => list(a, flags).await,
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

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit;
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
                match topic {
                    tl::enums::ForumTopic::Topic(t) => rows.push(serde_json::json!({
                        "id": t.id,
                        "title": t.title,
                        "icon_emoji_id": t.icon_emoji_id,
                    })),
                    tl::enums::ForumTopic::Deleted(_) => {}
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
                        ]
                    })
                    .collect();
                output::print_table(&["id", "title", "icon_emoji_id"], &table_rows);
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
    let bytes = emoji.as_bytes();
    if bytes.is_empty() {
        return Err(TeleError::Usage(
            "--emoji cannot be empty; only a single-codepoint emoji (4 UTF-8 bytes) is accepted; custom-emoji document IDs are not supported"
                .to_string(),
        ));
    }
    let codepoints = emoji.chars().count();
    if bytes.len() != 4 || codepoints != 1 {
        return Err(TeleError::Usage(format!(
            "--emoji \"{emoji}\" must be a single-codepoint emoji (4 UTF-8 bytes); custom-emoji document IDs are not supported (got {} bytes, {codepoints} codepoints)",
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
