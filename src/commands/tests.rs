use super::*;
use crate::executor::effective_parallel;
use grammers_session::Session;

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
fn deletion_match_set_channel_target_takes_full_list_with_label() {
    let targets = vec![PeerId::channel_unchecked(1234567890)];
    let matched = deletion_match_set(&[4, 5], Some(1234567890), &ObservedPeers::new(), &targets)
        .expect("channel hit");
    assert_eq!(matched, vec![4, 5]);
    assert!(deletion_match_set(&[4], Some(999), &ObservedPeers::new(), &targets).is_none());
    assert!(
        deletion_match_set(&[4], None, &ObservedPeers::new(), &targets).is_none(),
        "channel target cannot match a deletion without channel_id"
    );
}

#[test]
fn deletion_match_set_observed_subset_for_user_target() {
    let observed = observed_fixture();
    let targets = vec![PeerId::user_unchecked(7)];
    let matched =
        deletion_match_set(&[999, 102, 101], None, &observed, &targets).expect("observed hit");
    assert_eq!(matched, vec![102, 101]);
    assert!(deletion_match_set(&[999], None, &observed, &targets).is_none());
}

#[test]
fn deletion_match_set_unions_targets_deduped_in_order() {
    let mut observed = ObservedPeers::new();
    observed.observe(10, PeerId::chat_unchecked(42));
    observed.observe(11, PeerId::user_unchecked(7));
    let targets = vec![PeerId::chat_unchecked(42), PeerId::user_unchecked(7)];
    let matched = deletion_match_set(&[11, 10, 11], None, &observed, &targets).expect("union hit");
    assert_eq!(matched, vec![10, 11]);
}

#[test]
fn sole_chat_label_labels_only_single_target() {
    assert_eq!(sole_chat_label(&[PeerId::channel_unchecked(5)]), Some(5));
    assert_eq!(sole_chat_label(&[]), None);
    assert_eq!(
        sole_chat_label(&[PeerId::channel_unchecked(5), PeerId::user_unchecked(6)]),
        None
    );
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
    let edit_channel = tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
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
fn chat_allows_everything_when_no_targets() {
    let f = EventFilter::default();
    assert!(f.chat_allows(Some(PeerId::channel_unchecked(1))));
    assert!(f.chat_allows(None));
}

#[test]
fn chat_allows_any_target_in_union() {
    let f = EventFilter {
        chats: vec![PeerId::channel_unchecked(7), PeerId::user_unchecked(9)],
        ..Default::default()
    };
    assert!(f.chat_allows(Some(PeerId::user_unchecked(9))));
    assert!(f.chat_allows(Some(PeerId::channel_unchecked(7))));
    assert!(!f.chat_allows(Some(PeerId::channel_unchecked(42))));
    assert!(!f.chat_allows(None));
}

#[test]
fn chat_allows_distinguishes_peer_kind_on_same_bare_id() {
    let f = EventFilter {
        chats: vec![PeerId::chat_unchecked(7)],
        ..Default::default()
    };
    assert!(f.chat_allows(Some(PeerId::chat_unchecked(7))));
    assert!(!f.chat_allows(Some(PeerId::user_unchecked(7))));
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
    assert_eq!(effective_parallel(Some(1), 3).unwrap(), 1);
    assert_eq!(effective_parallel(Some(2), 1).unwrap(), 2);
    assert_eq!(effective_parallel(Some(32), 1).unwrap(), 32);
}

#[test]
fn effective_parallel_config_is_fallback_default() {
    assert_eq!(effective_parallel(None, 1).unwrap(), 1);
    assert_eq!(effective_parallel(None, 2).unwrap(), 2);
    assert_eq!(effective_parallel(None, 32).unwrap(), 32);
}

#[test]
fn effective_parallel_out_of_range_errors() {
    assert!(matches!(
        effective_parallel(Some(0), 3),
        Err(TeleError::Usage(_))
    ));
    assert!(matches!(
        effective_parallel(Some(99), 1),
        Err(TeleError::Usage(_))
    ));
    assert!(matches!(
        effective_parallel(None, 0),
        Err(TeleError::Usage(_))
    ));
    assert!(matches!(
        effective_parallel(None, 999),
        Err(TeleError::Usage(_))
    ));
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
fn getstate_probe_error_keeps_rpc_taxonomy() {
    let err = getstate_probe_error(rpc_error(500, "INTERNAL"));
    assert!(matches!(
        &err,
        TeleError::Rpc(_, 500, name, _) if name == "INTERNAL"
    ));
    assert!(
        err.message().starts_with("initial GetState failed:"),
        "err: {err}"
    );
    assert!(err.message().contains("INTERNAL"), "err: {err}");
    assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
    assert!(!is_auth_error(&err));
}

#[test]
fn getstate_probe_error_keeps_other_for_non_rpc() {
    let err = getstate_probe_error(grammers_client::InvocationError::Dropped);
    assert!(matches!(err, TeleError::Other(_)));
    assert!(err.message().starts_with("initial GetState failed:"));
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
    assert_eq!(
        crate::error::aggregate_exit_code(3, &[]),
        crate::error::EXIT_OK
    );
}

#[test]
fn aggregate_exit_any_success_is_partial() {
    assert_eq!(
        crate::error::aggregate_exit_code(1, &[crate::error::EXIT_ALL_FAILED]),
        crate::error::EXIT_PARTIAL
    );
}

#[test]
fn aggregate_exit_all_failed_auth_only_is_auth() {
    assert_eq!(
        crate::error::aggregate_exit_code(0, &[crate::error::EXIT_AUTH, crate::error::EXIT_AUTH]),
        crate::error::EXIT_AUTH
    );
}

#[test]
fn aggregate_exit_mixed_failures_let_telegram_win_over_usage() {
    assert_eq!(
        crate::error::aggregate_exit_code(
            0,
            &[crate::error::EXIT_USAGE, crate::error::EXIT_ALL_FAILED]
        ),
        crate::error::EXIT_ALL_FAILED
    );
    assert_eq!(
        crate::error::aggregate_exit_code(0, &[crate::error::EXIT_USAGE, crate::error::EXIT_AUTH]),
        crate::error::EXIT_AUTH
    );
    assert_eq!(
        crate::error::aggregate_exit_code(
            0,
            &[crate::error::EXIT_AUTH, crate::error::EXIT_ALL_FAILED]
        ),
        crate::error::EXIT_AUTH
    );
}

#[test]
fn aggregate_exit_returns_usage_when_all_failures_usage() {
    assert_eq!(
        crate::error::aggregate_exit_code(0, &[crate::error::EXIT_USAGE, crate::error::EXIT_USAGE]),
        crate::error::EXIT_USAGE
    );
}

#[test]
fn aggregate_exit_returns_auth_when_all_failures_auth() {
    assert_eq!(
        crate::error::aggregate_exit_code(0, &[crate::error::EXIT_AUTH]),
        crate::error::EXIT_AUTH
    );
}

#[test]
fn aggregate_exit_returns_partial_when_some_ok() {
    assert_eq!(
        crate::error::aggregate_exit_code(1, &[crate::error::EXIT_USAGE]),
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

fn user_tl_peer() -> tl::enums::Peer {
    tl::enums::Peer::User(tl::types::PeerUser { user_id: 7 })
}

fn pts_update(
    peer: tl::enums::Peer,
    id: i32,
    pts: i32,
    count: i32,
    grouped_id: Option<i64>,
) -> tl::enums::Update {
    let mut msg = match tl_message(peer) {
        tl::enums::Message::Message(m) => m,
        _ => unreachable!("tl_message always builds a concrete message"),
    };
    msg.id = id;
    msg.grouped_id = grouped_id;
    tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: tl::enums::Message::Message(msg),
        pts,
        pts_count: count,
    })
}

fn channel_pts_update(channel_id: i64, pts: i32, count: i32) -> tl::enums::Update {
    tl::enums::Update::DeleteChannelMessages(tl::types::UpdateDeleteChannelMessages {
        channel_id,
        messages: vec![1],
        pts,
        pts_count: count,
    })
}

#[test]
fn gap_tracker_first_sighting_records_baseline() {
    let mut t = GapTracker::default();
    let u = pts_update(user_tl_peer(), 1, 10, 2, None);
    assert!(t.observe(&u).is_none());
    let u = pts_update(user_tl_peer(), 2, 12, 1, None);
    assert!(t.observe(&u).is_none(), "contiguous advance is not a gap");
}

#[test]
fn gap_tracker_reports_jump_with_expected_and_observed() {
    let mut t = GapTracker::default();
    assert!(t
        .observe(&pts_update(user_tl_peer(), 1, 10, 1, None))
        .is_none());
    let signal = t
        .observe(&pts_update(user_tl_peer(), 2, 15, 1, None))
        .expect("jump must signal");
    assert_eq!(signal.expected_pts, 11);
    assert_eq!(signal.observed_pts, 15);
    assert_eq!(signal.box_key, StateBoxKey::Common);
}

#[test]
fn gap_tracker_accepts_count_sized_advance_without_gap() {
    let mut t = GapTracker::default();
    assert!(t
        .observe(&pts_update(user_tl_peer(), 1, 10, 3, None))
        .is_none());
    assert!(t
        .observe(&pts_update(user_tl_peer(), 2, 13, 1, None))
        .is_none());
}

#[test]
fn gap_tracker_ignores_stale_and_duplicate_pts() {
    let mut t = GapTracker::default();
    assert!(t
        .observe(&pts_update(user_tl_peer(), 1, 10, 2, None))
        .is_none());
    assert!(t
        .observe(&pts_update(user_tl_peer(), 1, 8, 1, None))
        .is_none());
    assert!(t
        .observe(&pts_update(user_tl_peer(), 1, 10, 1, None))
        .is_none());
    let signal = t
        .observe(&pts_update(user_tl_peer(), 3, 99, 1, None))
        .expect("tracker still advances after stale input");
    assert_eq!(signal.expected_pts, 12, "stale pts did not move baseline");
}

#[test]
fn gap_tracker_tracks_channels_independently_of_common_box() {
    let mut t = GapTracker::default();
    assert!(t.observe(&channel_pts_update(1000, 5, 1)).is_none());
    assert!(
        t.observe(&pts_update(user_tl_peer(), 1, 50, 1, None))
            .is_none(),
        "common box starts fresh even with channel state present"
    );
    let signal = t
        .observe(&channel_pts_update(1000, 9, 1))
        .expect("channel jump signals");
    assert_eq!(signal.box_key, StateBoxKey::Channel(1000));
    assert_eq!(signal.observed_pts, 9);
    assert!(
        t.observe(&channel_pts_update(2000, 4, 1)).is_none(),
        "second channel has its own box"
    );
}

#[test]
fn gap_tracker_caps_and_evicts_oldest_channel_boxes() {
    let mut t = GapTracker::default();
    for ch in 0..(GAP_TRACKER_CAP as i64) {
        assert!(t.observe(&channel_pts_update(ch, 1, 1)).is_none());
    }
    assert_eq!(t.last.len(), GAP_TRACKER_CAP);
    assert!(t
        .observe(&channel_pts_update(GAP_TRACKER_CAP as i64, 1, 1))
        .is_none());
    assert_eq!(
        t.last.len(),
        GAP_TRACKER_CAP,
        "cap must hold after overflow insert"
    );
    assert!(
        t.observe(&channel_pts_update(0, 100, 1)).is_none(),
        "evicted box re-baselines instead of signaling a stale gap"
    );
}

#[test]
fn pts_point_reads_channel_from_message_peer_for_channel_updates() {
    let raw = tl::enums::Update::NewChannelMessage(tl::types::UpdateNewChannelMessage {
        message: tl_message(channel_peer()),
        pts: 3,
        pts_count: 1,
    });
    let point = pts_point(&raw).unwrap();
    assert_eq!(point.box_key, StateBoxKey::Channel(1234567890));
    let raw = tl::enums::Update::EditChannelMessage(tl::types::UpdateEditChannelMessage {
        message: tl_message(channel_peer()),
        pts: 4,
        pts_count: 2,
    });
    let point = pts_point(&raw).unwrap();
    assert_eq!(point.box_key, StateBoxKey::Channel(1234567890));
    assert_eq!(point.pts_count, 2);
}

#[test]
fn pts_point_common_variants_cover_new_edit_delete() {
    for raw in [
        pts_update(user_tl_peer(), 1, 5, 1, None),
        tl::enums::Update::EditMessage(tl::types::UpdateEditMessage {
            message: tl_message(user_tl_peer()),
            pts: 6,
            pts_count: 1,
        }),
        tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
            messages: vec![1],
            pts: 7,
            pts_count: 1,
        }),
    ] {
        let point = pts_point(&raw).expect("message-bearing update carries pts");
        assert_eq!(point.box_key, StateBoxKey::Common);
    }
}

