use std::io::Write;

use crate::client::{self, ClientGuard};
use crate::error::{TeleError, TeleResult};
use crate::executor::GlobalFlags;
use crate::output;
use clap::Args;
use grammers_client::tl::{self, Serializable};
use grammers_session::types::{PeerId, PeerKind};
use grammers_session::updates::State;
#[derive(Args)]
pub struct ListenArgs {
    #[arg(
        long = "timeout-secs",
        default_value_t = 0,
        help = "max listen duration in seconds (0 = forever)"
    )]
    timeout_secs: u64,
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "NewMessage",
        help = "event types: NewMessage, MessageEdited, MessageDeleted, Raw"
    )]
    events: Vec<String>,
    #[arg(long, help = "output raw TL updates instead of parsed events")]
    raw: bool,
    #[arg(long, help = "only show events from this chat")]
    chat: Option<String>,
}
const VALID_EVENTS: &[&str] = &["NewMessage", "MessageEdited", "MessageDeleted", "Raw"];
const MAX_RECONNECT_BACKOFF: u32 = 30;
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
pub async fn run(args: &ListenArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    use grammers_client::update::Update;
    let mut events: Vec<String> = args.events.clone();
    events.retain(|e| VALID_EVENTS.contains(&e.as_str()));
    if events.len() != args.events.len() {
        return Err(TeleError::Usage(format!(
            "unknown event name in --events (valid: {VALID_EVENTS:?})"
        )));
    }
    if args.raw && !events.iter().any(|e| e == "Raw") {
        events.push("Raw".to_string());
    }
    let names = crate::executor::select_accounts(flags)?;
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
    if flags.dry_run {
        output::log_line("info", "[dry-run] would stream updates");
        if flags.json || flags.jsonl {
            for name in &names {
                output::print_json_result(&dry_run_row(&events, name))?;
            }
        }
        return Ok(crate::error::EXIT_OK);
    }
    if !flags.json && !flags.jsonl {
        output::log_line("info", "listen streams JSONL events on stdout");
    }
    let timeout_secs = args.timeout_secs;
    let chat_filter = args.chat.clone();
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let parallel = effective_parallel(flags.parallel, cfg.parallel_max) as usize;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(parallel));
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let config_path = config_path.clone();
        let chat_filter = chat_filter.clone();
        let events = events.clone();
        let semaphore = std::sync::Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| TeleError::Other(e.to_string()))?;
            let result: TeleResult<()> = async {
                let creds =
                    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))?;
                let deadline = if timeout_secs > 0 {
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs))
                } else {
                    None
                };
                let mut resolved: Option<PeerId> = None;
                let mut failures: u32 = 0;
                loop {
                    if let Some(d) = deadline {
                        if std::time::Instant::now() >= d {
                            break;
                        }
                    }
                    let mut guard =
                        match ClientGuard::connect(&name, creds.api_id, config_path.as_deref())
                            .await
                        {
                            Ok(guard) => guard,
                            Err(e) => {
                                handle_stream_failure(
                                    &name,
                                    TeleError::from(e),
                                    &mut failures,
                                    deadline,
                                )
                                .await?;
                                continue;
                            }
                        };
                    if let Err(e) = client::authorize(&guard.client).await {
                        handle_stream_failure(&name, e, &mut failures, deadline).await?;
                        continue;
                    }
                    if resolved.is_none() {
                        resolved = match &chat_filter {
                            Some(target) => match crate::entities::resolve_peer(
                                &guard.client,
                                &guard.session,
                                target,
                            )
                            .await
                            {
                                Ok(peer) => Some(peer.id()),
                                Err(e) => {
                                    handle_stream_failure(
                                        &name,
                                        TeleError::Other(format!(
                                            "cannot resolve chat {target:?}: {e}"
                                        )),
                                        &mut failures,
                                        deadline,
                                    )
                                    .await?;
                                    continue;
                                }
                            },
                            None => None,
                        };
                    }
                    if let Err(e) = guard
                        .client
                        .invoke(&tl::functions::updates::GetState {})
                        .await
                    {
                        let err = getstate_probe_error(e);
                        if is_auth_error(&err) {
                            output::log_line(
                                "error",
                                &format!("{name}: not authorized, stopping stream"),
                            );
                            return Err(err);
                        }
                        handle_stream_failure(&name, err, &mut failures, deadline).await?;
                        continue;
                    }
                    let receiver = std::mem::replace(
                        &mut guard.updates,
                        tokio::sync::mpsc::unbounded_channel().1,
                    );
                    let mut stream = match guard
                        .client
                        .stream_updates(
                            receiver,
                            grammers_client::client::UpdatesConfiguration {
                                catch_up: true,
                                update_queue_limit: Some(1000),
                            },
                        )
                        .await
                    {
                        Ok(stream) => stream,
                        Err(e) => {
                            handle_stream_failure(
                                &name,
                                TeleError::Other(e.to_string()),
                                &mut failures,
                                deadline,
                            )
                            .await?;
                            continue;
                        }
                    };
                    failures = on_reconnect_success(failures);
                    loop {
                        if let Some(d) = deadline {
                            if std::time::Instant::now() >= d {
                                break;
                            }
                        }
                        let update = match tokio::time::timeout(
                            poll_timeout(deadline, std::time::Instant::now()),
                            stream.next(),
                        )
                        .await
                        {
                            Ok(Ok(u)) => u,
                            Ok(Err(e)) => {
                                if crate::error::invocation_is_unauthorized(&e) {
                                    output::log_line(
                                        "error",
                                        &format!("{name}: not authorized, stopping stream"),
                                    );
                                    return Err(crate::error::invocation_error(e));
                                }
                                handle_stream_failure(
                                    &name,
                                    crate::error::invocation_error(e),
                                    &mut failures,
                                    deadline,
                                )
                                .await?;
                                break;
                            }
                            Err(_) => continue,
                        };
                        failures = on_reconnect_success(failures);
                        match &update {
                            Update::NewMessage(m) => {
                                if !events.iter().any(|e| e == "NewMessage") {
                                    continue;
                                }
                                let peer = update_peer(&m.raw);
                                if !raw_should_stream(peer, resolved) {
                                    continue;
                                }
                                if is_empty_update(&m.raw) {
                                    continue;
                                }
                                let row = crate::serialize::message_to_json(m)?;
                                emit_row(event_row(
                                    "NewMessage",
                                    &name,
                                    peer.and_then(|p| p.bare_id()),
                                    None,
                                    Some(row),
                                ))
                                .await?;
                            }
                            Update::MessageEdited(m) => {
                                if !events.iter().any(|e| e == "MessageEdited") {
                                    continue;
                                }
                                let peer = update_peer(&m.raw);
                                if !raw_should_stream(peer, resolved) {
                                    continue;
                                }
                                if is_empty_update(&m.raw) {
                                    continue;
                                }
                                let row = crate::serialize::message_to_json(m)?;
                                emit_row(event_row(
                                    "MessageEdited",
                                    &name,
                                    peer.and_then(|p| p.bare_id()),
                                    None,
                                    Some(row),
                                ))
                                .await?;
                            }
                            Update::MessageDeleted(d) => {
                                if !events.iter().any(|e| e == "MessageDeleted") {
                                    continue;
                                }
                                if let Some(target) = &resolved {
                                    if !deleted_matches(d.channel_id(), *target) {
                                        continue;
                                    }
                                }
                                emit_row(event_row(
                                    "MessageDeleted",
                                    &name,
                                    d.channel_id(),
                                    Some(d.messages()),
                                    None,
                                ))
                                .await?;
                            }
                            _ => {
                                if !events.iter().any(|e| e == "Raw") {
                                    continue;
                                }
                                if !raw_should_stream(update_peer(update.raw()), resolved) {
                                    continue;
                                }
                                emit_row(raw_row(&name, update.raw(), update.state())).await?;
                            }
                        }
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = &result {
                output::log_line("error", &format!("{name}: {}", e.message()));
            }
            result
        });
    }
    let mut ok_count = 0usize;
    let mut failed: Vec<i32> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => ok_count += 1,
            Ok(Err(e)) => failed.push(e.exit_code()),
            Err(_) => failed.push(crate::error::EXIT_ALL_FAILED),
        }
    }
    if timeout_secs > 0 {
        output::log_line("info", "listen timeout reached");
    }
    Ok(aggregate_exit(ok_count, &failed))
}

