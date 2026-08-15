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
    if flags.dry_run {
        output::log_line("info", "[dry-run] would stream updates");
        return Ok(crate::error::EXIT_OK);
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
                let mut backoff: u32 = 1;
                let mut failures: u32 = 0;
                loop {
                    if let Some(d) = deadline {
                        if std::time::Instant::now() >= d {
                            break;
                        }
                    }
                    let mut guard =
                        ClientGuard::connect(&name, creds.api_id, config_path.as_deref()).await?;
                    client::authorize(&guard.client).await?;
                    if resolved.is_none() {
                        resolved = match &chat_filter {
                            Some(target) => Some(
                                crate::entities::resolve_peer(&guard.client, &guard.session, target)
                                    .await
                                    .map_err(|e| {
                                        TeleError::Other(format!("cannot resolve chat {target:?}: {e}"))
                                    })?
                                    .id(),
                            ),
                            None => None,
                        };
                    }
                    let receiver = std::mem::replace(
                        &mut guard.updates,
                        tokio::sync::mpsc::unbounded_channel().1,
                    );
                    let mut stream = guard
                        .client
                        .stream_updates(
                            receiver,
                            grammers_client::client::UpdatesConfiguration::default(),
                        )
                        .await
                        .map_err(|e| TeleError::Other(e.to_string()))?;
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
                                failures += 1;
                                if !reconnect_allowed(failures) {
                                    let mut err = crate::error::invocation_error(e);
                                    if let TeleError::Invocation(message, _) = &mut err {
                                        *message = format!(
                                            "{name}: updates stream failed {failures} consecutive times, giving up: {message}"
                                        );
                                    }
                                    return Err(err);
                                }
                                let sleep_for = match deadline {
                                    Some(d) => std::time::Duration::from_secs(backoff.into())
                                        .min(d.saturating_duration_since(std::time::Instant::now())),
                                    None => std::time::Duration::from_secs(backoff.into()),
                                };
                                output::log_line(
                                    "error",
                                    &reconnect_message(
                                        &name,
                                        failures,
                                        backoff,
                                        &crate::error::invocation_message(&e),
                                    ),
                                );
                                tokio::time::sleep(sleep_for).await;
                                backoff = next_backoff(backoff);
                                break;
                            }
                            Err(_) => continue,
                        };
                        failures = 0;
                        backoff = 1;
                        match &update {
                        Update::NewMessage(m) => {
                            if !events.iter().any(|e| e == "NewMessage") {
                                continue;
                            }
                            if let Some(target) = &resolved {
                                if !message_matches(m.peer_id(), *target) {
                                    continue;
                                }
                            }
                            let row = crate::serialize::message_to_json(m)?;
                            output::print_json_result(&event_row(
                                "NewMessage",
                                &name,
                                m.peer_id().bare_id(),
                                None,
                                Some(row),
                            ))?;
                        }
                        Update::MessageEdited(m) => {
                            if !events.iter().any(|e| e == "MessageEdited") {
                                continue;
                            }
                            if let Some(target) = &resolved {
                                if !message_matches(m.peer_id(), *target) {
                                    continue;
                                }
                            }
                            let row = crate::serialize::message_to_json(m)?;
                            output::print_json_result(&event_row(
                                "MessageEdited",
                                &name,
                                m.peer_id().bare_id(),
                                None,
                                Some(row),
                            ))?;
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
                            output::print_json_result(&event_row(
                                "MessageDeleted",
                                &name,
                                d.channel_id(),
                                Some(d.messages()),
                                None,
                            ))?;
                        }
                        _ => {
                            if !events.iter().any(|e| e == "Raw") {
                                continue;
                            }
                            output::print_json_result(&raw_row(
                                &name,
                                update.raw(),
                                update.state(),
                            ))?;
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

fn aggregate_exit(ok_count: usize, failed: &[i32]) -> i32 {
    if failed.is_empty() {
        crate::error::EXIT_OK
    } else if ok_count > 0 {
        crate::error::EXIT_PARTIAL
    } else if failed.iter().all(|c| *c == crate::error::EXIT_AUTH) {
        crate::error::EXIT_AUTH
    } else {
        crate::error::EXIT_ALL_FAILED
    }
}

fn message_matches(peer: PeerId, resolved: PeerId) -> bool {
    peer == resolved
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

fn reconnect_message(account: &str, failures: u32, backoff: u32, cause: &str) -> String {
    format!(
        "{account}: updates stream error ({cause}), reconnecting (attempt {failures}/{MAX_RECONNECT_ATTEMPTS}) in {backoff}s"
    )
}

fn next_backoff(secs: u32) -> u32 {
    (secs * 2).min(MAX_RECONNECT_BACKOFF)
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
    fn message_matches_same_peer() {
        let chat = PeerId::channel_unchecked(1234567890);
        assert!(message_matches(chat, chat));
    }

    #[test]
    fn message_matches_rejects_other_peer() {
        let a = PeerId::channel_unchecked(1234567890);
        let b = PeerId::channel_unchecked(42);
        assert!(!message_matches(a, b));
    }

    #[test]
    fn message_matches_distinguishes_user_from_chat_with_same_bare_id() {
        let user = PeerId::user_unchecked(123);
        let chat = PeerId::chat_unchecked(123);
        assert!(!message_matches(user, chat));
    }

    #[test]
    fn message_matches_distinguishes_chat_from_channel_with_same_bare_id() {
        let chat = PeerId::chat_unchecked(123);
        let channel = PeerId::channel_unchecked(123);
        assert!(!message_matches(chat, channel));
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
    fn reconnect_backoff_doubles_up_to_cap() {
        assert_eq!(next_backoff(1), 2);
        assert_eq!(next_backoff(2), 4);
        assert_eq!(next_backoff(8), 16);
        assert_eq!(next_backoff(16), 30);
        assert_eq!(next_backoff(30), 30);
    }

    #[test]
    fn reconnect_backoff_zero_stays_zero() {
        assert_eq!(next_backoff(0), 0);
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
            aggregate_exit(0, &[crate::error::EXIT_USAGE]),
            crate::error::EXIT_ALL_FAILED
        );
    }
}