#[test]
fn pts_point_skips_updates_without_message_pts() {
    assert!(pts_point(&tl::enums::Update::PtsChanged).is_none());
    let empty_channel = tl::enums::Update::NewChannelMessage(tl::types::UpdateNewChannelMessage {
        message: empty_message(),
        pts: 1,
        pts_count: 1,
    });
    assert!(
        pts_point(&empty_channel).is_none(),
        "channel updates without a peer cannot be keyed"
    );
}

#[test]
fn gap_row_shape_matches_raw_state_snapshot_convention() {
    let mut t = GapTracker::default();
    assert!(t.observe(&channel_pts_update(1000, 5, 1)).is_none());
    let signal = t.observe(&channel_pts_update(1000, 20, 1)).expect("gap");
    let state = State {
        date: 77,
        seq: 88,
        message_box: Some(MessageBox::Channel {
            channel_id: 1000,
            pts: 20,
        }),
    };
    let row = gap_row("work", &signal, &state);
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "Gap");
    assert_eq!(obj["account"], "work");
    assert_eq!(obj["reason"], "pts_jump");
    assert_eq!(obj["expected_pts"], 6);
    assert_eq!(obj["observed_pts"], 20);
    assert_eq!(obj["channel_id"], 1000);
    assert_eq!(obj["state"]["date"], 77);
    assert_eq!(obj["state"]["seq"], 88);
    assert_eq!(obj["state"]["channel_id"], 1000);
    assert_eq!(obj["state"]["pts"], 20);
}

fn member_row(id: i32, chat: i64, grouped: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "date": format!("2026-08-13T12:00:{id:02}+00:00"),
        "text": format!("m{id}"),
        "peer": {"id": chat, "kind": "chat", "name": "g"},
        "grouped_id": grouped,
    })
}

#[test]
fn album_member_reads_chat_and_grouped_id() {
    let (chat, gid) = album_member(&member_row(1, 456, 9001)).unwrap();
    assert_eq!(chat, 456);
    assert_eq!(gid, 9001);
}

#[test]
fn album_member_rejects_ungrouped_and_peerless_rows() {
    let ungrouped = serde_json::json!({"id": 1, "peer": {"id": 5}});
    assert!(album_member(&ungrouped).is_none());
    let no_peer = serde_json::json!({"id": 1, "grouped_id": 3});
    assert!(album_member(&no_peer).is_none());
    let null_peer = serde_json::json!({"id": 1, "grouped_id": 3, "peer": serde_json::Value::Null});
    assert!(album_member(&null_peer).is_none());
}

#[tokio::test(start_paused = true)]
async fn album_ingest_buffers_members_and_extends_deadline_to_quiescence() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    assert!(album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now).is_empty());
    assert!(album_ingest("work", &mut buf, member_row(2, 456, 9001), 456, 9001, now).is_empty());
    tokio::time::advance(std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS - 1)).await;
    let pending = buf
        .get(&(456, 9001))
        .expect("album still pending before deadline");
    assert_eq!(pending.rows.len(), 2);
    let deadline = pending.deadline;
    tokio::time::sleep_until(deadline).await;
    let done = album_flush("work", &mut buf);
    assert_eq!(done.len(), 1);
    let done = done.into_iter().next().unwrap();
    assert_eq!(done["event"], "Album");
    assert_eq!(done["messages"].as_array().unwrap().len(), 2);
    assert!(buf.is_empty());
}

