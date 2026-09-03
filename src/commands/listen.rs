use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use crate::client::{self, ClientGuard};
use crate::error::{TeleError, TeleResult};
use crate::executor::{require_explicit_selection, GlobalFlags};
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
        help = "event types: NewMessage, MessageEdited, MessageDeleted, Raw, Album, Gap, Service, ChatAction, UserUpdate, CallbackQuery (ChatAction/UserUpdate/CallbackQuery are parsed out of grammers' raw-update wrapper)"
    )]
    events: Vec<String>,
    #[arg(
        long,
        help = "also emit raw TL updates alongside the parsed event allowlist"
    )]
    raw: bool,
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "CHAT",
        help = "only show events from these chats (repeatable or comma-separated)"
    )]
    chat: Vec<String>,
    #[arg(
        long,
        value_name = "USER",
        help = "only show events sent by these users (repeatable)"
    )]
    from: Vec<String>,
    #[arg(
        long,
        conflicts_with = "out",
        help = "only incoming messages (message events only)"
    )]
    r#in: bool,
    #[arg(long, help = "only outgoing messages (message events only)")]
    out: bool,
    #[arg(
        long,
        value_name = "RE",
        action = clap::ArgAction::Append,
        help = "only messages whose text matches one of these Rust regexes, case-sensitive (repeatable; message events only)"
    )]
    pattern: Vec<String>,
}
const VALID_EVENTS: &[&str] = &[
    "NewMessage",
    "MessageEdited",
    "MessageDeleted",
    "Raw",
    "Album",
    "Gap",
    "Service",
    "ChatAction",
    "UserUpdate",
    "CallbackQuery",
];
const MAX_RECONNECT_BACKOFF: u32 = 30;
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
const ALBUM_FLUSH_MILLIS: u64 = 500;
const MAX_ALBUM_LEN: usize = 20;
const ALBUM_BUFFER_CAP: usize = 20;
const OBSERVED_PEER_CAP: usize = 10_000;
const GAP_TRACKER_CAP: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    In,
    Out,
}

#[derive(Clone, Debug, Default)]
struct EventFilter {
    chats: Vec<PeerId>,
    senders: Vec<PeerId>,
    direction: Option<Direction>,
    patterns: Vec<regex::Regex>,
}

impl EventFilter {
    fn chat_allows(&self, peer: Option<PeerId>) -> bool {
        match self.chats.as_slice() {
            [] => true,
            targets => peer.is_some_and(|p| targets.contains(&p)),
        }
    }

    fn message_allows(&self, sender: Option<PeerId>, out: Option<bool>) -> bool {
        if !self.sender_allows(sender) {
            return false;
        }
        self.direction_allows(out)
    }

    fn text_allows(&self, text: Option<&str>) -> bool {
        match self.patterns.as_slice() {
            [] => true,
            patterns => text.is_some_and(|t| patterns.iter().any(|re| re.is_match(t))),
        }
    }

    fn sender_allows(&self, sender: Option<PeerId>) -> bool {
        match self.senders.as_slice() {
            [] => true,
            targets => sender.is_some_and(|s| targets.contains(&s)),
        }
    }

    fn direction_allows(&self, out: Option<bool>) -> bool {
        match self.direction {
            None => true,
            Some(Direction::In) => out == Some(false),
            Some(Direction::Out) => out == Some(true),
        }
    }

    fn raw_allows(&self, peer: Option<PeerId>) -> bool {
        self.senders.is_empty()
            && self.direction.is_none()
            && self.patterns.is_empty()
            && self.chat_allows(peer)
    }

    fn action_allows(&self, peer: Option<PeerId>, sender: Option<PeerId>) -> bool {
        self.chat_allows(peer) && self.sender_allows(sender)
    }

    fn deletions_pass(&self) -> bool {
        self.senders.is_empty() && self.direction.is_none() && self.patterns.is_empty()
    }
}

fn deletion_match_set(
    messages: &[i32],
    channel_id: Option<i64>,
    observed: &ObservedPeers,
    targets: &[PeerId],
) -> Option<Vec<i32>> {
    if targets.is_empty() {
        return Some(messages.to_vec());
    }
    let mut matched: Vec<i32> = Vec::new();
    for target in targets {
        let hits: Vec<i32> = if target.kind() == PeerKind::Channel {
            if deleted_matches(channel_id, *target) {
                messages.to_vec()
            } else {
                Vec::new()
            }
        } else {
            observed_deletion_ids(messages, observed, *target)
        };
        for id in hits {
            if !matched.contains(&id) {
                matched.push(id);
            }
        }
    }
    (!matched.is_empty()).then_some(matched)
}

fn sole_chat_label(targets: &[PeerId]) -> Option<i64> {
    match targets {
        [only] => only.bare_id(),
        _ => None,
    }
}

fn message_sender(msg: &tl::enums::Message) -> Option<PeerId> {
    match msg {
        tl::enums::Message::Message(m) => m.from_id.as_ref().map(PeerId::from),
        tl::enums::Message::Service(m) => m.from_id.as_ref().map(PeerId::from),
        tl::enums::Message::Empty(_) => None,
    }
}

fn message_outgoing(msg: &tl::enums::Message) -> Option<bool> {
    match msg {
        tl::enums::Message::Message(m) => Some(m.out),
        tl::enums::Message::Service(m) => Some(m.out),
        tl::enums::Message::Empty(_) => None,
    }
}

fn update_sender(u: &tl::enums::Update) -> Option<PeerId> {
    match u {
        tl::enums::Update::NewMessage(x) => message_sender(&x.message),
        tl::enums::Update::NewChannelMessage(x) => message_sender(&x.message),
        tl::enums::Update::EditMessage(x) => message_sender(&x.message),
        tl::enums::Update::EditChannelMessage(x) => message_sender(&x.message),
        _ => None,
    }
}

fn update_outgoing(u: &tl::enums::Update) -> Option<bool> {
    match u {
        tl::enums::Update::NewMessage(x) => message_outgoing(&x.message),
        tl::enums::Update::NewChannelMessage(x) => message_outgoing(&x.message),
        tl::enums::Update::EditMessage(x) => message_outgoing(&x.message),
        tl::enums::Update::EditChannelMessage(x) => message_outgoing(&x.message),
        _ => None,
    }
}

fn message_text(msg: &tl::enums::Message) -> Option<&str> {
    match msg {
        tl::enums::Message::Message(m) => Some(m.message.as_str()),
        _ => None,
    }
}

fn update_text(u: &tl::enums::Update) -> Option<&str> {
    match u {
        tl::enums::Update::NewMessage(x) => message_text(&x.message),
        tl::enums::Update::NewChannelMessage(x) => message_text(&x.message),
        tl::enums::Update::EditMessage(x) => message_text(&x.message),
        tl::enums::Update::EditChannelMessage(x) => message_text(&x.message),
        _ => None,
    }
}