async fn emit_row(value: serde_json::Value) -> TeleResult<()> {
    let line = serde_json::to_string(&value)?;
    tokio::task::spawn_blocking(move || {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")?;
        out.flush()
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    Ok(())
}

fn aggregate_exit(ok_count: usize, failed: &[i32]) -> i32 {
    if failed.is_empty() {
        crate::error::EXIT_OK
    } else if ok_count > 0 {
        crate::error::EXIT_PARTIAL
    } else if failed.iter().all(|c| *c == crate::error::EXIT_USAGE) {
        crate::error::EXIT_USAGE
    } else if failed.iter().all(|c| *c == crate::error::EXIT_AUTH) {
        crate::error::EXIT_AUTH
    } else {
        crate::error::EXIT_ALL_FAILED
    }
}

async fn handle_stream_failure(
    account: &str,
    err: TeleError,
    failures: &mut u32,
    deadline: Option<std::time::Instant>,
) -> TeleResult<()> {
    if is_auth_error(&err) {
        return Err(err);
    }
    *failures = on_failure(*failures);
    if give_up(*failures) {
        return Err(TeleError::Other(format!(
            "{account}: updates stream failed {failures} consecutive times, giving up: {}",
            err.message()
        )));
    }
    let delay = next_delay(*failures);
    let sleep_for = match deadline {
        Some(d) => delay.min(d.saturating_duration_since(std::time::Instant::now())),
        None => delay,
    };
    output::log_line(
        "error",
        &reconnect_message(account, *failures, delay.as_secs() as u32, err.message()),
    );
    tokio::time::sleep(sleep_for).await;
    Ok(())
}

fn is_auth_error(e: &TeleError) -> bool {
    matches!(e, TeleError::Auth(_))
}

fn getstate_probe_error(e: grammers_client::InvocationError) -> TeleError {
    if crate::error::invocation_is_unauthorized(&e) {
        crate::error::invocation_error(e)
    } else {
        TeleError::Other(format!("initial GetState failed: {e}"))
    }
}

fn next_delay(attempt: u32) -> std::time::Duration {
    let secs = if attempt == 0 {
        0
    } else {
        (1u32 << (attempt - 1).min(5)).min(MAX_RECONNECT_BACKOFF)
    };
    std::time::Duration::from_secs(secs.into())
}

fn update_peer(u: &tl::enums::Update) -> Option<PeerId> {
    match u {
        tl::enums::Update::NewMessage(x) => message_peer(&x.message),
        tl::enums::Update::NewChannelMessage(x) => message_peer(&x.message),
        tl::enums::Update::EditMessage(x) => message_peer(&x.message),
        tl::enums::Update::EditChannelMessage(x) => message_peer(&x.message),
        tl::enums::Update::DeleteChannelMessages(x) => PeerId::channel(x.channel_id),
        _ => None,
    }
}

fn message_peer(msg: &tl::enums::Message) -> Option<PeerId> {
    match msg {
        tl::enums::Message::Message(m) => Some(PeerId::from(&m.peer_id)),
        tl::enums::Message::Service(m) => Some(PeerId::from(&m.peer_id)),
        tl::enums::Message::Empty(_) => None,
    }
}

fn is_empty_message(msg: &tl::enums::Message) -> bool {
    matches!(msg, tl::enums::Message::Empty(_))
}

fn is_empty_update(u: &tl::enums::Update) -> bool {
    match u {
        tl::enums::Update::NewMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::NewChannelMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::EditMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::EditChannelMessage(x) => is_empty_message(&x.message),
        _ => false,
    }
}

fn raw_should_stream(peer: Option<PeerId>, chat: Option<PeerId>) -> bool {
    match chat {
        None => true,
        Some(target) => peer == Some(target),
    }
}

fn deleted_matches(bare_id: Option<i64>, resolved: PeerId) -> bool {
    if resolved.kind() == PeerKind::User {
        return false;
    }
    match (bare_id, resolved.bare_id()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn effective_parallel(flag: Option<u32>, cfg_max: u32) -> u32 {
    flag.unwrap_or(cfg_max).clamp(1, 3)
}

fn reconnect_allowed(consecutive_failures: u32) -> bool {
    consecutive_failures <= MAX_RECONNECT_ATTEMPTS
}

fn on_failure(failures: u32) -> u32 {
    failures + 1
}

fn on_reconnect_success(failures: u32) -> u32 {
    let _ = failures;
    0
}

fn give_up(failures: u32) -> bool {
    !reconnect_allowed(failures)
}

fn reconnect_message(account: &str, failures: u32, backoff: u32, cause: &str) -> String {
    format!(
        "{account}: updates stream error ({cause}), reconnecting (attempt {failures}/{MAX_RECONNECT_ATTEMPTS}) in {backoff}s"
    )
}

fn poll_timeout(
    deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> std::time::Duration {
    match deadline {
        Some(d) => std::time::Duration::from_secs(3600).min(d.saturating_duration_since(now)),
        None => std::time::Duration::from_secs(3600),
    }
}

fn dry_run_row(events: &[String], account: &str) -> serde_json::Value {
    let label = events.join(",");
    let would = format!("stream {label} updates from account {account}");
    event_row(
        &label,
        account,
        None,
        None,
        Some(serde_json::json!({
            "dry_run": true,
            "would": would,
        })),
    )
}

fn event_row(
    event: &str,
    account: &str,
    chat_id: Option<i64>,
    ids: Option<&[i32]>,
    message: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut row = match message {
        Some(value) => value.as_object().cloned().unwrap_or_default(),
        None => serde_json::Map::new(),
    };
    row.insert("event".into(), serde_json::Value::from(event));
    row.insert("account".into(), serde_json::Value::from(account));
    if let Some(chat_id) = chat_id {
        row.insert("chat_id".into(), serde_json::json!(chat_id));
    }
    if let Some(ids) = ids {
        row.insert("ids".into(), serde_json::json!(ids));
    }
    serde_json::Value::Object(row)
}

fn raw_row(account: &str, raw: &tl::enums::Update, state: &State) -> serde_json::Value {
    let mut row = event_row("Raw", account, None, None, None);
    if let serde_json::Value::Object(map) = &mut row {
        map.insert(
            "raw".into(),
            serde_json::Value::from(base64_encode(&raw.to_bytes())),
        );
        map.insert("state".into(), state_to_json(state));
    }
    row
}

fn state_to_json(state: &State) -> serde_json::Value {
    use grammers_session::updates::MessageBox;
    let mut out = serde_json::Map::new();
    out.insert("date".into(), serde_json::json!(state.date));
    out.insert("seq".into(), serde_json::json!(state.seq));
    match &state.message_box {
        Some(MessageBox::Common { pts }) => {
            out.insert("pts".into(), serde_json::json!(pts));
        }
        Some(MessageBox::Secondary { qts }) => {
            out.insert("qts".into(), serde_json::json!(qts));
        }
        Some(MessageBox::Channel { channel_id, pts }) => {
            out.insert("channel_id".into(), serde_json::json!(channel_id));
            out.insert("pts".into(), serde_json::json!(pts));
        }
        None => {}
    }
    serde_json::Value::Object(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_message_row() -> serde_json::Value {
        serde_json::json!({
            "id": 123,
            "date": "2026-08-13T12:00:00+00:00",
            "text": "hello",
        })
    }

    #[test]
    fn dry_run_row_mirrors_event_row_with_would() {
        let row = dry_run_row(&["NewMessage".to_string()], "work");
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "NewMessage");
        assert_eq!(obj["account"], "work");
        assert_eq!(obj["dry_run"], serde_json::json!(true));
        let would = obj["would"].as_str().unwrap();
        assert!(would.contains("stream"), "would: {would}");
        assert!(would.contains("NewMessage"), "would: {would}");
        assert!(would.contains("work"), "would: {would}");
    }

    #[test]
    fn dry_run_row_lists_all_configured_events() {
        let row = dry_run_row(
            &["NewMessage".to_string(), "MessageDeleted".to_string()],
            "home",
        );
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "NewMessage,MessageDeleted");
        assert!(obj["would"]
            .as_str()
            .unwrap()
            .contains("NewMessage,MessageDeleted"));
        assert_eq!(obj["account"], "home");
    }

    #[test]
    fn message_row_matches_contract_keys() {
        let row = event_row(
            "NewMessage",
            "work",
            Some(456),
            None,
            Some(base_message_row()),
        );
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "NewMessage");
        assert_eq!(obj["account"], "work");
        assert_eq!(obj["chat_id"], 456);
        assert_eq!(obj["id"], 123);
        assert_eq!(obj["text"], "hello");
    }

    #[test]
    fn message_row_omits_chat_id_when_unknown() {
        let row = event_row(
            "MessageEdited",
            "work",
            None,
            None,
            Some(base_message_row()),
        );
        assert!(!row.as_object().unwrap().contains_key("chat_id"));
        assert_eq!(row["event"], "MessageEdited");
    }

    #[test]
    fn deleted_row_has_ids_list() {
        let row = event_row("MessageDeleted", "work", Some(456), Some(&[1, 2, 3]), None);
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "MessageDeleted");
        assert_eq!(obj["chat_id"], 456);
        assert_eq!(obj["ids"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn deleted_row_omits_chat_id_when_unknown() {
        let row = event_row("MessageDeleted", "work", None, Some(&[5]), None);
        assert!(!row.as_object().unwrap().contains_key("chat_id"));
        assert_eq!(row["ids"], serde_json::json!([5]));
    }

    #[test]
    fn raw_row_contains_no_debug_dump() {
        let row = event_row("Raw", "work", None, None, None);
        let obj = row.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj["event"], "Raw");
    }

    #[test]
    fn event_row_merges_message_and_ids() {
        let row = event_row(
            "MessageDeleted",
            "work",
            Some(456),
            Some(&[7]),
            Some(base_message_row()),
        );
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "MessageDeleted");
        assert_eq!(obj["account"], "work");
        assert_eq!(obj["chat_id"], 456);
        assert_eq!(obj["ids"], serde_json::json!([7]));
        assert_eq!(obj["id"], 123);
        assert_eq!(obj["text"], "hello");
    }

    #[test]
    fn event_row_args_override_message_keys() {
        let row = event_row(
            "NewMessage",
            "work",
            None,
            None,
            Some(serde_json::json!({ "event": "fake", "account": "other", "id": 1 })),
        );
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "NewMessage");
        assert_eq!(obj["account"], "work");
    }

    #[test]
    fn event_row_chat_id_zero_is_kept() {
        let row = event_row("Raw", "work", Some(0), None, None);
        assert_eq!(row["chat_id"], 0);
    }

    #[test]
    fn event_row_empty_ids_is_kept() {
        let row = event_row("MessageDeleted", "work", None, Some(&[]), None);
        assert!(row.as_object().unwrap().contains_key("ids"));
        assert_eq!(row["ids"], serde_json::json!([]));
    }

    use grammers_session::types::PeerId;
    use grammers_session::updates::{MessageBox, State};

    #[test]
    fn raw_row_embeds_encoded_payload_and_state() {
        use base64::Engine;
        let raw = tl::enums::Update::PtsChanged;
        let state = State {
            date: 123,
            seq: 456,
            message_box: None,
        };
        let row = raw_row("work", &raw, &state);
        let obj = row.as_object().unwrap();
        assert_eq!(obj["event"], "Raw");
        assert_eq!(obj["account"], "work");
        let encoded = obj["raw"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(decoded, raw.to_bytes());
        assert_eq!(obj["state"]["date"], 123);
        assert_eq!(obj["state"]["seq"], 456);
        assert!(obj["state"].get("pts").is_none());
        assert!(obj["state"].get("qts").is_none());
        assert!(obj["state"].get("channel_id").is_none());
    }

    #[test]
    fn raw_row_keeps_existing_event_and_account_fields() {
        let raw = tl::enums::Update::PtsChanged;
        let state = State {
            date: 1,
            seq: 2,
            message_box: None,
        };
        let row = raw_row("work", &raw, &state);
        let obj = row.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert_eq!(obj["event"], "Raw");
        assert_eq!(obj["account"], "work");
    }

    #[test]
    fn state_json_without_message_box_has_only_date_and_seq() {
        let state = State {
            date: 7,
            seq: 8,
            message_box: None,
        };
        let v = state_to_json(&state);
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj["date"], 7);
        assert_eq!(obj["seq"], 8);
    }

    #[test]
    fn state_json_common_box_has_pts() {
        let state = State {
            date: 1,
            seq: 2,
            message_box: Some(MessageBox::Common { pts: 42 }),
        };
        let v = state_to_json(&state);
        assert_eq!(v["pts"], 42);
        assert!(v.get("qts").is_none());
        assert!(v.get("channel_id").is_none());
    }

    #[test]
    fn state_json_secondary_box_has_qts() {
        let state = State {
            date: 1,
            seq: 2,
            message_box: Some(MessageBox::Secondary { qts: 43 }),
        };
        let v = state_to_json(&state);
        assert_eq!(v["qts"], 43);
        assert!(v.get("pts").is_none());
    }

    #[test]
    fn state_json_channel_box_has_channel_id_and_pts() {
        let state = State {
            date: 1,
            seq: 2,
            message_box: Some(MessageBox::Channel {
                channel_id: 9_876_543_210,
                pts: 44,
            }),
        };
        let v = state_to_json(&state);
        assert_eq!(v["channel_id"], 9_876_543_210i64);
        assert_eq!(v["pts"], 44);
        assert!(v.get("qts").is_none());
    }

    #[test]
    fn deleted_matches_bare_channel_id() {
        let resolved = PeerId::channel_unchecked(1234567890);
        assert!(deleted_matches(Some(1234567890), resolved));
        assert!(!deleted_matches(Some(999), resolved));
        assert!(!deleted_matches(None, resolved));
    }

    #[test]
    fn deleted_matches_chat_target_by_bare_id() {
        let resolved = PeerId::chat_unchecked(123);
        assert!(deleted_matches(Some(123), resolved));
        assert!(!deleted_matches(Some(999), resolved));
        assert!(!deleted_matches(None, resolved));
    }

    #[test]
    fn deleted_matches_never_matches_a_user() {
        let resolved = PeerId::user_unchecked(7);
        assert!(!deleted_matches(Some(7), resolved));
    }

    fn tl_message(peer: tl::enums::Peer) -> tl::enums::Message {
        tl::enums::Message::Message(tl::types::Message {
            out: false,
            mentioned: false,
            media_unread: false,
            silent: false,
            post: false,
            from_scheduled: false,
            legacy: false,
            edit_hide: false,
            pinned: false,
            noforwards: false,
            invert_media: false,
            offline: false,
            video_processing_pending: false,
            paid_suggested_post_stars: false,
            paid_suggested_post_ton: false,
            id: 1,
            from_id: None,
            from_boosts_applied: None,
            from_rank: None,
            peer_id: peer,
            saved_peer_id: None,
            fwd_from: None,
            via_bot_id: None,
            via_business_bot_id: None,
            guestchat_via_from: None,
            reply_to: None,
            date: 0,
            message: String::new(),
            media: None,
            reply_markup: None,
            entities: None,
            views: None,
            forwards: None,
            replies: None,
            edit_date: None,
            post_author: None,
            grouped_id: None,
            reactions: None,
            restriction_reason: None,
            ttl_period: None,
            quick_reply_shortcut_id: None,
            effect: None,
            factcheck: None,
            report_delivery_until_date: None,
            paid_message_stars: None,
            suggested_post: None,
            schedule_repeat_period: None,
            summary_from_language: None,
            rich_message: None,
        })
    }

    fn channel_peer() -> tl::enums::Peer {
        tl::enums::Peer::Channel(tl::types::PeerChannel {
            channel_id: 1234567890,
        })
    }

    #[test]
    fn update_peer_extracts_channel_from_new_message() {
        let u = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
            message: tl_message(channel_peer()),
            pts: 1,
            pts_count: 1,
        });
        assert_eq!(update_peer(&u), Some(PeerId::channel_unchecked(1234567890)));
    }

    #[test]
    fn update_peer_extracts_channel_from_edit_channel_message() {
        let u = tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
            message: tl_message(channel_peer()),
            pts: 1,
            pts_count: 1,
        });
        assert_eq!(update_peer(&u), Some(PeerId::channel_unchecked(1234567890)));
    }

    #[test]
    fn update_peer_empty_message_is_none_not_panic() {
        let u = tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
            message: tl::enums::Message::Empty(tl::types::MessageEmpty {
                id: 7,
                peer_id: None,
            }),
            pts: 1,
            pts_count: 1,
        });
        assert_eq!(update_peer(&u), None);
    }

    fn empty_message() -> tl::enums::Message {
        tl::enums::Message::Empty(tl::types::MessageEmpty {
            id: 7,
            peer_id: None,
        })
    }

    #[test]
    fn is_empty_message_recognizes_peerless_empty() {
        assert!(is_empty_message(&empty_message()));
    }

    #[test]
    fn is_empty_message_rejects_real_and_service_messages() {
        assert!(!is_empty_message(&tl_message(channel_peer())));
        assert!(!is_empty_message(&tl::enums::Message::Service(
            tl::types::MessageService {
                out: false,
                mentioned: false,
                media_unread: false,
                reactions_are_possible: false,
                silent: false,
                post: false,
                legacy: false,
                id: 1,
                from_id: None,
                peer_id: channel_peer(),
                saved_peer_id: None,
                reply_to: None,
                date: 0,
                action: tl::enums::MessageAction::Empty,
                reactions: None,
                ttl_period: None,
            },
        )));
    }

    #[test]
    fn is_empty_update_recognizes_empty_edit_channel_message() {
        let u = tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
            message: empty_message(),
            pts: 1,
            pts_count: 1,
        });
        assert!(is_empty_update(&u));
    }

    #[test]
    fn is_empty_update_recognizes_empty_new_message() {
        let u = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
            message: empty_message(),
            pts: 1,
            pts_count: 1,
        });
        assert!(is_empty_update(&u));
    }

    #[test]
    fn is_empty_update_rejects_real_updates() {
        let new_msg = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
            message: tl_message(channel_peer()),
            pts: 1,
            pts_count: 1,
        });
        let edit_channel =
            tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
                message: tl_message(channel_peer()),
                pts: 1,
                pts_count: 1,
            });
        assert!(!is_empty_update(&new_msg));
        assert!(!is_empty_update(&edit_channel));
        assert!(!is_empty_update(&tl::enums::Update::PtsChanged));
        assert!(!is_empty_update(&tl::enums::Update::DeleteChannelMessages(
            tl::types::UpdateDeleteChannelMessages {
                channel_id: 1,
                messages: vec![1],
                pts: 2,
                pts_count: 1,
            },
        )));
    }

    #[test]
    fn update_peer_delete_channel_messages_uses_channel_id() {
        let u = tl::enums::Update::DeleteChannelMessages(tl::types::UpdateDeleteChannelMessages {
            channel_id: 1234567890,
            messages: vec![1, 2],
            pts: 3,
            pts_count: 1,
        });
        assert_eq!(update_peer(&u), Some(PeerId::channel_unchecked(1234567890)));
    }

    #[test]
    fn update_peer_delete_messages_and_unrelated_are_none() {
        let del = tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
            messages: vec![1],
            pts: 2,
            pts_count: 1,
        });
        assert_eq!(update_peer(&del), None);
        assert_eq!(update_peer(&tl::enums::Update::PtsChanged), None);
    }

    #[test]
    fn update_peer_preserves_peer_kind() {
        let u = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
            message: tl_message(tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 42 })),
            pts: 1,
            pts_count: 1,
        });
        assert_eq!(update_peer(&u), Some(PeerId::chat_unchecked(42)));
    }

    #[test]
    fn raw_should_stream_streams_everything_without_filter() {
        assert!(raw_should_stream(Some(PeerId::channel_unchecked(1)), None));
        assert!(raw_should_stream(None, None));
    }

    #[test]
    fn raw_should_stream_requires_matching_peer_with_filter() {
        let chat = PeerId::channel_unchecked(1234567890);
        assert!(raw_should_stream(Some(chat), Some(chat)));
        assert!(!raw_should_stream(
            Some(PeerId::channel_unchecked(42)),
            Some(chat)
        ));
        assert!(!raw_should_stream(None, Some(chat)));
    }

    #[test]
    fn poll_timeout_without_deadline_is_full_window() {
        let now = std::time::Instant::now();
        assert_eq!(
            poll_timeout(None, now),
            std::time::Duration::from_secs(3600)
        );
    }

    #[test]
    fn poll_timeout_uses_remaining_when_below_window() {
        let now = std::time::Instant::now();
        let deadline = Some(now + std::time::Duration::from_secs(30));
        assert_eq!(
            poll_timeout(deadline, now),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn poll_timeout_caps_remaining_at_window() {
        let now = std::time::Instant::now();
        let deadline = Some(now + std::time::Duration::from_secs(7200));
        assert_eq!(
            poll_timeout(deadline, now),
            std::time::Duration::from_secs(3600)
        );
    }

    #[test]
    fn poll_timeout_zero_when_deadline_passed() {
        let now = std::time::Instant::now();
        let deadline = Some(now - std::time::Duration::from_secs(1));
        assert_eq!(poll_timeout(deadline, now), std::time::Duration::ZERO);
    }

    #[test]
    fn effective_parallel_flag_overrides_config() {
        assert_eq!(effective_parallel(Some(1), 3), 1);
        assert_eq!(effective_parallel(Some(2), 1), 2);
        assert_eq!(effective_parallel(Some(3), 1), 3);
    }

    #[test]
    fn effective_parallel_config_is_fallback_default() {
        assert_eq!(effective_parallel(None, 1), 1);
        assert_eq!(effective_parallel(None, 2), 2);
        assert_eq!(effective_parallel(None, 3), 3);
    }

    #[test]
    fn effective_parallel_clamped_one_to_three() {
        assert_eq!(effective_parallel(Some(0), 3), 1);
        assert_eq!(effective_parallel(Some(9), 1), 3);
        assert_eq!(effective_parallel(None, 0), 1);
        assert_eq!(effective_parallel(None, 99), 3);
    }

    #[test]
    fn reconnect_allowed_up_to_max_attempts() {
        assert!(reconnect_allowed(0));
        assert!(reconnect_allowed(1));
        assert!(reconnect_allowed(MAX_RECONNECT_ATTEMPTS));
        assert!(!reconnect_allowed(MAX_RECONNECT_ATTEMPTS + 1));
    }

    #[test]
    fn reconnect_message_reports_attempt_backoff_and_cause() {
        let msg = reconnect_message("work", 3, 4, "request error: dropped (cancelled)");
        assert!(msg.contains("work"));
        assert!(msg.contains("reconnect"));
        assert!(msg.contains(&format!("3/{MAX_RECONNECT_ATTEMPTS}")));
        assert!(msg.contains("4s"));
        assert!(msg.contains("dropped"));
    }

    #[test]
    fn next_delay_doubles_up_to_cap() {
        assert_eq!(next_delay(1), std::time::Duration::from_secs(1));
        assert_eq!(next_delay(2), std::time::Duration::from_secs(2));
        assert_eq!(next_delay(3), std::time::Duration::from_secs(4));
        assert_eq!(next_delay(4), std::time::Duration::from_secs(8));
        assert_eq!(next_delay(5), std::time::Duration::from_secs(16));
        assert_eq!(next_delay(6), std::time::Duration::from_secs(30));
        assert_eq!(next_delay(30), std::time::Duration::from_secs(30));
    }

    #[test]
    fn next_delay_zero_attempt_is_zero() {
        assert_eq!(next_delay(0), std::time::Duration::ZERO);
    }

    use grammers_client::sender::RpcError;

    fn rpc_error(code: i32, name: &str) -> grammers_client::InvocationError {
        grammers_client::InvocationError::Rpc(RpcError {
            code,
            name: name.to_string(),
            value: None,
            caused_by: None,
        })
    }

    #[test]
    fn getstate_probe_error_is_reconnectable_for_rpc_failure() {
        let err = getstate_probe_error(rpc_error(500, "INTERNAL"));
        assert!(matches!(err, TeleError::Other(_)));
        assert!(
            err.message().starts_with("initial GetState failed:"),
            "err: {err}"
        );
        assert!(err.message().contains("INTERNAL"), "err: {err}");
        assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
        assert!(!is_auth_error(&err));
    }

    #[test]
    fn getstate_probe_error_fails_fast_on_unauthorized() {
        let err = getstate_probe_error(rpc_error(401, "AUTH_KEY_UNREGISTERED"));
        assert!(matches!(err, TeleError::Auth(_)));
        assert_eq!(err.exit_code(), crate::error::EXIT_AUTH);
    }

    #[test]
    fn is_auth_error_accepts_only_auth_kind() {
        assert!(is_auth_error(&TeleError::Auth(
            "session invalid".to_string()
        )));
        assert!(!is_auth_error(&TeleError::Usage("x".to_string())));
        assert!(!is_auth_error(&TeleError::Config("x".to_string())));
        assert!(!is_auth_error(&TeleError::Invocation(
            "rpc error 400: CHAT_INVALID".to_string(),
            None
        )));
        assert!(!is_auth_error(&TeleError::Other("x".to_string())));
    }

    #[test]
    fn aggregate_exit_all_ok_is_ok() {
        assert_eq!(aggregate_exit(3, &[]), crate::error::EXIT_OK);
    }

    #[test]
    fn aggregate_exit_any_success_is_partial() {
        assert_eq!(
            aggregate_exit(1, &[crate::error::EXIT_ALL_FAILED]),
            crate::error::EXIT_PARTIAL
        );
    }

    #[test]
    fn aggregate_exit_all_failed_auth_only_is_auth() {
        assert_eq!(
            aggregate_exit(0, &[crate::error::EXIT_AUTH, crate::error::EXIT_AUTH]),
            crate::error::EXIT_AUTH
        );
    }

    #[test]
    fn aggregate_exit_all_failed_mixed_is_all_failed() {
        assert_eq!(
            aggregate_exit(0, &[crate::error::EXIT_AUTH, crate::error::EXIT_ALL_FAILED]),
            crate::error::EXIT_ALL_FAILED
        );
        assert_eq!(
            aggregate_exit(
                0,
                &[crate::error::EXIT_USAGE, crate::error::EXIT_ALL_FAILED]
            ),
            crate::error::EXIT_ALL_FAILED
        );
    }

    #[test]
    fn aggregate_exit_returns_usage_when_all_failures_usage() {
        assert_eq!(
            aggregate_exit(0, &[crate::error::EXIT_USAGE, crate::error::EXIT_USAGE]),
            crate::error::EXIT_USAGE
        );
    }

    #[test]
    fn aggregate_exit_returns_auth_when_all_failures_auth() {
        assert_eq!(
            aggregate_exit(0, &[crate::error::EXIT_AUTH]),
            crate::error::EXIT_AUTH
        );
    }

    #[test]
    fn aggregate_exit_returns_all_failed_for_mixed() {
        assert_eq!(
            aggregate_exit(0, &[crate::error::EXIT_USAGE, crate::error::EXIT_AUTH]),
            crate::error::EXIT_ALL_FAILED
        );
    }

    #[test]
    fn aggregate_exit_returns_ok_when_no_failures() {
        assert_eq!(aggregate_exit(1, &[]), crate::error::EXIT_OK);
    }

    #[test]
    fn aggregate_exit_returns_partial_when_some_ok() {
        assert_eq!(
            aggregate_exit(1, &[crate::error::EXIT_USAGE]),
            crate::error::EXIT_PARTIAL
        );
    }

    #[test]
    fn on_failure_increments_consecutive_counter() {
        assert_eq!(on_failure(0), 1);
        assert_eq!(on_failure(1), 2);
        assert_eq!(
            on_failure(MAX_RECONNECT_ATTEMPTS),
            MAX_RECONNECT_ATTEMPTS + 1
        );
    }

    #[test]
    fn on_reconnect_success_resets_counter_to_zero() {
        assert_eq!(on_reconnect_success(0), 0);
        assert_eq!(on_reconnect_success(3), 0);
        assert_eq!(on_reconnect_success(MAX_RECONNECT_ATTEMPTS + 1), 0);
    }

    #[test]
    fn failure_then_reconnect_success_cycle_never_gives_up() {
        let mut failures = 0;
        for _ in 0..10 {
            failures = on_failure(failures);
            assert!(reconnect_allowed(failures));
            failures = on_reconnect_success(failures);
            assert_eq!(failures, 0);
        }
    }

    #[test]
    fn consecutive_failures_give_up_after_max_attempts() {
        let mut failures = 0;
        while !give_up(failures) {
            failures = on_failure(failures);
        }
        assert_eq!(failures, MAX_RECONNECT_ATTEMPTS + 1);
        assert!(give_up(failures));
    }
}