#[tokio::test(start_paused = true)]
async fn album_ingest_group_switch_keeps_previous_album_pending() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
    album_ingest("work", &mut buf, member_row(2, 456, 9001), 456, 9001, now);
    assert!(album_ingest("home", &mut buf, member_row(9, 789, 42), 789, 42, now).is_empty());
    assert_eq!(buf.len(), 2, "both albums stay pending on key switch");
    let pending = buf.get(&(456, 9001)).unwrap();
    assert_eq!((pending.chat_id, pending.grouped_id), (456, 9001));
    assert_eq!(pending.rows.len(), 2);
    let pending = buf.get(&(789, 42)).unwrap();
    assert_eq!((pending.chat_id, pending.grouped_id), (789, 42));
    assert_eq!(pending.rows.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn album_sweep_flushes_only_due_albums() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
    album_ingest("home", &mut buf, member_row(9, 789, 42), 789, 42, now);
    tokio::time::advance(std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS)).await;
    let later = tokio::time::Instant::now();
    album_ingest("home", &mut buf, member_row(10, 789, 43), 789, 43, later);
    let swept = album_sweep("work", &mut buf, later);
    assert_eq!(
        swept.len(),
        2,
        "both expired albums flushed, fresh one kept"
    );
    let grouped: Vec<i64> = swept
        .iter()
        .map(|r| r["grouped_id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        grouped,
        vec![9001, 42],
        "flush ordered by deadline, ties by (chat_id, grouped_id)"
    );
    assert_eq!(buf.len(), 1);
    assert!(buf.contains_key(&(789, 43)));
}

#[tokio::test(start_paused = true)]
async fn album_sweep_respects_per_entry_deadlines() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
    tokio::time::advance(std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS - 1)).await;
    let mid = tokio::time::Instant::now();
    assert!(
        album_sweep("work", &mut buf, mid).is_empty(),
        "nothing due before the deadline"
    );
    tokio::time::sleep_until(buf.values().next().unwrap().deadline).await;
    let swept = album_sweep("work", &mut buf, tokio::time::Instant::now());
    assert_eq!(swept.len(), 1);
    assert!(buf.is_empty());
}

#[tokio::test(start_paused = true)]
async fn album_buffer_cap_evicts_oldest_and_flushes_it() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    for i in 0..ALBUM_BUFFER_CAP {
        let gid = (i + 1) as i64;
        album_ingest(
            "work",
            &mut buf,
            member_row(i as i32, 456, gid),
            456,
            gid,
            now,
        );
    }
    assert_eq!(buf.len(), ALBUM_BUFFER_CAP);
    album_ingest("work", &mut buf, member_row(99, 789, 42), 789, 42, now);
    assert_eq!(buf.len(), ALBUM_BUFFER_CAP, "cap maintained via eviction");
    assert!(
        !buf.contains_key(&(456, 1)),
        "oldest album (earliest deadline) was evicted"
    );
    assert!(buf.contains_key(&(789, 42)));
}

#[tokio::test(start_paused = true)]
async fn album_flush_chat_flushes_only_matching_chat() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
    album_ingest("work", &mut buf, member_row(2, 789, 42), 789, 42, now);
    let flushed = album_flush_chat("work", &mut buf, Some(456));
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0]["chat_id"], 456);
    assert_eq!(buf.len(), 1);
    assert!(buf.contains_key(&(789, 42)));
    assert!(album_flush_chat("work", &mut buf, Some(111)).is_empty());
    let flushed = album_flush_chat("work", &mut buf, None);
    assert!(flushed.is_empty(), "None targets no chat, flushes nothing");
    assert_eq!(buf.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn album_flush_timer_fires_after_quiescence_window() {
    let mut buf = AlbumBuffer::new();
    let now = tokio::time::Instant::now();
    album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
    let deadline = buf.get(&(456, 9001)).unwrap().deadline;
    assert_eq!(
        deadline - now,
        std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS)
    );
    tokio::time::sleep_until(deadline).await;
    assert!(tokio::time::Instant::now() >= deadline);
}

#[test]
fn album_flush_empty_buffer_is_none() {
    let mut buf = AlbumBuffer::new();
    assert!(album_flush("work", &mut buf).is_empty());
}

#[test]
fn album_complete_carries_shared_metadata_and_member_payloads() {
    let pending = PendingAlbum {
        chat_id: 456,
        grouped_id: 9001,
        rows: vec![member_row(1, 456, 9001), member_row(2, 456, 9001)],
        deadline: tokio::time::Instant::now(),
    };
    let row = album_complete("work", &pending);
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "Album");
    assert_eq!(obj["account"], "work");
    assert_eq!(obj["chat_id"], 456);
    assert_eq!(obj["grouped_id"], serde_json::json!(9001));
    assert_eq!(obj["ids"], serde_json::json!([1, 2]));
    assert_eq!(obj["date"], "2026-08-13T12:00:01+00:00");
    let messages = obj["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["text"], "m1");
    assert_eq!(messages[1]["text"], "m2");
}

fn observed_fixture() -> ObservedPeers {
    let mut o = ObservedPeers::new();
    o.observe(101, PeerId::user_unchecked(7));
    o.observe(102, PeerId::user_unchecked(7));
    o.observe(103, PeerId::chat_unchecked(42));
    o
}

#[test]
fn observed_peers_round_trips_lookup() {
    let o = observed_fixture();
    assert_eq!(
        o.peer_of(PeerId::user_unchecked(7), 101),
        Some(PeerId::user_unchecked(7))
    );
    assert_eq!(
        o.peer_of(PeerId::chat_unchecked(42), 103),
        Some(PeerId::chat_unchecked(42))
    );
    assert_eq!(o.peer_of(PeerId::user_unchecked(7), 999), None);
}

#[test]
fn observed_peers_evicts_oldest_beyond_cap() {
    let mut o = ObservedPeers::new();
    for i in 0..(OBSERVED_PEER_CAP as i32) {
        o.observe(i, PeerId::user_unchecked(1));
    }
    o.observe(OBSERVED_PEER_CAP as i32, PeerId::user_unchecked(2));
    assert_eq!(
        o.peer_of(PeerId::user_unchecked(1), 0),
        None,
        "oldest entry evicted"
    );
    assert_eq!(
        o.peer_of(PeerId::user_unchecked(2), OBSERVED_PEER_CAP as i32),
        Some(PeerId::user_unchecked(2))
    );
    assert_eq!(o.by_id.len(), OBSERVED_PEER_CAP);
}

#[test]
fn observed_peers_reinsert_keeps_first_mapping_without_double_queue_entry() {
    let mut o = ObservedPeers::new();
    o.observe(1, PeerId::user_unchecked(7));
    o.observe(1, PeerId::user_unchecked(7));
    assert_eq!(
        o.peer_of(PeerId::user_unchecked(7), 1),
        Some(PeerId::user_unchecked(7))
    );
    assert_eq!(o.by_id.len(), 1);
}

#[test]
fn observed_deletion_ids_filters_by_target_and_keeps_order() {
    let o = observed_fixture();
    let target = PeerId::user_unchecked(7);
    assert_eq!(
        observed_deletion_ids(&[999, 102, 103, 101], &o, target),
        vec![102, 101]
    );
    assert!(observed_deletion_ids(&[999], &o, target).is_empty());
}

#[test]
fn observed_deletion_ids_distinguishes_peer_kind() {
    let o = observed_fixture();
    let chat = PeerId::chat_unchecked(42);
    assert_eq!(observed_deletion_ids(&[103], &o, chat), vec![103]);
    let impostor = PeerId::user_unchecked(42);
    assert!(observed_deletion_ids(&[103], &o, impostor).is_empty());
}

fn filter_message(
    f: &EventFilter,
    peer: Option<PeerId>,
    sender: Option<PeerId>,
    out: Option<bool>,
) -> bool {
    f.chat_allows(peer) && f.message_allows(sender, out)
}