fn compile_pattern(patterns: &[String]) -> TeleResult<Vec<regex::Regex>> {
    patterns
        .iter()
        .map(|p| {
            regex::Regex::new(p)
                .map_err(|e| TeleError::Usage(format!("invalid --pattern {p:?}: {e}")))
        })
        .collect()
}

fn resolution_usage_error(flag: &str, target: &str, cause: &TeleError) -> TeleError {
    TeleError::Usage(format!(
        "cannot resolve {flag} {target}: {}",
        cause.message()
    ))
}

fn validate_listen_inputs(chats: &[String], senders: &[String]) -> TeleResult<()> {
    for target in chats.iter().chain(senders.iter()) {
        if target.trim().is_empty() {
            return Err(TeleError::Usage(format!(
                "empty --chat/--from target: {target:?}"
            )));
        }
    }
    Ok(())
}

async fn resolve_filter(
    client: &grammers_client::Client,
    session: &grammers_session::storages::SqliteSession,
    chats: &[String],
    senders: &[String],
    direction: Option<Direction>,
) -> TeleResult<EventFilter> {
    let mut resolved_chats = Vec::with_capacity(chats.len());
    for target in chats {
        let peer = crate::entities::resolve_peer(client, session, target)
            .await
            .map_err(|e| resolution_usage_error("--chat", target, &e))?;
        resolved_chats.push(peer.id());
    }
    let mut resolved_senders = Vec::with_capacity(senders.len());
    for target in senders {
        let peer = crate::entities::resolve_peer(client, session, target)
            .await
            .map_err(|e| resolution_usage_error("--from", target, &e))?;
        resolved_senders.push(peer.id());
    }
    Ok(EventFilter {
        chats: resolved_chats,
        senders: resolved_senders,
        direction,
        patterns: Vec::new(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum StateBoxKey {
    Common,
    Channel(i64),
}

struct PtsPoint {
    box_key: StateBoxKey,
    pts: i32,
    pts_count: i32,
}

struct GapSignal {
    box_key: StateBoxKey,
    expected_pts: i32,
    observed_pts: i32,
}

struct GapTracker {
    last: crate::capped_map::CappedMap<StateBoxKey, (i32, i32)>,
}

impl Default for GapTracker {
    fn default() -> Self {
        GapTracker {
            last: crate::capped_map::CappedMap::new(GAP_TRACKER_CAP),
        }
    }
}

impl GapTracker {
    fn observe(&mut self, raw: &tl::enums::Update) -> Option<GapSignal> {
        let point = pts_point(raw)?;
        match self.last.get_mut(&point.box_key) {
            None => {
                self.last
                    .insert(point.box_key, (point.pts, point.pts_count.max(0)));
                None
            }
            Some(&mut (last_pts, last_count)) => {
                let expected = last_pts.saturating_add(last_count);
                let signal = (point.pts > expected).then_some(GapSignal {
                    box_key: point.box_key,
                    expected_pts: expected,
                    observed_pts: point.pts,
                });
                if point.pts > last_pts {
                    self.last
                        .insert(point.box_key, (point.pts, point.pts_count.max(0)));
                }
                signal
            }
        }
    }
}

fn pts_point(raw: &tl::enums::Update) -> Option<PtsPoint> {
    let common = |pts: i32, pts_count: i32| {
        Some(PtsPoint {
            box_key: StateBoxKey::Common,
            pts,
            pts_count,
        })
    };
    let channel_of_message = |message: &tl::enums::Message| -> Option<i64> {
        match message {
            tl::enums::Message::Message(m) => peer_channel_id(&m.peer_id),
            tl::enums::Message::Service(s) => peer_channel_id(&s.peer_id),
            tl::enums::Message::Empty(_) => None,
        }
    };
    match raw {
        tl::enums::Update::NewMessage(x) => common(x.pts, x.pts_count),
        tl::enums::Update::EditMessage(x) => common(x.pts, x.pts_count),
        tl::enums::Update::DeleteMessages(x) => common(x.pts, x.pts_count),
        tl::enums::Update::NewChannelMessage(x) => {
            channel_of_message(&x.message).map(|channel_id| PtsPoint {
                box_key: StateBoxKey::Channel(channel_id),
                pts: x.pts,
                pts_count: x.pts_count,
            })
        }
        tl::enums::Update::EditChannelMessage(x) => {
            channel_of_message(&x.message).map(|channel_id| PtsPoint {
                box_key: StateBoxKey::Channel(channel_id),
                pts: x.pts,
                pts_count: x.pts_count,
            })
        }
        tl::enums::Update::DeleteChannelMessages(x) => Some(PtsPoint {
            box_key: StateBoxKey::Channel(x.channel_id),
            pts: x.pts,
            pts_count: x.pts_count,
        }),
        _ => None,
    }
}

fn peer_channel_id(peer: &tl::enums::Peer) -> Option<i64> {
    match peer {
        tl::enums::Peer::Channel(c) => Some(c.channel_id),
        _ => None,
    }
}

fn gap_row(account: &str, gap: &GapSignal, state: &State) -> serde_json::Value {
    let mut row = event_row("Gap", account, None, None, None);
    if let serde_json::Value::Object(map) = &mut row {
        map.insert("reason".into(), serde_json::Value::from("pts_jump"));
        map.insert("expected_pts".into(), serde_json::json!(gap.expected_pts));
        map.insert("observed_pts".into(), serde_json::json!(gap.observed_pts));
        if let StateBoxKey::Channel(channel_id) = gap.box_key {
            map.insert("channel_id".into(), serde_json::json!(channel_id));
        }
        map.insert("state".into(), state_to_json(state));
    }
    row
}

fn album_member(row: &serde_json::Value) -> Option<(i64, i64)> {
    let grouped_id = row.get("grouped_id")?.as_i64()?;
    let chat_id = row.get("peer")?.get("id")?.as_i64()?;
    Some((chat_id, grouped_id))
}

struct PendingAlbum {
    chat_id: i64,
    grouped_id: i64,
    rows: Vec<serde_json::Value>,
    deadline: tokio::time::Instant,
}

type AlbumKey = (i64, i64);
type AlbumBuffer = HashMap<AlbumKey, PendingAlbum>;

fn album_ingest(
    account: &str,
    buf: &mut AlbumBuffer,
    row: serde_json::Value,
    chat_id: i64,
    grouped_id: i64,
    now: tokio::time::Instant,
) -> Vec<serde_json::Value> {
    let mut flushed = Vec::new();
    let key = (chat_id, grouped_id);
    if let Some(pending) = buf.get_mut(&key) {
        if pending.rows.len() >= MAX_ALBUM_LEN {
            flushed.push(album_complete(account, pending));
            buf.remove(&key);
        }
    } else if buf.len() >= ALBUM_BUFFER_CAP {
        if let Some(oldest) = buf
            .values()
            .min_by_key(|p| (p.deadline, p.chat_id, p.grouped_id))
            .map(|p| (p.chat_id, p.grouped_id))
        {
            if let Some(pending) = buf.remove(&oldest) {
                flushed.push(album_complete(account, &pending));
            }
        }
    }
    let entry = buf.entry(key).or_insert_with(|| PendingAlbum {
        chat_id,
        grouped_id,
        rows: Vec::new(),
        deadline: now,
    });
    entry.rows.push(row);
    entry.deadline = now + std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS);
    flushed
}

fn album_flush(account: &str, buf: &mut AlbumBuffer) -> Vec<serde_json::Value> {
    let mut keys: Vec<AlbumKey> = buf.keys().copied().collect();
    keys.sort_by_key(|k| (buf[k].deadline, *k));
    keys.into_iter()
        .filter_map(|k| buf.remove(&k).map(|p| album_complete(account, &p)))
        .collect()
}

fn album_flush_chat(
    account: &str,
    buf: &mut AlbumBuffer,
    chat_id: Option<i64>,
) -> Vec<serde_json::Value> {
    let mut keys: Vec<AlbumKey> = buf
        .keys()
        .copied()
        .filter(|k| Some(k.0) == chat_id)
        .collect();
    keys.sort_by_key(|k| (buf[k].deadline, *k));
    keys.into_iter()
        .filter_map(|k| buf.remove(&k).map(|p| album_complete(account, &p)))
        .collect()
}

fn album_sweep(
    account: &str,
    buf: &mut AlbumBuffer,
    now: tokio::time::Instant,
) -> Vec<serde_json::Value> {
    let mut due: Vec<AlbumKey> = buf
        .iter()
        .filter(|(_, p)| p.deadline <= now)
        .map(|(k, _)| *k)
        .collect();
    due.sort_by_key(|k| (buf[k].deadline, *k));
    due.into_iter()
        .filter_map(|k| buf.remove(&k).map(|p| album_complete(account, &p)))
        .collect()
}

fn min_album_deadline(buf: &AlbumBuffer) -> Option<tokio::time::Instant> {
    buf.values().map(|p| p.deadline).min()
}

fn album_complete(account: &str, pending: &PendingAlbum) -> serde_json::Value {
    let ids: Vec<i32> = pending
        .rows
        .iter()
        .filter_map(|r| {
            r.get("id")
                .and_then(|v| v.as_i64())
                .and_then(|v| i32::try_from(v).ok())
        })
        .collect();
    let mut row = event_row("Album", account, Some(pending.chat_id), None, None);
    if let serde_json::Value::Object(map) = &mut row {
        map.insert("grouped_id".into(), serde_json::json!(pending.grouped_id));
        map.insert("ids".into(), serde_json::json!(ids));
        if let Some(first) = pending.rows.first().and_then(|r| r.get("date").cloned()) {
            map.insert("date".into(), first);
        }
        map.insert(
            "messages".into(),
            serde_json::Value::Array(pending.rows.clone()),
        );
    }
    row
}

struct ObservedPeers {
    by_id: crate::capped_map::CappedMap<(PeerId, i32), PeerId>,
}

impl ObservedPeers {
    fn new() -> Self {
        ObservedPeers {
            by_id: crate::capped_map::CappedMap::new(OBSERVED_PEER_CAP),
        }
    }

    fn observe(&mut self, id: i32, peer: PeerId) {
        let key = (peer, id);
        if !self.by_id.contains(&key) {
            self.by_id.insert(key, peer);
        }
    }

    fn peer_of(&self, peer: PeerId, id: i32) -> Option<PeerId> {
        self.by_id.get(&(peer, id)).copied()
    }
}

fn observed_deletion_ids(ids: &[i32], observed: &ObservedPeers, target: PeerId) -> Vec<i32> {
    ids.iter()
        .copied()
        .filter(|id| observed.peer_of(target, *id) == Some(target))
        .collect()
}

const LISTEN_DEDUPE_CAP: usize = 10_000;

type ListenDedupe = super::serve::CappedDedupe<(i64, i32, i32)>;

fn dedupe_key(
    chat_id: Option<i64>,
    msg_id: i32,
    raw: &tl::enums::Update,
) -> Option<(i64, i32, i32)> {
    let point = pts_point(raw)?;
    Some((chat_id.unwrap_or(0), msg_id, point.pts))
}

#[cfg(test)]
use super::serve::pts_from_state;

pub async fn run(args: &ListenArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    use grammers_client::update::Update;
    let mut events: Vec<String> = args.events.clone();
    events.retain(|e| VALID_EVENTS.contains(&e.as_str()));
    if events.len() != args.events.len() {
        return Err(TeleError::Usage(format!(
            "unknown event name in --events (valid: {})",
            VALID_EVENTS.join(", ")
        )));
    }
    if args.raw && !events.iter().any(|e| e == "Raw") {
        events.push("Raw".to_string());
    }
    let filter_patterns = compile_pattern(&args.pattern)?;
    require_explicit_selection("listen", flags)?;
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
    let direction = if args.out {
        Some(Direction::Out)
    } else if args.r#in {
        Some(Direction::In)
    } else {
        None
    };
    let chat_targets = args.chat.clone();
    let from_targets = args.from.clone();
    validate_listen_inputs(&chat_targets, &from_targets)?;
    let cfg = crate::config::load_config(config_path.as_deref())?;
    let parallel = crate::executor::effective_parallel(flags.parallel, cfg.parallel_max)? as usize;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let config_path = config_path.clone();
        let chat_targets = chat_targets.clone();
        let from_targets = from_targets.clone();
        let events = events.clone();
        let filter_patterns = filter_patterns.clone();
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await;
            let result: TeleResult<()> = async {
                let creds =
                    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))?;
                let deadline = if timeout_secs > 0 {
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs))
                } else {
                    None
                };
                let mut filter = EventFilter::default();
                let mut targets_resolved = false;
                let mut failures: u32 = 0;
                let mut dedupe = ListenDedupe::new(LISTEN_DEDUPE_CAP);
                let rate_limiter = match ClientGuard::account_rate_limiter(
                    &name,
                    config_path.as_deref(),
                ) {
                    Ok(rl) => rl,
                    Err(e) => return Err(TeleError::from(e)),
                };
                loop {
                    if let Some(d) = deadline {
                        if std::time::Instant::now() >= d {
                            break;
                        }
                    }
                    let mut guard = match ClientGuard::connect_with_limiter(
                        &name,
                        creds.api_id,
                        config_path.as_deref(),
                        std::sync::Arc::clone(&rate_limiter),
                    )
                    .await
                    {
                            Ok(guard) => guard,
                            Err(e) => {
                                handle_stream_failure(
                                    &name,
                                    TeleError::from(e),
                                    &mut failures,
                                    deadline,
                                    MAX_RECONNECT_ATTEMPTS,
                                )
                                .await?;
                                continue;
                            }
                        };
                    if let Err(e) = client::authorize(&guard.client).await {
                        handle_stream_failure(&name, e, &mut failures, deadline, MAX_RECONNECT_ATTEMPTS).await?;
                        continue;
                    }
                    if !targets_resolved {
                        filter = resolve_filter(
                            &guard.client,
                            &guard.session,
                            &chat_targets,
                            &from_targets,
                            direction,
                        )
                        .await?;
                        filter.patterns = filter_patterns.clone();
                        targets_resolved = true;
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
                        handle_stream_failure(&name, err, &mut failures, deadline, MAX_RECONNECT_ATTEMPTS).await?;
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
                                MAX_RECONNECT_ATTEMPTS,
                            )
                            .await?;
                            continue;
                        }
                    };
                    failures = on_reconnect_success(failures);
                    let mut gaps = GapTracker::default();
                    let mut observed = ObservedPeers::new();
                    let mut album = AlbumBuffer::new();
                    let gap_on = events.iter().any(|e| e == "Gap");
                    let album_on = events.iter().any(|e| e == "Album");
                    let new_message_on = events.iter().any(|e| e == "NewMessage");
                    let service_on = events.iter().any(|e| e == "Service");
                    let chat_action_on = events.iter().any(|e| e == "ChatAction");
                    let user_update_on = events.iter().any(|e| e == "UserUpdate");
                    let callback_query_on = events.iter().any(|e| e == "CallbackQuery");
                    loop {
                        if let Some(d) = deadline {
                            if std::time::Instant::now() >= d {
                                for done in album_flush(&name, &mut album) {
                                    if emit_row_or_stop(&name, done).await? {
                                        return Ok(());
                                    }
                                }
                                break;
                            }
                        }
                        enum Tick {
                            Update(Box<grammers_client::update::Update>),
                            StreamError(grammers_client::InvocationError),
                            Idle,
                            AlbumDue,
                        }
                        let album_timer = async {
                            match min_album_deadline(&album) {
                                Some(due) => tokio::time::sleep_until(due).await,
                                None => std::future::pending::<()>().await,
                            }
                        };
                        let tick = tokio::select! {
                            _ = album_timer => Tick::AlbumDue,
                            res = tokio::time::timeout(
                                poll_timeout(deadline, std::time::Instant::now()),
                                stream.next(),
                            ) => match res {
                                Ok(Ok(u)) => Tick::Update(Box::new(u)),
                                Ok(Err(e)) => Tick::StreamError(e),
                                Err(_) => Tick::Idle,
                            },
                        };
                        let update = match tick {
                            Tick::AlbumDue => {
                                for done in
                                    album_sweep(&name, &mut album, tokio::time::Instant::now())
                                {
                                    if emit_row_or_stop(&name, done).await? {
                                        return Ok(());
                                    }
                                }
                                continue;
                            }
                            Tick::Idle => continue,
                            Tick::StreamError(e) => {
                                if crate::error::invocation_is_unauthorized(&e) {
                                    output::log_line(
                                        "error",
                                        &format!("{name}: not authorized, stopping stream"),
                                    );
                                    return Err(crate::error::invocation_error(e));
                                }
                                for done in album_flush(&name, &mut album) {
                                    if emit_row_or_stop(&name, done).await? {
                                        return Ok(());
                                    }
                                }
                                handle_stream_failure(
                                    &name,
                                    crate::error::invocation_error(e),
                                    &mut failures,
                                    deadline,
                                    MAX_RECONNECT_ATTEMPTS,
                                )
                                .await?;
                                break;
                            }
                            Tick::Update(u) => *u,
                        };
                        failures = on_reconnect_success(failures);
                        if let Err(e) = stream.sync_update_state().await {
                            output::log_line(
                                "warn",
                                &format!("{name}: sync_update_state failed: {e}"),
                            );
                        }
                        if gap_on {
                            if let Some(signal) = gaps.observe(update.raw()) {
                                                                if emit_row_or_stop(&name, gap_row(&name, &signal, update.state())).await? { return Ok(()); }

                            }
                        }
                        match &update {
                            Update::NewMessage(m) => {
                                let service_flavored =
                                    matches!(&(**m).raw, tl::enums::Message::Service(_));
                                if !message_event_applies(
                                    new_message_on,
                                    album_on,
                                    service_on,
                                    service_flavored,
                                ) {
                                    continue;
                                }
                                let peer = update_peer(&m.raw);
                                if !filter.chat_allows(peer) {
                                    continue;
                                }
                                if is_empty_update(&m.raw) {
                                    continue;
                                }
                                if !filter
                                    .message_allows(update_sender(&m.raw), update_outgoing(&m.raw))
                                {
                                    continue;
                                }
                                if !filter.text_allows(update_text(&m.raw)) {
                                    continue;
                                }
                                if !filter.chats.is_empty() {
                                    if let Some(peer) = m.peer() {
                                        observed.observe(m.id(), peer.id());
                                    }
                                }
                                let chat_id = peer.and_then(|p| p.bare_id());
                                if let Some(key) = dedupe_key(chat_id, m.id(), update.raw()) {
                                    if dedupe.check(key) {
                                        continue;
                                    }
                                }
                                if routes_to_service(service_on, service_flavored) {
                                    if let tl::enums::Message::Service(svc) = &(**m).raw {
                                        let mut svc_row = match streamed_message_row(m) {
                                            Ok(r) => r,
                                            Err(e) => {
                                                output::log_line(
                                                    "error",
                                                    &format!(
                                                        "{name}: failed to serialize message {}: {}",
                                                        m.id(),
                                                        e.message()
                                                    ),
                                                );
                                                continue;
                                            }
                                        };
                                        crate::serialize::ensure_outer_peer_sender(
                                            &mut svc_row,
                                            peer,
                                            None,
                                        );
                                                                                if emit_row_or_stop(&name, service_row(
                                            &name, chat_id, svc_row, &svc.action,
                                        )).await? { return Ok(()); }

                                        continue;
                                    }
                                }
                                let mut row = match streamed_message_row(m) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        output::log_line(
                                            "error",
                                            &format!(
                                                "{name}: failed to serialize message {}: {}",
                                                m.id(),
                                                e.message()
                                            ),
                                        );
                                        continue;
                                    }
                                };
                                crate::serialize::ensure_outer_peer_sender(&mut row, peer, None);
                                let grouped = if album_on { album_member(&row) } else { None };
                                match (grouped, chat_id) {
                                    (Some((member_chat, gid)), _) => {
                                        for done in album_ingest(
                                            &name,
                                            &mut album,
                                            row,
                                            member_chat,
                                            gid,
                                            tokio::time::Instant::now(),
                                        ) {
                                            if emit_row_or_stop(&name, done).await? {
                                                return Ok(());
                                            }
                                        }
                                    }
                                    (None, _) => {
                                        if !new_message_on {
                                            continue;
                                        }
                                for done in
                                    album_flush_chat(&name, &mut album, chat_id)
                                {
                                    if emit_row_or_stop(&name, done).await? {
                                        return Ok(());
                                    }
                                }
                                if emit_row_or_stop(&name, event_row(
                                    "NewMessage",
                                    &name,
                                    chat_id,
                                    None,
                                    Some(row),
                                )).await? { return Ok(()); }

                                    }
                                }
                            }
                            Update::MessageEdited(m) => {
                                let service_flavored =
                                    matches!(&(**m).raw, tl::enums::Message::Service(_));
                                if !routes_to_service(service_on, service_flavored)
                                    && !events.iter().any(|e| e == "MessageEdited")
                                {
                                    continue;
                                }
                                let peer = update_peer(&m.raw);
                                if !filter.chat_allows(peer) {
                                    continue;
                                }
                                if is_empty_update(&m.raw) {
                                    continue;
                                }
                                if !filter
                                    .message_allows(update_sender(&m.raw), update_outgoing(&m.raw))
                                {
                                    continue;
                                }
                                if !filter.text_allows(update_text(&m.raw)) {
                                    continue;
                                }
                                if !filter.chats.is_empty() {
                                    if let Some(peer) = m.peer() {
                                        observed.observe(m.id(), peer.id());
                                    }
                                }
                                let chat_id = peer.and_then(|p| p.bare_id());
                                if let Some(key) = dedupe_key(chat_id, m.id(), update.raw()) {
                                    if dedupe.check(key) {
                                        continue;
                                    }
                                }
                                if routes_to_service(service_on, service_flavored) {
                                    if let tl::enums::Message::Service(svc) = &(**m).raw {
                                        let mut svc_row = match streamed_message_row(m) {
                                            Ok(r) => r,
                                            Err(e) => {
                                                output::log_line(
                                                    "error",
                                                    &format!(
                                                        "{name}: failed to serialize message {}: {}",
                                                        m.id(),
                                                        e.message()
                                                    ),
                                                );
                                                continue;
                                            }
                                        };
                                        crate::serialize::ensure_outer_peer_sender(
                                            &mut svc_row,
                                            peer,
                                            None,
                                        );
                                                                                if emit_row_or_stop(&name, service_row(
                                            &name, chat_id, svc_row, &svc.action,
                                        )).await? { return Ok(()); }

                                        continue;
                                    }
                                }
                                let mut row = match streamed_message_row(m) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        output::log_line(
                                            "error",
                                            &format!(
                                                "{name}: failed to serialize message {}: {}",
                                                m.id(),
                                                e.message()
                                            ),
                                        );
                                        continue;
                                    }
                                };
                                crate::serialize::ensure_outer_peer_sender(&mut row, peer, None);
                                                                if emit_row_or_stop(&name, event_row(
                                    "MessageEdited",
                                    &name,
                                    chat_id,
                                    None,
                                    Some(row),
                                )).await? { return Ok(()); }

                            }
                            Update::MessageDeleted(d) => {
                                if !events.iter().any(|e| e == "MessageDeleted") {
                                    continue;
                                }
                                if !filter.deletions_pass() {
                                    continue;
                                }
                                let matched = match filter.chats.as_slice() {
                                    [] => Some(d.messages().to_vec()),
                                    targets => deletion_match_set(
                                        d.messages(),
                                        d.channel_id(),
                                        &observed,
                                        targets,
                                    ),
                                };
                                let Some(matched) = matched else {
                                    continue;
                                };
                                                                if emit_row_or_stop(&name, event_row(
                                    "MessageDeleted",
                                    &name,
                                    sole_chat_label(&filter.chats),
                                    Some(&matched),
                                    None,
                                )).await? { return Ok(()); }

                            }
                            _ => {
                                if chat_action_on {
                                    if let Some((peer, sender, row)) =
                                        chat_action_row(&name, update.raw())
                                    {
                                        if !filter.action_allows(peer, sender) {
                                            continue;
                                        }
                                                                                if emit_row_or_stop(&name, row).await? { return Ok(()); }

                                        continue;
                                    }
                                }
                                if user_update_on {
                                    if let Some((peer, sender, row)) =
                                        user_update_row(&name, update.raw())
                                    {
                                        if !filter.action_allows(peer, sender) {
                                            continue;
                                        }
                                                                                if emit_row_or_stop(&name, row).await? { return Ok(()); }

                                        continue;
                                    }
                                }
                                if callback_query_on {
                                    if let Some((peer, sender, row)) =
                                        callback_query_row(&name, update.raw())
                                    {
                                        if !filter.action_allows(peer, sender) {
                                            continue;
                                        }
                                                                                if emit_row_or_stop(&name, row).await? { return Ok(()); }

                                        continue;
                                    }
                                }
                                if !events.iter().any(|e| e == "Raw") {
                                    continue;
                                }
                                if !filter.raw_allows(update_peer(update.raw())) {
                                    continue;
                                }
                                                                if emit_row_or_stop(&name, raw_row(&name, update.raw(), update.state())).await? { return Ok(()); }

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
    Ok(crate::error::aggregate_exit_code(ok_count, &failed))
}

