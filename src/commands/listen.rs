use crate::client::{self, ClientGuard};
use crate::error::{TeleError, TeleResult};
use crate::executor::GlobalFlags;
use crate::output;
use clap::Args;
#[derive(Args)]
pub struct ListenArgs {
    #[arg(long, default_value_t = 0)]
    timeout_secs: u64,
    #[arg(long, value_delimiter = ',', default_value = "NewMessage")]
    events: Vec<String>,
    #[arg(long)]
    raw: bool,
    #[arg(long)]
    chat: Option<String>,
    #[arg(long)]
    quiet: bool,
}
const VALID_EVENTS: &[&str] = &["NewMessage", "MessageEdited", "MessageDeleted", "Raw"];
pub async fn run(args: &ListenArgs, flags: &GlobalFlags) -> TeleResult<()> {
    let config_path = flags.config_path.clone();
    use grammers_client::update::Update;
    let mut events: Vec<String> = args.events.clone();
    events.retain(|e| VALID_EVENTS.contains(&e.as_str()));
    if events.len() != args.events.len() {
        return Err(TeleError::Usage(format!(
            "unknown event name in --events (valid: {VALID_EVENTS:?})"
        )));
    }
    if !flags.json && !flags.jsonl {
        output::log_line("info", "listen streams JSONL events on stdout");
    }
    let names = crate::executor::select_accounts(flags)?;
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
    let timeout_secs = args.timeout_secs;
    let raw = args.raw;
    let chat_filter = args.chat.clone();
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let config_path = config_path.clone();
        let chat_filter = chat_filter.clone();
        let events = events.clone();
        tasks.spawn(async move {            let result: TeleResult<()> = async {                let creds = crate::config::credentials()                    .map_err(|e| TeleError::Config(e.to_string()))?;                let mut guard = ClientGuard::connect(&name, creds.api_id, config_path.as_deref()).await?;                client::authorize(&guard.client, &creds).await?;                let receiver = std::mem::replace(                    &mut guard.updates,                    tokio::sync::mpsc::unbounded_channel().1,                );                let mut stream = guard                    .client                    .stream_updates(                        receiver,                        grammers_client::client::UpdatesConfiguration::default(),                    )                    .await                    .map_err(|e| TeleError::Other(e.to_string()))?;                let deadline = if timeout_secs > 0 {                    Some(                        std::time::Instant::now()                            + std::time::Duration::from_secs(timeout_secs),                    )                } else {                    None                };                loop {                    if let Some(d) = deadline {                        if std::time::Instant::now() >= d {                            break;                        }                    }                    let update = match tokio::time::timeout(                        std::time::Duration::from_secs(3600),                        stream.next(),                    )                    .await                    {                        Ok(Ok(u)) => u,                        Ok(Err(e)) => {                            if crate::error::invocation_is_unauthorized(&e) {                                output::log_line(                                    "error",                                    &format!("{name}: not authorized, stopping stream"),                                );                            }                            break;                        }                        Err(_) => continue,                    };                    if raw {                        let raw_debug = match &update {                            Update::Raw(r) => format!("{:?}", r.raw),                            _ => "not raw".to_string(),                        };                        println!(                            "{}",                            serde_json::json!({                                "account": name,                                "type": "raw",                                "update": raw_debug,                            })                        );                        continue;                    }                    match update {                        Update::NewMessage(m) => {                            if events.iter().any(|e| e == "NewMessage") {                            if let Some(filter) = &chat_filter {                                let chat_name = m                                    .peer()                                    .map(crate::serialize::peer_name)                                    .unwrap_or_default();                                if !chat_name.contains(filter)                                    && !m.peer_id().to_string().contains(filter)                                {                                    continue;                                }                            }                            let mut row = crate::serialize::message_to_json(&m)?;                            row.as_object_mut()                                .unwrap()                                .insert("account".into(), serde_json::Value::from(name.clone()));                            row.as_object_mut()                                .unwrap()                                .insert("type".into(), serde_json::Value::from("new_message"));                            output::print_json(&row);                        }                        }Update::MessageEdited(m) => {                            if !events.iter().any(|e| e == "MessageEdited") {                                continue;                            }                            let mut row = crate::serialize::message_to_json(&m)?;                            row.as_object_mut()                                .unwrap()                                .insert("account".into(), serde_json::Value::from(name.clone()));                            row.as_object_mut()                                .unwrap()                                .insert("type".into(), serde_json::Value::from("message_edited"));                            output::print_json(&row);                        }                        Update::MessageDeleted(d) => {                            if !events.iter().any(|e| e == "MessageDeleted") {                                continue;                            }                            println!(                                "{}",                                serde_json::json!({                                    "account": name,                                    "type": "message_deleted",                                    "chat_id": d.channel_id().map(|c| c.to_string()).unwrap_or_default(),                                    "ids": d.messages(),                                })                            );                        }                        other => {                            if !events.iter().any(|e| e == "Raw") {                                continue;                            }                            println!(                                "{}",                                serde_json::json!({                                    "account": name,                                    "type": "update",                                    "kind": format!("{other:?}"),                                })                            );                        }                    }                }                Ok(())            }            .await;            if let Err(e) = result {                output::log_line("error", &format!("{name}: {}", e.message()));            }            Ok::<(), ()>(())        });
    }
    while tasks.join_next().await.is_some() {}
    if timeout_secs > 0 {
        output::log_line("info", "listen timeout reached");
    }
    Ok(())
}