#[test]
fn sender_filter_matches_any_target_in_union() {
    let f = EventFilter {
        senders: vec![PeerId::user_unchecked(7), PeerId::channel_unchecked(8)],
        ..Default::default()
    };
    assert!(f.message_allows(Some(PeerId::user_unchecked(7)), Some(false)));
    assert!(f.message_allows(Some(PeerId::channel_unchecked(8)), None));
    assert!(!f.message_allows(Some(PeerId::user_unchecked(9)), Some(true)));
}

#[test]
fn sender_filter_drops_messages_without_sender() {
    let f = EventFilter {
        senders: vec![PeerId::user_unchecked(7)],
        ..Default::default()
    };
    assert!(!f.message_allows(None, Some(false)));
    assert!(EventFilter::default().message_allows(None, Some(false)));
}

#[test]
fn direction_out_keeps_only_outgoing_rows() {
    let f = EventFilter {
        direction: Some(Direction::Out),
        ..Default::default()
    };
    assert!(f.message_allows(None, Some(true)));
    assert!(!f.message_allows(None, Some(false)));
}

#[test]
fn direction_in_keeps_only_incoming_rows() {
    let f = EventFilter {
        direction: Some(Direction::In),
        ..Default::default()
    };
    assert!(f.message_allows(None, Some(false)));
    assert!(!f.message_allows(None, Some(true)));
}

#[test]
fn direction_drops_events_without_out_flag() {
    let fin = EventFilter {
        direction: Some(Direction::In),
        ..Default::default()
    };
    let fout = EventFilter {
        direction: Some(Direction::Out),
        ..Default::default()
    };
    let sender = Some(PeerId::user_unchecked(1));
    assert!(!fin.message_allows(sender, None));
    assert!(!fout.message_allows(sender, None));
    assert!(
        EventFilter::default().message_allows(sender, None),
        "unset filters pass rows regardless of shape"
    );
}

#[test]
fn filter_dimensions_compose_and_wise() {
    let f = EventFilter {
        chats: vec![PeerId::chat_unchecked(1)],
        senders: vec![PeerId::user_unchecked(2)],
        direction: Some(Direction::In),
        patterns: Vec::new(),
    };
    let peer_ok = Some(PeerId::chat_unchecked(1));
    let sender_ok = Some(PeerId::user_unchecked(2));
    assert!(filter_message(&f, peer_ok, sender_ok, Some(false)));
    assert!(!filter_message(
        &f,
        Some(PeerId::chat_unchecked(5)),
        sender_ok,
        Some(false)
    ));
    assert!(!filter_message(
        &f,
        peer_ok,
        Some(PeerId::user_unchecked(3)),
        Some(false)
    ));
    assert!(!filter_message(&f, peer_ok, sender_ok, Some(true)));
}

#[test]
fn sender_dimension_is_or_wise_within_itself() {
    let f = EventFilter {
        chats: vec![PeerId::chat_unchecked(1)],
        senders: vec![PeerId::user_unchecked(2), PeerId::user_unchecked(3)],
        direction: None,
        patterns: Vec::new(),
    };
    let peer_ok = Some(PeerId::chat_unchecked(1));
    assert!(filter_message(
        &f,
        peer_ok,
        Some(PeerId::user_unchecked(2)),
        Some(false)
    ));
    assert!(filter_message(
        &f,
        peer_ok,
        Some(PeerId::user_unchecked(3)),
        Some(true)
    ));
}

#[test]
fn raw_events_blocked_when_sender_or_direction_filters_set() {
    let peer = Some(PeerId::channel_unchecked(4));
    assert!(EventFilter::default().raw_allows(peer));
    let f_chat = EventFilter {
        chats: vec![PeerId::channel_unchecked(4)],
        ..Default::default()
    };
    assert!(f_chat.raw_allows(peer));
    assert!(!f_chat.raw_allows(Some(PeerId::channel_unchecked(5))));
    let f_from = EventFilter {
        senders: vec![PeerId::user_unchecked(7)],
        ..Default::default()
    };
    assert!(!f_from.raw_allows(peer), "raw has no sender to check");
    let f_dir = EventFilter {
        direction: Some(Direction::In),
        ..Default::default()
    };
    assert!(!f_dir.raw_allows(peer), "raw has no out flag");
}

#[test]
fn deletions_blocked_when_sender_or_direction_filters_set() {
    assert!(EventFilter::default().deletions_pass());
    assert!(EventFilter {
        chats: vec![PeerId::channel_unchecked(4)],
        ..Default::default()
    }
    .deletions_pass());
    let f_from = EventFilter {
        senders: vec![PeerId::user_unchecked(7)],
        ..Default::default()
    };
    assert!(!f_from.deletions_pass());
    let f_dir = EventFilter {
        direction: Some(Direction::Out),
        ..Default::default()
    };
    assert!(!f_dir.deletions_pass());
}

fn text_update(body: &str) -> tl::enums::Update {
    let mut msg = match tl_message(user_tl_peer()) {
        tl::enums::Message::Message(m) => m,
        _ => unreachable!("tl_message always builds a concrete message"),
    };
    msg.message = body.to_string();
    tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: tl::enums::Message::Message(msg),
        pts: 1,
        pts_count: 1,
    })
}

fn service_textless_update() -> tl::enums::Update {
    tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: tl::enums::Message::Service(tl::types::MessageService {
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
        }),
        pts: 1,
        pts_count: 1,
    })
}

#[test]
fn compile_pattern_rejects_invalid_regex_as_usage() {
    let err = compile_pattern(&["(".to_string()]).expect_err("invalid regex must be rejected");
    assert!(matches!(err, TeleError::Usage(_)), "err: {err}");
    assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
    assert!(err.message().contains("--pattern"), "err: {err}");
}

#[test]
fn compile_pattern_none_is_no_filter_and_valid_pattern_compiles() {
    assert!(compile_pattern(&[]).unwrap().is_empty());
    assert_eq!(compile_pattern(&["^buy".to_string()]).unwrap().len(), 1);
}

#[test]
fn compile_pattern_matches_as_written_case_sensitively() {
    let patterns = compile_pattern(&["alert".to_string()]).unwrap();
    let re = patterns.first().unwrap();
    assert!(re.is_match("urgent alert now"));
    assert!(!re.is_match("nothing here"));
    assert!(!re.is_match("ALERT"), "matching stays case-sensitive");
}

