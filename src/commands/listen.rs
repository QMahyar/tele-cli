use std::collections::{HashMap, VecDeque};
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

#[derive(Default)]
struct GapTracker {
    last: HashMap<StateBoxKey, (i32, i32)>,
    order: VecDeque<StateBoxKey>,
}

impl GapTracker {
    fn observe(&mut self, raw: &tl::enums::Update) -> Option<GapSignal> {
        let point = pts_point(raw)?;
        if !self.last.contains_key(&point.box_key) && self.last.len() >= GAP_TRACKER_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.last.remove(&oldest);
            }
        }
        match self.last.get_mut(&point.box_key) {
            None => {
                self.last
                    .insert(point.box_key, (point.pts, point.pts_count.max(0)));
                self.order.push_back(point.box_key);
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

fn album_ingest(
    account: &str,
    buf: &mut Option<PendingAlbum>,
    row: serde_json::Value,
    chat_id: i64,
    grouped_id: i64,
    now: tokio::time::Instant,
) -> Option<serde_json::Value> {
    let flush = match buf {
        None => None,
        Some(pending) if pending.chat_id == chat_id && pending.grouped_id == grouped_id => {
            if pending.rows.len() >= MAX_ALBUM_LEN {
                Some(album_complete(account, pending))
            } else {
                None
            }
        }
        Some(pending) => Some(album_complete(account, pending)),
    };
    let deadline = now + std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS);
    match buf {
        Some(pending) if pending.chat_id == chat_id && pending.grouped_id == grouped_id => {
            if flush.is_some() {
                *buf = Some(PendingAlbum {
                    chat_id,
                    grouped_id,
                    rows: vec![row],
                    deadline,
                });
            } else {
                pending.rows.push(row);
                pending.deadline = deadline;
            }
        }
        _ => {
            *buf = Some(PendingAlbum {
                chat_id,
                grouped_id,
                rows: vec![row],
                deadline,
            });
        }
    }
    flush
}

fn album_flush(account: &str, buf: &mut Option<PendingAlbum>) -> Option<serde_json::Value> {
    buf.take().map(|pending| album_complete(account, &pending))
}

fn album_complete(account: &str, pending: &PendingAlbum) -> serde_json::Value {
    let ids: Vec<i32> = pending
        .rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_i64()).and_then(|v| i32::try_from(v).ok()))
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
    by_id: HashMap<(PeerId, i32), PeerId>,
    order: VecDeque<(PeerId, i32)>,
}

