use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::TeleResult;
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
    #[arg(long, help = "4-byte emoji for topic icon (optional)")]
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
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let emoji = args.emoji.clone();
        let title = args.title.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true, "chat": target, "title": title}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let icon_emoji_id = emoji.and_then(|e| emoji_to_icon_id(&e));
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
    let limit = args.limit as i32;
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
            let chat = entities::resolve_peer(&guard.client, guard.session.as_ref(), &target)
                .await
                .map_err(tele_invocation)?;
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            let results: tl::enums::messages::ForumTopics = guard
                .client
                .invoke(&tl::functions::messages::GetForumTopics {
                    peer,
                    q: None,
                    offset_date: 0,
                    offset_id: 0,
                    offset_topic: 0,
                    limit,
                })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::messages::ForumTopics::Topics(topics) = results;
            let mut rows = Vec::new();
            for topic in topics.topics {
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

fn emoji_to_icon_id(emoji: &str) -> Option<i64> {
    let bytes = emoji.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(i64::from_be_bytes([
        0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3],
    ]))
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
    serde_json::json!({"dry_run": true, "chat": target})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_dry_run_payload_marks_dry_run_with_chat() {
        let v = list_dry_run_payload("work");
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["chat"], serde_json::json!("work"));
    }
}