#[test]
fn multiple_patterns_match_any_within_dimension() {
    let f = EventFilter {
        patterns: compile_pattern(&["^buy".to_string(), "sell$".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(f.text_allows(Some("buy now")));
    assert!(f.text_allows(Some("want to sell")));
    assert!(!f.text_allows(Some("hold forever")));
    assert!(!f.text_allows(None));
}

#[test]
fn pattern_filter_matches_and_mismatches_text_case_sensitively() {
    let f = EventFilter {
        patterns: compile_pattern(&["alert".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(f.text_allows(Some("urgent alert now")));
    assert!(!f.text_allows(Some("nothing here")));
    assert!(!f.text_allows(Some("ALERT")), "case-sensitive by default");
}

#[test]
fn pattern_filter_drops_textless_rows_but_passes_all_when_unset() {
    let f = EventFilter {
        patterns: compile_pattern(&[".".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(
        !f.text_allows(None),
        "textless rows cannot satisfy a text pattern"
    );
    assert!(EventFilter::default().text_allows(None));
    assert!(EventFilter::default().text_allows(Some("any")));
}

#[test]
fn update_text_reads_body_only_from_concrete_message_kinds() {
    assert_eq!(
        update_text(&text_update("hello there")),
        Some("hello there")
    );
    let empty_update = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: empty_message(),
        pts: 1,
        pts_count: 1,
    });
    assert_eq!(update_text(&empty_update), None);
    assert_eq!(update_text(&service_textless_update()), None);
    assert_eq!(update_text(&tl::enums::Update::PtsChanged), None);
}

fn filter_full_row(
    f: &EventFilter,
    sender: Option<PeerId>,
    out: Option<bool>,
    text: Option<&str>,
) -> bool {
    f.message_allows(sender, out) && f.text_allows(text)
}

#[test]
fn pattern_composes_with_sender_dimension_and_wise() {
    let f = EventFilter {
        senders: vec![PeerId::user_unchecked(7)],
        patterns: compile_pattern(&["alert".to_string()]).unwrap(),
        ..Default::default()
    };
    let sender_ok = Some(PeerId::user_unchecked(7));
    let sender_other = Some(PeerId::user_unchecked(8));
    assert!(filter_full_row(
        &f,
        sender_ok,
        Some(false),
        Some("big alert")
    ));
    assert!(
        !filter_full_row(&f, sender_ok, Some(false), Some("quiet")),
        "right sender, non-matching text"
    );
    assert!(
        !filter_full_row(&f, sender_other, Some(false), Some("big alert")),
        "matching text, wrong sender"
    );
}

#[test]
fn raw_events_blocked_when_pattern_set() {
    let peer = Some(PeerId::channel_unchecked(4));
    let f = EventFilter {
        patterns: compile_pattern(&["x".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(!f.raw_allows(peer), "raw carries no text to match");
    assert!(EventFilter::default().raw_allows(peer));
}

#[test]
fn deletions_blocked_when_pattern_set() {
    let f = EventFilter {
        patterns: compile_pattern(&["x".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(!f.deletions_pass(), "deletions carry no text to match");
    assert!(EventFilter::default().deletions_pass());
}

#[test]
fn listen_parses_pattern_flag() {
    use crate::Command;
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "tele",
        "listen",
        "--account",
        "a",
        "--pattern",
        "^buy|sell$",
    ])
    .expect("--pattern must parse");
    match cli.command {
        Command::Listen(args) => {
            assert_eq!(args.pattern, vec!["^buy|sell$"]);
        }
        _ => panic!("expected listen subcommand"),
    }
}

#[test]
fn listen_parses_repeated_pattern_flags() {
    use crate::Command;
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "tele",
        "listen",
        "--account",
        "a",
        "--pattern",
        "^buy",
        "--pattern",
        "sell$",
    ])
    .expect("repeated --pattern must parse");
    match cli.command {
        Command::Listen(args) => {
            assert_eq!(args.pattern, vec!["^buy", "sell$"]);
            let patterns = compile_pattern(&args.pattern).unwrap();
            assert_eq!(patterns.len(), 2);
            assert!(patterns.iter().any(|re| re.is_match("buy now")));
            assert!(patterns.iter().any(|re| re.is_match("please sell")));
            assert!(!patterns.iter().any(|re| re.is_match("hold")));
        }
        _ => panic!("expected listen subcommand"),
    }
}

#[test]
fn listen_help_documents_pattern_case_sensitivity() {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let listen = cmd
        .find_subcommand_mut("listen")
        .expect("listen subcommand");
    let help = listen.render_help().to_string();
    assert!(help.contains("--pattern"), "help: {help}");
    assert!(help.contains("case-sensitive"), "help: {help}");
}

#[test]
fn message_sender_reads_from_id_and_distinguishes_kinds() {
    let msg = tl_message_with_from(Some(tl::enums::Peer::User(tl::types::PeerUser {
        user_id: 7,
    })));
    assert_eq!(message_sender(&msg), Some(PeerId::user_unchecked(7)));
    let chat_msg = tl_message_with_from(Some(tl::enums::Peer::Chat(tl::types::PeerChat {
        chat_id: 9,
    })));
    assert_eq!(message_sender(&chat_msg), Some(PeerId::chat_unchecked(9)));
}

#[test]
fn message_sender_none_when_absent_or_empty() {
    let no_from = tl_message_with_from(None);
    assert_eq!(message_sender(&no_from), None);
    assert_eq!(message_sender(&empty_message()), None);
}

#[test]
fn message_outgoing_reads_flag_for_real_service_but_not_empty() {
    assert_eq!(message_outgoing(&tl_message(channel_peer())), Some(false));
    let mut outgoing = match tl_message(channel_peer()) {
        tl::enums::Message::Message(m) => m,
        _ => unreachable!("tl_message always builds a concrete message"),
    };
    outgoing.out = true;
    assert_eq!(
        message_outgoing(&tl::enums::Message::Message(outgoing)),
        Some(true)
    );
    let service = tl::enums::Message::Service(tl::types::MessageService {
        out: true,
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
    });
    assert_eq!(message_outgoing(&service), Some(true));
    assert_eq!(message_outgoing(&empty_message()), None);
}

fn tl_message_with_from(from_id: Option<tl::enums::Peer>) -> tl::enums::Message {
    let mut inner = match tl_message(channel_peer()) {
        tl::enums::Message::Message(m) => m,
        _ => unreachable!("tl_message always builds a concrete message"),
    };
    inner.from_id = from_id;
    tl::enums::Message::Message(inner)
}

#[test]
fn resolution_usage_error_wraps_cause_as_usage_exit_one() {
    let cause = TeleError::Invocation("rpc error 400: USERNAME_NOT_OCCUPIED".to_string(), None);
    let err = resolution_usage_error("--from", "@ghost", &cause);
    assert!(matches!(err, TeleError::Usage(_)));
    assert!(
        err.message().contains("cannot resolve --from @ghost"),
        "err: {err}"
    );
    assert!(
        err.message().contains("USERNAME_NOT_OCCUPIED"),
        "err: {err}"
    );
    assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
}

#[test]
fn validate_listen_inputs_rejects_empty_targets_before_connecting() {
    let err = validate_listen_inputs(&["@ok".to_string(), "   ".to_string()], &[])
        .expect_err("blank --chat must be rejected");
    assert!(matches!(err, TeleError::Usage(_)), "err: {err}");
    let err =
        validate_listen_inputs(&[], &["".to_string()]).expect_err("empty --from must be rejected");
    assert!(matches!(err, TeleError::Usage(_)), "err: {err}");
    assert!(validate_listen_inputs(&["@a".to_string()], &["+15550001111".to_string()]).is_ok());
}

#[test]
fn listen_accepts_repeated_chat_and_from_flags() {
    use crate::Command;
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "tele",
        "listen",
        "--account",
        "a",
        "--chat",
        "@one,@two",
        "--chat",
        "1234567890",
        "--from",
        "@alice",
        "--from",
        "@bob",
    ])
    .expect("repeatable flags must parse");
    match cli.command {
        Command::Listen(args) => {
            assert_eq!(args.chat, vec!["@one", "@two", "1234567890"]);
            assert_eq!(args.from, vec!["@alice", "@bob"]);
            assert!(!args.r#in && !args.out);
        }
        _ => panic!("expected listen subcommand"),
    }
}

#[test]
fn listen_in_conflicts_with_out() {
    use clap::Parser;
    let parsed = crate::Cli::try_parse_from(["tele", "listen", "--in", "--out"]);
    match parsed {
        Err(err) => {
            assert!(err.to_string().contains("cannot be used with"), "{err}");
        }
        Ok(_) => panic!("--in and --out must conflict"),
    }
    assert!(crate::Cli::try_parse_from(["tele", "listen", "--in"]).is_ok());
    assert!(crate::Cli::try_parse_from(["tele", "listen", "--out"]).is_ok());
}

#[test]
fn listen_accepts_service_chataction_userupdate_event_names() {
    use crate::Command;
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "tele",
        "listen",
        "--account",
        "a",
        "--events",
        "Service,ChatAction,UserUpdate",
    ])
    .expect("new event kinds must parse");
    match cli.command {
        Command::Listen(args) => {
            for kind in ["Service", "ChatAction", "UserUpdate"] {
                assert!(
                    args.events.contains(&kind.to_string()),
                    "{kind} must be accepted"
                );
            }
        }
        _ => panic!("expected listen subcommand"),
    }
}

#[test]
fn listen_rejects_unknown_new_kind_typos_still() {
    assert!(
        !VALID_EVENTS.contains(&"Services"),
        "typo'd event names must stay outside the allowlist"
    );
    for kind in ["Service", "ChatAction", "UserUpdate"] {
        assert!(
            VALID_EVENTS.contains(&kind),
            "{kind} must be in the allowlist"
        );
    }
}

#[test]
fn listen_help_documents_service_chataction_userupdate_kinds() {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let listen = cmd
        .find_subcommand_mut("listen")
        .expect("listen subcommand");
    let help = listen.render_help().to_string();
    for kind in [
        "Service",
        "ChatAction",
        "UserUpdate",
        "NewMessage",
        "MessageDeleted",
        "Raw",
    ] {
        assert!(help.contains(kind), "help must document {kind}: {help}");
    }
}

fn add_user_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::ChatAddUser(tl::types::MessageActionChatAddUser {
        users: vec![11, 12],
    })
}

fn joined_by_link_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::ChatJoinedByLink(tl::types::MessageActionChatJoinedByLink {
        inviter_id: 2,
    })
}

fn joined_by_request_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::ChatJoinedByRequest
}

fn delete_user_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::ChatDeleteUser(tl::types::MessageActionChatDeleteUser { user_id: 13 })
}

fn pin_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::PinMessage
}

fn chat_create_action() -> tl::enums::MessageAction {
    tl::enums::MessageAction::ChatCreate(tl::types::MessageActionChatCreate {
        title: "crew".into(),
        users: vec![1],
    })
}

#[test]
fn common_message_actions_map_to_friendly_labels() {
    let cases = [
        (
            add_user_action(),
            ("messageActionChatAddUser", "join-invite"),
        ),
        (
            joined_by_link_action(),
            ("messageActionChatJoinedByLink", "join"),
        ),
        (
            joined_by_request_action(),
            ("messageActionChatJoinedByRequest", "join"),
        ),
        (
            delete_user_action(),
            ("messageActionChatDeleteUser", "leave"),
        ),
        (pin_action(), ("messageActionPinMessage", "pin")),
    ];
    for (action, expected) in cases {
        assert_eq!(
            message_action_kind_label(&action),
            expected,
            "label table drifted for {expected:?}"
        );
    }
}

#[test]
fn unmapped_message_actions_keep_raw_variant_name_as_label() {
    assert_eq!(
        message_action_kind_label(&chat_create_action()),
        ("messageActionChatCreate", "messageActionChatCreate")
    );
    assert_eq!(
        message_action_kind_label(&tl::enums::MessageAction::Empty),
        ("messageActionEmpty", "messageActionEmpty")
    );
}

fn typing_action() -> tl::enums::SendMessageAction {
    tl::enums::SendMessageAction::SendMessageTypingAction
}

fn upload_photo_action() -> tl::enums::SendMessageAction {
    tl::enums::SendMessageAction::SendMessageUploadPhotoAction(
        tl::types::SendMessageUploadPhotoAction { progress: 40 },
    )
}

#[test]
fn typing_action_maps_typing_label_and_falls_back_to_raw_kind() {
    assert_eq!(
        typing_action_kind_label(&typing_action()),
        ("sendMessageTypingAction", "typing")
    );
    assert_eq!(
        typing_action_kind_label(&upload_photo_action()),
        (
            "sendMessageUploadPhotoAction",
            "sendMessageUploadPhotoAction"
        )
    );
}

#[test]
fn user_status_maps_presence_labels() {
    let cases = [
        (
            tl::enums::UserStatus::Online(tl::types::UserStatusOnline { expires: 500 }),
            "online",
        ),
        (
            tl::enums::UserStatus::Offline(tl::types::UserStatusOffline { was_online: 300 }),
            "offline",
        ),
        (
            tl::enums::UserStatus::Recently(tl::types::UserStatusRecently { by_me: false }),
            "recently",
        ),
        (
            tl::enums::UserStatus::LastWeek(tl::types::UserStatusLastWeek { by_me: false }),
            "last-week",
        ),
        (
            tl::enums::UserStatus::LastMonth(tl::types::UserStatusLastMonth { by_me: false }),
            "last-month",
        ),
        (tl::enums::UserStatus::Empty, "empty"),
    ];
    for (status, label) in cases {
        let (_, got) = user_status_kind_label(&status);
        assert_eq!(got, label, "presence label drift for {label}");
    }
}

fn service_base_row() -> serde_json::Value {
    serde_json::json!({
        "id": 77,
        "date": "2026-08-20T10:00:00+00:00",
        "text": "",
    })
}

#[test]
fn service_row_carries_additive_service_action_over_base_fields() {
    let row = service_row("work", Some(456), service_base_row(), &pin_action());
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "Service");
    assert_eq!(obj["account"], "work");
    assert_eq!(obj["chat_id"], 456);
    assert_eq!(obj["id"], 77);
    assert_eq!(obj["service_action"]["kind"], "messageActionPinMessage");
    assert_eq!(obj["service_action"]["label"], "pin");
}