async fn emit_row(value: serde_json::Value) -> TeleResult<()> {
    let line = serde_json::to_string(&value)?;
    tokio::task::spawn_blocking(move || {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")?;
        out.flush()
    })
    .await
    .map_err(|e| TeleError::TaskPanic(e.to_string()))??;
    Ok(())
}

async fn emit_row_or_stop(account: &str, value: serde_json::Value) -> TeleResult<bool> {
    match emit_row(value).await {
        Ok(()) => Ok(false),
        Err(e) if emit_stops_stream(&e) => Ok(true),
        Err(e) => {
            output::log_line("error", &format!("{account}: emit failed: {}", e.message()));
            Ok(false)
        }
    }
}

fn emit_stops_stream(err: &TeleError) -> bool {
    err.is_broken_pipe()
}

fn flood_wait_secs(err: &TeleError) -> Option<std::time::Duration> {
    match err {
        TeleError::Rpc(_, 420, _, Some(s)) => Some(std::time::Duration::from_secs(u64::from(*s))),
        TeleError::Invocation(_, Some(s)) => Some(std::time::Duration::from_secs(u64::from(*s))),
        _ => None,
    }
}

pub(crate) async fn handle_stream_failure(
    account: &str,
    err: TeleError,
    failures: &mut u32,
    deadline: Option<std::time::Instant>,
    max_attempts: u32,
) -> TeleResult<()> {
    if is_auth_error(&err) {
        return Err(err);
    }
    *failures = on_failure(*failures);
    if *failures > max_attempts {
        return Err(match err {
            TeleError::Rpc(msg, code, name, seconds) => TeleError::Rpc(
                format!("{account}: updates stream failed {failures} consecutive times, giving up: {msg}"),
                code,
                name,
                seconds,
            ),
            other => TeleError::Other(format!(
                "{account}: updates stream failed {failures} consecutive times, giving up: {}",
                other.message()
            )),
        });
    }
    let base_delay = next_delay(*failures);
    let delay = match flood_wait_secs(&err) {
        Some(wait) => base_delay.max(wait),
        None => base_delay,
    };
    let sleep_for = match deadline {
        Some(d) => delay.min(d.saturating_duration_since(std::time::Instant::now())),
        None => delay,
    };
    output::log_line(
        "error",
        &reconnect_message(account, *failures, delay.as_secs() as u32, &err.message()),
    );
    tokio::time::sleep(sleep_for).await;
    Ok(())
}