impl ObservedPeers {
    fn new() -> Self {
        ObservedPeers {
            by_id: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn observe(&mut self, id: i32, peer: PeerId) {
        let key = (peer, id);
        if self.by_id.contains_key(&key) {
            return;
        }
        if self.by_id.len() >= OBSERVED_PEER_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
        self.by_id.insert(key, peer);
        self.order.push_back(key);
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
                    let mut gaps = GapTracker::default();
                    let mut observed = ObservedPeers::new();
                    let mut album: Option<PendingAlbum> = None;
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
                                if let Some(done) = album_flush(&name, &mut album) {
                                                                        if emit_row_or_stop(&name, done).await? { return Ok(()); }

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
                            match &album {
                                Some(pending) => tokio::time::sleep_until(pending.deadline).await,
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
                                if let Some(done) = album_flush(&name, &mut album) {
                                                                        if emit_row_or_stop(&name, done).await? { return Ok(()); }

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
                                if let Some(done) = album_flush(&name, &mut album) {
                                                                        if emit_row_or_stop(&name, done).await? { return Ok(()); }

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
                                        if let Some(done) = album_ingest(
                                            &name,
                                            &mut album,
                                            row,
                                            member_chat,
                                            gid,
                                            tokio::time::Instant::now(),
                                        ) {
                                                                                        if emit_row_or_stop(&name, done).await? { return Ok(()); }

                                        }
                                    }
                                    (None, _) => {
                                        if !new_message_on {
                                            continue;
                                        }
                                        if let Some(done) = album_flush(&name, &mut album) {
                                                                                        if emit_row_or_stop(&name, done).await? { return Ok(()); }

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
) -> TeleResult<()> {
    if is_auth_error(&err) {
        return Err(err);
    }
    *failures = on_failure(*failures);
    if give_up(*failures) {
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
mod tests {
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
        let matched =
            deletion_match_set(&[4, 5], Some(1234567890), &ObservedPeers::new(), &targets)
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
        let matched =
            deletion_match_set(&[11, 10, 11], None, &observed, &targets).expect("union hit");
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
            crate::error::aggregate_exit_code(
                0,
                &[crate::error::EXIT_AUTH, crate::error::EXIT_AUTH]
            ),
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
            crate::error::aggregate_exit_code(
                0,
                &[crate::error::EXIT_USAGE, crate::error::EXIT_AUTH]
            ),
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
            crate::error::aggregate_exit_code(
                0,
                &[crate::error::EXIT_USAGE, crate::error::EXIT_USAGE]
            ),
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
        let empty_channel =
            tl::enums::Update::NewChannelMessage(tl::types::UpdateNewChannelMessage {
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
        let null_peer =
            serde_json::json!({"id": 1, "grouped_id": 3, "peer": serde_json::Value::Null});
        assert!(album_member(&null_peer).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn album_ingest_buffers_members_and_extends_deadline_to_quiescence() {
        let mut buf = None;
        let now = tokio::time::Instant::now();
        assert!(album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now).is_none());
        assert!(album_ingest("work", &mut buf, member_row(2, 456, 9001), 456, 9001, now).is_none());
        tokio::time::advance(std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS - 1)).await;
        let pending = buf.as_ref().expect("album still pending before deadline");
        assert_eq!(pending.rows.len(), 2);
        let deadline = pending.deadline;
        tokio::time::sleep_until(deadline).await;
        let done = album_flush("work", &mut buf).expect("flush after quiescence");
        assert_eq!(done["event"], "Album");
        assert_eq!(done["messages"].as_array().unwrap().len(), 2);
        assert!(buf.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn album_ingest_group_switch_flushes_previous_album() {
        let mut buf = None;
        let now = tokio::time::Instant::now();
        album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
        album_ingest("work", &mut buf, member_row(2, 456, 9001), 456, 9001, now);
        let done = album_ingest("home", &mut buf, member_row(9, 789, 42), 789, 42, now)
            .expect("previous group flushed on key switch");
        assert_eq!(done["chat_id"], 456);
        assert_eq!(done["grouped_id"], serde_json::json!(9001));
        assert_eq!(done["ids"], serde_json::json!([1, 2]));
        let pending = buf.as_ref().unwrap();
        assert_eq!((pending.chat_id, pending.grouped_id), (789, 42));
        assert_eq!(pending.rows.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn album_flush_timer_fires_after_quiescence_window() {
        let mut buf = None;
        let now = tokio::time::Instant::now();
        album_ingest("work", &mut buf, member_row(1, 456, 9001), 456, 9001, now);
        let deadline = buf.as_ref().unwrap().deadline;
        assert_eq!(
            deadline - now,
            std::time::Duration::from_millis(ALBUM_FLUSH_MILLIS)
        );
        tokio::time::sleep_until(deadline).await;
        assert!(tokio::time::Instant::now() >= deadline);
    }

    #[test]
    fn album_flush_empty_buffer_is_none() {
        let mut buf = None;
        assert!(album_flush("work", &mut buf).is_none());
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
        assert_eq!(o.order.len(), 1);
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
        let err = validate_listen_inputs(&[], &["".to_string()])
            .expect_err("empty --from must be rejected");
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
        tl::enums::MessageAction::ChatDeleteUser(tl::types::MessageActionChatDeleteUser {
            user_id: 13,
        })
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
        let empty_channel =
            tl::enums::Update::NewChannelMessage(tl::types::UpdateNewChannelMessage {
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
        let err: TeleError =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied").into();
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
}