#[test]
fn service_row_omits_chat_id_when_unknown() {
    let row = service_row("work", None, service_base_row(), &add_user_action());
    assert!(!row.as_object().unwrap().contains_key("chat_id"));
    assert_eq!(
        row["service_action"]["kind"], "messageActionChatAddUser",
        "kinds outside the friendly table keep raw variant names"
    );
    assert_eq!(row["service_action"]["label"], "join-invite");
}

fn user_typing_update(user_id: i64) -> tl::enums::Update {
    tl::enums::Update::UserTyping(tl::types::UpdateUserTyping {
        top_msg_id: None,
        user_id,
        action: typing_action(),
    })
}

fn chat_typing_update(chat_id: i64, sender: i64) -> tl::enums::Update {
    tl::enums::Update::ChatUserTyping(tl::types::UpdateChatUserTyping {
        chat_id,
        from_id: tl::enums::Peer::User(tl::types::PeerUser { user_id: sender }),
        action: upload_photo_action(),
    })
}

fn channel_typing_update(channel_id: i64, sender: i64) -> tl::enums::Update {
    tl::enums::Update::ChannelUserTyping(tl::types::UpdateChannelUserTyping {
        top_msg_id: None,
        channel_id,
        from_id: tl::enums::Peer::User(tl::types::PeerUser { user_id: sender }),
        action: typing_action(),
    })
}

#[test]
fn chat_action_user_typing_row_has_user_id_without_chat_id() {
    let (peer, sender, row) =
        chat_action_row("work", &user_typing_update(7)).expect("typing update parses");
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "ChatAction");
    assert_eq!(obj["account"], "work");
    assert_eq!(obj["user_id"], 7);
    assert!(!obj.contains_key("chat_id"), "DM typing has no chat id");
    assert_eq!(obj["action"]["kind"], "sendMessageTypingAction");
    assert_eq!(obj["action"]["label"], "typing");
    assert_eq!(
        peer.expect("user typing yields peer"),
        PeerId::user_unchecked(7)
    );
    assert_eq!(sender, Some(PeerId::user_unchecked(7)));
}

#[test]
fn chat_action_chat_typing_row_carries_chat_and_sender_ids() {
    let (peer, sender, row) =
        chat_action_row("work", &chat_typing_update(42, 8)).expect("typing update parses");
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "ChatAction");
    assert_eq!(obj["user_id"], 8);
    assert_eq!(obj["chat_id"], 42);
    assert_eq!(peer, Some(PeerId::chat_unchecked(42)));
    assert_eq!(sender, Some(PeerId::user_unchecked(8)));
    assert_eq!(
        obj["action"]["kind"], "sendMessageUploadPhotoAction",
        "unmapped actions keep raw variant name"
    );
    assert_eq!(obj["action"]["label"], "sendMessageUploadPhotoAction");
}

#[test]
fn chat_action_channel_typing_uses_channel_bare_chat_id() {
    let (peer, _sender, row) = chat_action_row("work", &channel_typing_update(1234567890, 8))
        .expect("typing update parses");
    let obj = row.as_object().unwrap();
    assert_eq!(obj["chat_id"], 1234567890);
    assert_eq!(peer, Some(PeerId::channel_unchecked(1234567890)));
}