fn is_auth_error(e: &TeleError) -> bool {
    matches!(e, TeleError::Auth(_))
}

pub(crate) fn getstate_probe_error(e: grammers_client::InvocationError) -> TeleError {
    if crate::error::invocation_is_unauthorized(&e) {
        crate::error::invocation_error(e)
    } else {
        match &e {
            grammers_client::InvocationError::Rpc(_) => {
                let mut err = crate::error::invocation_error(e);
                if let TeleError::Rpc(msg, _, _, _) = &mut err {
                    *msg = format!("initial GetState failed: {msg}");
                }
                err
            }
            other => TeleError::Other(format!("initial GetState failed: {other}")),
        }
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

pub(crate) fn update_peer(u: &tl::enums::Update) -> Option<PeerId> {
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

pub(crate) fn is_empty_update(u: &tl::enums::Update) -> bool {
    match u {
        tl::enums::Update::NewMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::NewChannelMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::EditMessage(x) => is_empty_message(&x.message),
        tl::enums::Update::EditChannelMessage(x) => is_empty_message(&x.message),
        _ => false,
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

#[cfg(test)]
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

#[cfg(test)]
fn give_up(failures: u32) -> bool {
    !reconnect_allowed(failures)
}

fn reconnect_message(account: &str, failures: u32, backoff: u32, cause: &str) -> String {
    format!(
        "{account}: updates stream error ({cause}), reconnecting (attempt {failures}/{MAX_RECONNECT_ATTEMPTS}) in {backoff}s"
    )
}

pub(crate) fn poll_timeout(
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

pub(crate) fn event_row(
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
            serde_json::Value::from({
                use base64::engine::general_purpose::STANDARD;
                use base64::Engine;
                STANDARD.encode(raw.to_bytes())
            }),
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

fn streamed_message_row(m: &grammers_client::message::Message) -> TeleResult<serde_json::Value> {
    let mut row = crate::serialize::message_to_json(m)?;
    crate::serialize::enrich_message_row(&mut row, m);
    Ok(row)
}

fn message_event_applies(
    new_message_on: bool,
    album_on: bool,
    service_on: bool,
    service_flavored: bool,
) -> bool {
    new_message_on || album_on || (service_on && service_flavored)
}

fn routes_to_service(service_on: bool, service_flavored: bool) -> bool {
    service_on && service_flavored
}

fn service_row(
    account: &str,
    chat_id: Option<i64>,
    base: serde_json::Value,
    action: &tl::enums::MessageAction,
) -> serde_json::Value {
    let mut row = event_row("Service", account, chat_id, None, Some(base));
    let (kind, label) = message_action_kind_label(action);
    if let serde_json::Value::Object(map) = &mut row {
        map.insert(
            "service_action".into(),
            serde_json::json!({ "kind": kind, "label": label }),
        );
    }
    row
}

fn chat_action_row(
    account: &str,
    raw: &tl::enums::Update,
) -> Option<(Option<PeerId>, Option<PeerId>, serde_json::Value)> {
    let build = |peer: Option<PeerId>,
                 emit_chat_id: Option<i64>,
                 sender: Option<PeerId>,
                 user_id: Option<i64>,
                 action: &tl::enums::SendMessageAction| {
        let (kind, label) = typing_action_kind_label(action);
        let mut row = event_row("ChatAction", account, None, None, None);
        if let serde_json::Value::Object(map) = &mut row {
            if let Some(chat_id) = emit_chat_id {
                map.insert("chat_id".into(), serde_json::json!(chat_id));
            }
            if let Some(user_id) = user_id {
                map.insert("user_id".into(), serde_json::json!(user_id));
            }
            map.insert(
                "action".into(),
                serde_json::json!({ "kind": kind, "label": label }),
            );
        }
        (peer, sender, row)
    };
    let sender_user_id = |from_id: &tl::enums::Peer| match from_id {
        tl::enums::Peer::User(user) => Some(user.user_id),
        _ => None,
    };
    match raw {
        tl::enums::Update::UserTyping(t) => {
            let peer = PeerId::user(t.user_id);
            Some(build(peer, None, peer, Some(t.user_id), &t.action))
        }
        tl::enums::Update::ChatUserTyping(t) => {
            let chat = PeerId::chat(t.chat_id);
            let sender = PeerId::from(&t.from_id);
            let emit = chat.and_then(|p| p.bare_id());
            Some(build(
                chat,
                emit,
                Some(sender),
                sender_user_id(&t.from_id),
                &t.action,
            ))
        }
        tl::enums::Update::ChannelUserTyping(t) => {
            let chat = PeerId::channel(t.channel_id);
            let sender = PeerId::from(&t.from_id);
            let emit = chat.and_then(|p| p.bare_id());
            Some(build(
                chat,
                emit,
                Some(sender),
                sender_user_id(&t.from_id),
                &t.action,
            ))
        }
        _ => None,
    }
}

fn user_update_row(
    account: &str,
    raw: &tl::enums::Update,
) -> Option<(Option<PeerId>, Option<PeerId>, serde_json::Value)> {
    match raw {
        tl::enums::Update::UserStatus(u) => {
            let peer = PeerId::user(u.user_id);
            let mut row = event_row("UserUpdate", account, None, None, None);
            if let serde_json::Value::Object(map) = &mut row {
                map.insert("user_id".into(), serde_json::json!(u.user_id));
                map.insert("status".into(), user_status_json(&u.status));
            }
            Some((peer, peer, row))
        }
        _ => None,
    }
}

fn callback_query_row(
    account: &str,
    raw: &tl::enums::Update,
) -> Option<(Option<PeerId>, Option<PeerId>, serde_json::Value)> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let build = |peer: Option<PeerId>,
                 sender: Option<PeerId>,
                 user_id: Option<i64>,
                 chat_id: Option<i64>,
                 msg_id: Option<i32>,
                 data: &[u8]| {
        let mut row = event_row("CallbackQuery", account, None, None, None);
        if let serde_json::Value::Object(map) = &mut row {
            if let Some(user_id) = user_id {
                map.insert("user_id".into(), serde_json::json!(user_id));
            }
            if let Some(chat_id) = chat_id {
                map.insert("chat_id".into(), serde_json::json!(chat_id));
            }
            if let Some(msg_id) = msg_id {
                map.insert("message_id".into(), serde_json::json!(msg_id));
            }
            map.insert(
                "data".into(),
                serde_json::json!(String::from_utf8_lossy(data)),
            );
            map.insert("data_b64".into(), serde_json::json!(STANDARD.encode(data)));
        }
        (peer, sender, row)
    };
    match raw {
        tl::enums::Update::BotCallbackQuery(q) => {
            let peer = PeerId::from(&q.peer);
            let chat_id = peer.bare_id();
            let sender = PeerId::user(q.user_id);
            Some(build(
                Some(peer),
                sender,
                Some(q.user_id),
                chat_id,
                Some(q.msg_id),
                q.data.as_deref().unwrap_or_default(),
            ))
        }
        tl::enums::Update::InlineBotCallbackQuery(q) => {
            let sender = PeerId::user(q.user_id);
            Some(build(
                sender,
                sender,
                Some(q.user_id),
                None,
                None,
                q.data.as_deref().unwrap_or_default(),
            ))
        }
        _ => None,
    }
}

fn user_status_json(status: &tl::enums::UserStatus) -> serde_json::Value {
    let (kind, label) = user_status_kind_label(status);
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), serde_json::json!(kind));
    out.insert("label".into(), serde_json::json!(label));
    match status {
        tl::enums::UserStatus::Online(t) => {
            out.insert("expires".into(), serde_json::json!(t.expires));
        }
        tl::enums::UserStatus::Offline(t) => {
            out.insert("was_online".into(), serde_json::json!(t.was_online));
        }
        _ => {}
    }
    serde_json::Value::Object(out)
}

fn user_status_kind_label(status: &tl::enums::UserStatus) -> (&'static str, &'static str) {
    match status {
        tl::enums::UserStatus::Empty => ("userStatusEmpty", "empty"),
        tl::enums::UserStatus::Online(_) => ("userStatusOnline", "online"),
        tl::enums::UserStatus::Offline(_) => ("userStatusOffline", "offline"),
        tl::enums::UserStatus::Recently(_) => ("userStatusRecently", "recently"),
        tl::enums::UserStatus::LastWeek(_) => ("userStatusLastWeek", "last-week"),
        tl::enums::UserStatus::LastMonth(_) => ("userStatusLastMonth", "last-month"),
    }
}

fn typing_action_kind_label(action: &tl::enums::SendMessageAction) -> (&'static str, &'static str) {
    let kind = match action {
        tl::enums::SendMessageAction::SendMessageTypingAction => "sendMessageTypingAction",
        tl::enums::SendMessageAction::SendMessageCancelAction => "sendMessageCancelAction",
        tl::enums::SendMessageAction::SendMessageRecordVideoAction => {
            "sendMessageRecordVideoAction"
        }
        tl::enums::SendMessageAction::SendMessageUploadVideoAction(_) => {
            "sendMessageUploadVideoAction"
        }
        tl::enums::SendMessageAction::SendMessageRecordAudioAction => {
            "sendMessageRecordAudioAction"
        }
        tl::enums::SendMessageAction::SendMessageUploadAudioAction(_) => {
            "sendMessageUploadAudioAction"
        }
        tl::enums::SendMessageAction::SendMessageUploadPhotoAction(_) => {
            "sendMessageUploadPhotoAction"
        }
        tl::enums::SendMessageAction::SendMessageUploadDocumentAction(_) => {
            "sendMessageUploadDocumentAction"
        }
        tl::enums::SendMessageAction::SendMessageGeoLocationAction => {
            "sendMessageGeoLocationAction"
        }
        tl::enums::SendMessageAction::SendMessageChooseContactAction => {
            "sendMessageChooseContactAction"
        }
        tl::enums::SendMessageAction::SendMessageGamePlayAction => "sendMessageGamePlayAction",
        tl::enums::SendMessageAction::SendMessageRecordRoundAction => {
            "sendMessageRecordRoundAction"
        }
        tl::enums::SendMessageAction::SendMessageUploadRoundAction(_) => {
            "sendMessageUploadRoundAction"
        }
        tl::enums::SendMessageAction::SpeakingInGroupCallAction => "speakingInGroupCallAction",
        tl::enums::SendMessageAction::SendMessageHistoryImportAction(_) => {
            "sendMessageHistoryImportAction"
        }
        tl::enums::SendMessageAction::SendMessageChooseStickerAction => {
            "sendMessageChooseStickerAction"
        }
        tl::enums::SendMessageAction::SendMessageEmojiInteraction(_) => {
            "sendMessageEmojiInteraction"
        }
        tl::enums::SendMessageAction::SendMessageEmojiInteractionSeen(_) => {
            "sendMessageEmojiInteractionSeen"
        }
        tl::enums::SendMessageAction::SendMessageTextDraftAction(_) => "sendMessageTextDraftAction",
        tl::enums::SendMessageAction::InputSendMessageRichMessageDraftAction(_) => {
            "inputSendMessageRichMessageDraftAction"
        }
        tl::enums::SendMessageAction::SendMessageRichMessageDraftAction(_) => {
            "sendMessageRichMessageDraftAction"
        }
    };
    let label = match kind {
        "sendMessageTypingAction" => "typing",
        _ => kind,
    };
    (kind, label)
}

fn message_action_kind_label(action: &tl::enums::MessageAction) -> (&'static str, &'static str) {
    let kind = message_action_kind(action);
    let label = match kind {
        "messageActionChatAddUser" | "messageActionInviteToGroupCall" => "join-invite",
        "messageActionChatJoinedByLink" | "messageActionChatJoinedByRequest" => "join",
        "messageActionChatDeleteUser" => "leave",
        "messageActionPinMessage" => "pin",
        _ => kind,
    };
    (kind, label)
}

fn message_action_kind(action: &tl::enums::MessageAction) -> &'static str {
    match action {
        tl::enums::MessageAction::Empty => "messageActionEmpty",
        tl::enums::MessageAction::ChatCreate(_) => "messageActionChatCreate",
        tl::enums::MessageAction::ChatEditTitle(_) => "messageActionChatEditTitle",
        tl::enums::MessageAction::ChatEditPhoto(_) => "messageActionChatEditPhoto",
        tl::enums::MessageAction::ChatDeletePhoto => "messageActionChatDeletePhoto",
        tl::enums::MessageAction::ChatAddUser(_) => "messageActionChatAddUser",
        tl::enums::MessageAction::ChatDeleteUser(_) => "messageActionChatDeleteUser",
        tl::enums::MessageAction::ChatJoinedByLink(_) => "messageActionChatJoinedByLink",
        tl::enums::MessageAction::ChannelCreate(_) => "messageActionChannelCreate",
        tl::enums::MessageAction::ChatMigrateTo(_) => "messageActionChatMigrateTo",
        tl::enums::MessageAction::ChannelMigrateFrom(_) => "messageActionChannelMigrateFrom",
        tl::enums::MessageAction::PinMessage => "messageActionPinMessage",
        tl::enums::MessageAction::HistoryClear => "messageActionHistoryClear",
        tl::enums::MessageAction::GameScore(_) => "messageActionGameScore",
        tl::enums::MessageAction::PaymentSentMe(_) => "messageActionPaymentSentMe",
        tl::enums::MessageAction::PaymentSent(_) => "messageActionPaymentSent",
        tl::enums::MessageAction::PhoneCall(_) => "messageActionPhoneCall",
        tl::enums::MessageAction::ScreenshotTaken => "messageActionScreenshotTaken",
        tl::enums::MessageAction::CustomAction(_) => "messageActionCustomAction",
        tl::enums::MessageAction::BotAllowed(_) => "messageActionBotAllowed",
        tl::enums::MessageAction::SecureValuesSentMe(_) => "messageActionSecureValuesSentMe",
        tl::enums::MessageAction::SecureValuesSent(_) => "messageActionSecureValuesSent",
        tl::enums::MessageAction::ContactSignUp => "messageActionContactSignUp",
        tl::enums::MessageAction::GeoProximityReached(_) => "messageActionGeoProximityReached",
        tl::enums::MessageAction::GroupCall(_) => "messageActionGroupCall",
        tl::enums::MessageAction::InviteToGroupCall(_) => "messageActionInviteToGroupCall",
        tl::enums::MessageAction::SetMessagesTtl(_) => "messageActionSetMessagesTTL",
        tl::enums::MessageAction::GroupCallScheduled(_) => "messageActionGroupCallScheduled",
        tl::enums::MessageAction::SetChatTheme(_) => "messageActionSetChatTheme",
        tl::enums::MessageAction::ChatJoinedByRequest => "messageActionChatJoinedByRequest",
        tl::enums::MessageAction::WebViewDataSentMe(_) => "messageActionWebViewDataSentMe",
        tl::enums::MessageAction::WebViewDataSent(_) => "messageActionWebViewDataSent",
        tl::enums::MessageAction::GiftPremium(_) => "messageActionGiftPremium",
        tl::enums::MessageAction::TopicCreate(_) => "messageActionTopicCreate",
        tl::enums::MessageAction::TopicEdit(_) => "messageActionTopicEdit",
        tl::enums::MessageAction::SuggestProfilePhoto(_) => "messageActionSuggestProfilePhoto",
        tl::enums::MessageAction::RequestedPeer(_) => "messageActionRequestedPeer",
        tl::enums::MessageAction::SetChatWallPaper(_) => "messageActionSetChatWallPaper",
        tl::enums::MessageAction::GiftCode(_) => "messageActionGiftCode",
        tl::enums::MessageAction::GiveawayLaunch(_) => "messageActionGiveawayLaunch",
        tl::enums::MessageAction::GiveawayResults(_) => "messageActionGiveawayResults",
        tl::enums::MessageAction::BoostApply(_) => "messageActionBoostApply",
        tl::enums::MessageAction::RequestedPeerSentMe(_) => "messageActionRequestedPeerSentMe",
        tl::enums::MessageAction::PaymentRefunded(_) => "messageActionPaymentRefunded",
        tl::enums::MessageAction::GiftStars(_) => "messageActionGiftStars",
        tl::enums::MessageAction::PrizeStars(_) => "messageActionPrizeStars",
        tl::enums::MessageAction::StarGift(_) => "messageActionStarGift",
        tl::enums::MessageAction::StarGiftUnique(_) => "messageActionStarGiftUnique",
        tl::enums::MessageAction::PaidMessagesRefunded(_) => "messageActionPaidMessagesRefunded",
        tl::enums::MessageAction::PaidMessagesPrice(_) => "messageActionPaidMessagesPrice",
        tl::enums::MessageAction::ConferenceCall(_) => "messageActionConferenceCall",
        tl::enums::MessageAction::TodoCompletions(_) => "messageActionTodoCompletions",
        tl::enums::MessageAction::TodoAppendTasks(_) => "messageActionTodoAppendTasks",
        tl::enums::MessageAction::SuggestedPostApproval(_) => "messageActionSuggestedPostApproval",
        tl::enums::MessageAction::SuggestedPostSuccess(_) => "messageActionSuggestedPostSuccess",
        tl::enums::MessageAction::SuggestedPostRefund(_) => "messageActionSuggestedPostRefund",
        tl::enums::MessageAction::GiftTon(_) => "messageActionGiftTon",
        tl::enums::MessageAction::SuggestBirthday(_) => "messageActionSuggestBirthday",
        tl::enums::MessageAction::StarGiftPurchaseOffer(_) => "messageActionStarGiftPurchaseOffer",
        tl::enums::MessageAction::StarGiftPurchaseOfferDeclined(_) => {
            "messageActionStarGiftPurchaseOfferDeclined"
        }
        tl::enums::MessageAction::NewCreatorPending(_) => "messageActionNewCreatorPending",
        tl::enums::MessageAction::ChangeCreator(_) => "messageActionChangeCreator",
        tl::enums::MessageAction::NoForwardsToggle(_) => "messageActionNoForwardsToggle",
        tl::enums::MessageAction::NoForwardsRequest(_) => "messageActionNoForwardsRequest",
        tl::enums::MessageAction::PollAppendAnswer(_) => "messageActionPollAppendAnswer",
        tl::enums::MessageAction::PollDeleteAnswer(_) => "messageActionPollDeleteAnswer",
        tl::enums::MessageAction::ManagedBotCreated(_) => "messageActionManagedBotCreated",
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "tests.rs"]
mod tests;