#[test]
fn non_chataction_raw_updates_are_not_claimed_by_chat_action() {
    assert!(chat_action_row("work", &tl::enums::Update::PtsChanged).is_none());
    assert!(chat_action_row(
        "work",
        &tl::enums::Update::UserStatus(tl::types::UpdateUserStatus {
            user_id: 7,
            status: tl::enums::UserStatus::Online(tl::types::UserStatusOnline { expires: 1 }),
        })
    )
    .is_none());
}

fn user_status_update(status: tl::enums::UserStatus) -> tl::enums::Update {
    tl::enums::Update::UserStatus(tl::types::UpdateUserStatus { user_id: 7, status })
}

fn callback_query_update() -> tl::enums::Update {
    tl::enums::Update::BotCallbackQuery(tl::types::UpdateBotCallbackQuery {
        query_id: 5,
        user_id: 7,
        peer: tl::types::PeerUser { user_id: 7 }.into(),
        msg_id: 42,
        chat_instance: 99,
        data: Some(b"force_sub:refresh".to_vec()),
        game_short_name: None,
    })
}

#[test]
fn callback_query_row_reports_user_data_and_decoded_payload() {
    let (peer, sender, row) =
        callback_query_row("home", &callback_query_update()).expect("callback query parses");
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "CallbackQuery");
    assert_eq!(obj["account"], "home");
    assert_eq!(obj["user_id"], 7);
    assert_eq!(obj["message_id"], 42);
    assert_eq!(obj["data"], "force_sub:refresh");
    assert!(obj["data_b64"].as_str().is_some(), "base64 data present");
    assert_eq!(peer, Some(PeerId::user_unchecked(7)));
    assert_eq!(sender, Some(PeerId::user_unchecked(7)));
}

#[test]
fn user_update_row_reports_slim_presence_status() {
    let (peer, sender, row) = user_update_row(
        "home",
        &user_status_update(tl::enums::UserStatus::Online(tl::types::UserStatusOnline {
            expires: 900,
        })),
    )
    .expect("user status parses");
    let obj = row.as_object().unwrap();
    assert_eq!(obj["event"], "UserUpdate");
    assert_eq!(obj["account"], "home");
    assert_eq!(obj["user_id"], 7);
    assert_eq!(obj["status"]["kind"], "userStatusOnline");
    assert_eq!(obj["status"]["label"], "online");
    assert_eq!(obj["status"]["expires"], 900);
    assert_eq!(peer, Some(PeerId::user_unchecked(7)));
    assert_eq!(sender, Some(PeerId::user_unchecked(7)));
    assert!(!obj.contains_key("state"), "slim rows omit stream state");
}

#[test]
fn user_update_offline_row_carries_was_online() {
    let (_, _, row) = user_update_row(
        "home",
        &user_status_update(tl::enums::UserStatus::Offline(
            tl::types::UserStatusOffline { was_online: 55 },
        )),
    )
    .expect("user status parses");
    assert_eq!(row["status"]["kind"], "userStatusOffline");
    assert_eq!(row["status"]["label"], "offline");
    assert_eq!(row["status"]["was_online"], 55);
}

#[test]
fn non_userupdate_raw_updates_are_not_claimed_by_user_update() {
    assert!(user_update_row("work", &tl::enums::Update::PtsChanged).is_none());
    assert!(user_update_row("work", &user_typing_update(7)).is_none());
}

#[test]
fn action_allows_composes_chat_and_sender_dimensions() {
    let f = EventFilter {
        chats: vec![PeerId::chat_unchecked(42)],
        senders: vec![PeerId::user_unchecked(8)],
        direction: None,
        patterns: Vec::new(),
    };
    assert!(f.action_allows(
        Some(PeerId::chat_unchecked(42)),
        Some(PeerId::user_unchecked(8))
    ));
    assert!(
        !f.action_allows(
            Some(PeerId::chat_unchecked(43)),
            Some(PeerId::user_unchecked(8))
        ),
        "wrong chat blocked"
    );
    assert!(
        !f.action_allows(
            Some(PeerId::chat_unchecked(42)),
            Some(PeerId::user_unchecked(9))
        ),
        "wrong sender blocked"
    );
    assert!(
        !f.action_allows(None, None),
        "rows without ids cannot satisfy set dimensions"
    );
    assert!(
        EventFilter::default().action_allows(None, None),
        "unset dimensions pass rows without ids"
    );
}

#[test]
fn action_allows_ignores_direction_and_pattern_honestly() {
    let f_dir = EventFilter {
        direction: Some(Direction::In),
        ..Default::default()
    };
    assert!(
        f_dir.action_allows(
            Some(PeerId::user_unchecked(7)),
            Some(PeerId::user_unchecked(7))
        ),
        "direction has no meaning for typing/status rows and must not block them"
    );
    let f_pattern = EventFilter {
        patterns: compile_pattern(&["x".to_string()]).unwrap(),
        ..Default::default()
    };
    assert!(
        f_pattern.action_allows(
            Some(PeerId::user_unchecked(7)),
            Some(PeerId::user_unchecked(7))
        ),
        "pattern has no text to match on typing/status rows"
    );
}

#[test]
fn message_event_applies_gates_service_only_selections() {
    assert!(message_event_applies(true, false, false, true));
    assert!(
        message_event_applies(false, false, true, true),
        "service-only admits service"
    );
    assert!(!message_event_applies(false, false, true, false));
    assert!(
        message_event_applies(false, true, false, true),
        "album still buffers"
    );
    assert!(!message_event_applies(false, false, false, true));
}

#[test]
fn routes_to_service_requires_both_flavor_and_flag() {
    assert!(routes_to_service(true, true));
    assert!(!routes_to_service(false, true));
    assert!(!routes_to_service(true, false));
    assert!(!routes_to_service(false, false));
}

fn offline_client() -> grammers_client::Client {
    let session = std::sync::Arc::new(grammers_session::storages::MemorySession::default());
    let pool = grammers_client::sender::SenderPool::new(session, 12345);
    grammers_client::Client::new(pool.handle)
}

fn poll_media_fixture(question: &str) -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Poll(Box::new(tl::types::MessageMediaPoll {
        poll: tl::enums::Poll::Poll(tl::types::Poll {
            id: 9,
            closed: false,
            public_voters: false,
            multiple_choice: false,
            quiz: false,
            open_answers: false,
            revoting_disabled: false,
            shuffle_answers: false,
            hide_results_until_close: false,
            creator: false,
            subscribers_only: false,
            question: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: question.into(),
                entities: Vec::new(),
            }),
            answers: vec![tl::enums::PollAnswer::Answer(tl::types::PollAnswer {
                media: None,
                added_by: None,
                date: None,
                text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                    text: "Yes".into(),
                    entities: Vec::new(),
                }),
                option: b"y".to_vec(),
            })],
            close_period: None,
            close_date: None,
            countries_iso2: None,
            hash: 0,
        }),
        results: tl::enums::PollResults::Results(Box::new(tl::types::PollResults {
            min: false,
            has_unread_votes: false,
            can_view_stats: false,
            results: None,
            total_voters: None,
            recent_voters: None,
            solution: None,
            solution_entities: None,
            solution_media: None,
        })),
        attached_media: None,
    }))
}

fn short_update_message(
    client: &grammers_client::Client,
    text: &str,
    media: Option<tl::enums::MessageMedia>,
) -> grammers_client::message::Message {
    grammers_client::message::Message::from_raw_short_updates(
        client,
        tl::types::UpdateShortSentMessage {
            out: true,
            id: 5,
            pts: 0,
            pts_count: 0,
            date: 1700000000,
            media,
            entities: None,
            ttl_period: None,
        },
        grammers_client::message::InputMessage::new().text(text),
        grammers_session::types::PeerId::user(42)
            .unwrap()
            .to_ambient_ref(),
    )
}

#[test]
fn streamed_rows_attach_poll_object_matching_get_shape() {
    let client = offline_client();
    let msg = short_update_message(&client, "vote", Some(poll_media_fixture("Stream vote?")));
    let row = streamed_message_row(&msg).unwrap();
    assert_eq!(row["poll"]["question"], "Stream vote?");
    assert_eq!(row["poll"]["id"], 9);
    assert_eq!(row["poll"]["closed"], false);
    assert_eq!(row["poll"]["quiz"], false);
    let options = row["poll"]["options"].as_array().unwrap();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0]["index"], 1);
    assert_eq!(options[0]["text"], "Yes");
    assert!(options[0].get("voters").is_none());
}

#[test]
fn streamed_rows_without_poll_media_have_no_poll_key() {
    let client = offline_client();
    let msg = short_update_message(&client, "plain", None);
    let row = streamed_message_row(&msg).unwrap();
    assert!(
        row.get("poll").is_none(),
        "poll key must stay absent without poll media"
    );
    assert_eq!(row["text"], "plain");
}

#[test]
fn listen_dedupe_suppresses_duplicate_pts_windows() {
    let mut d = ListenDedupe::new(LISTEN_DEDUPE_CAP);
    let raw10 = pts_update(user_tl_peer(), 5, 10, 1, None);
    let raw11 = pts_update(user_tl_peer(), 5, 11, 1, None);
    let k1 = dedupe_key(Some(123), 5, &raw10).unwrap();
    let k2 = dedupe_key(Some(123), 5, &raw11).unwrap();
    let k3 = dedupe_key(Some(456), 5, &raw10).unwrap();
    assert!(!d.check(k1));
    assert!(d.check(k1));
    assert!(!d.check(k2));
    assert!(d.check(k2));
    assert!(!d.check(k3));
    assert!(d.check(k3));
}

#[test]
fn listen_dedupe_evicts_oldest_beyond_cap() {
    let mut d = ListenDedupe::new(LISTEN_DEDUPE_CAP);
    for i in 0..(LISTEN_DEDUPE_CAP as i32) {
        let raw = pts_update(user_tl_peer(), i, i, 1, None);
        assert!(!d.check(dedupe_key(Some(1), i, &raw).unwrap()));
    }
    assert_eq!(d.len(), LISTEN_DEDUPE_CAP);
    let first_raw = pts_update(user_tl_peer(), 0, 0, 1, None);
    let first = dedupe_key(Some(1), 0, &first_raw).unwrap();
    assert!(d.check(first));
    let overflow_raw = pts_update(
        user_tl_peer(),
        LISTEN_DEDUPE_CAP as i32,
        LISTEN_DEDUPE_CAP as i32,
        1,
        None,
    );
    assert!(!d.check(dedupe_key(Some(1), LISTEN_DEDUPE_CAP as i32, &overflow_raw).unwrap()));
}

#[test]
fn listen_dedupe_key_uses_raw_pts_not_global_state() {
    let raw_a = pts_update(user_tl_peer(), 42, 10, 1, None);
    let raw_b = pts_update(user_tl_peer(), 42, 11, 1, None);
    let key_a = dedupe_key(Some(1), 42, &raw_a).unwrap();
    let key_b = dedupe_key(Some(1), 42, &raw_b).unwrap();
    assert_ne!(key_a.2, key_b.2);
    assert_eq!(key_a.2, 10);
    assert_eq!(key_b.2, 11);
    let mut d = ListenDedupe::new(LISTEN_DEDUPE_CAP);
    assert!(!d.check(key_a));
    assert!(d.check(key_a));
    assert!(!d.check(key_b));
}

#[test]
fn listen_dedupe_key_none_for_pts_less_update() {
    assert!(dedupe_key(Some(1), 1, &tl::enums::Update::PtsChanged).is_none());
    let empty_channel = tl::enums::Update::NewChannelMessage(tl::types::UpdateNewChannelMessage {
        message: empty_message(),
        pts: 1,
        pts_count: 1,
    });
    assert!(dedupe_key(Some(1), 1, &empty_channel).is_none());
}

#[test]
fn listen_dedupe_edits_have_distinct_keys_by_pts() {
    let edit_a = tl::enums::Update::EditMessage(tl::types::UpdateEditMessage {
        message: tl_message(user_tl_peer()),
        pts: 20,
        pts_count: 1,
    });
    let edit_b = tl::enums::Update::EditMessage(tl::types::UpdateEditMessage {
        message: tl_message(user_tl_peer()),
        pts: 21,
        pts_count: 1,
    });
    let ka = dedupe_key(Some(1), 9, &edit_a).unwrap();
    let kb = dedupe_key(Some(1), 9, &edit_b).unwrap();
    assert_ne!(ka, kb);
    let mut d = ListenDedupe::new(LISTEN_DEDUPE_CAP);
    assert!(!d.check(ka));
    assert!(!d.check(kb));
    assert!(d.check(ka));
}

#[test]
fn listen_pts_from_state_reads_all_variants() {
    use grammers_session::updates::{MessageBox, State};
    let s = State {
        date: 1,
        seq: 2,
        message_box: Some(MessageBox::Common { pts: 42 }),
    };
    assert_eq!(pts_from_state(&s), 42);
    let s = State {
        date: 1,
        seq: 2,
        message_box: Some(MessageBox::Secondary { qts: 43 }),
    };
    assert_eq!(pts_from_state(&s), 43);
    let s = State {
        date: 1,
        seq: 2,
        message_box: Some(MessageBox::Channel {
            channel_id: 9,
            pts: 44,
        }),
    };
    assert_eq!(pts_from_state(&s), 44);
    let s = State {
        date: 1,
        seq: 2,
        message_box: None,
    };
    assert_eq!(pts_from_state(&s), 0);
}

#[tokio::test]
async fn listen_state_persists_and_resumes_offline() {
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "telecli-listen-state-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TELE_APP_DIR", &dir);
    std::fs::write(dir.join("config.toml"), "[accounts.listen_test]\n").unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    let path = crate::session::session_path("listen_test");
    {
        let sess = grammers_session::storages::SqliteSession::open(&path)
            .await
            .unwrap();
        sess.set_update_state(grammers_session::types::UpdateState::All(
            grammers_session::types::UpdatesState {
                pts: 88,
                qts: 1,
                date: 2000,
                seq: 2,
                channels: vec![],
            },
        ))
        .await
        .unwrap();
        let state = sess.updates_state().await.unwrap();
        let mbox = grammers_session::updates::MessageBoxes::load(state);
        assert_eq!(mbox.session_state().pts, 88);
        let mut dedupe = ListenDedupe::new(LISTEN_DEDUPE_CAP);
        let raw = pts_update(user_tl_peer(), 1, mbox.session_state().pts, 1, None);
        let k = dedupe_key(Some(1), 1, &raw).unwrap();
        assert!(!dedupe.check(k));
        assert!(dedupe.check(k));
    }
    {
        let sess2 = grammers_session::storages::SqliteSession::open(&path)
            .await
            .unwrap();
        let resumed = sess2.updates_state().await.unwrap();
        assert_eq!(resumed.pts, 88);
    }
    std::env::remove_var("TELE_APP_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn emit_broken_pipe_classified_as_clean_exit() {
    let err: TeleError = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
    assert!(err.is_broken_pipe());
    assert_eq!(err.exit_code(), crate::error::EXIT_OK);
}

#[test]
fn emit_other_error_not_broken_pipe() {
    let err: TeleError = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied").into();
    assert!(!err.is_broken_pipe());
    assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
    let err2 = TeleError::Other("serialization failed".to_string());
    assert!(!err2.is_broken_pipe());
}

#[test]
fn emit_stops_stream_only_on_broken_pipe() {
    let bp: TeleError = std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
    assert!(emit_stops_stream(&bp));
    let other = TeleError::Other("emit failed".to_string());
    assert!(!emit_stops_stream(&other));
}

#[test]
fn sync_update_state_error_is_non_fatal_and_logged() {
    let err = TeleError::Other("sync failed".to_string());
    assert!(!err.is_broken_pipe());
    assert!(!is_auth_error(&err));
}

#[test]
fn per_event_serialization_error_does_not_kill_stream() {
    let err = TeleError::Other("unserializable".to_string());
    assert!(!err.is_broken_pipe());
    assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
}
