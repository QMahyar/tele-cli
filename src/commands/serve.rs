use clap::Parser;
use grammers_client::tl;
use grammers_client::update::Update;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;

use crate::client::ServeShares;
use crate::commands::listen::{
    event_row, getstate_probe_error, handle_stream_failure, is_empty_update, poll_timeout,
    update_peer,
};
use crate::error::{TeleError, TeleResult, EXIT_OK};
use crate::executor::GlobalFlags;
use crate::output;
use std::time::Duration;

pub const SERVE_PROTOCOL: u32 = 1;
pub const SERVE_PROTOCOL_MIN: u32 = 1;

const SERVE_EVENTS: &[&str] = &["NewMessage", "MessageEdited"];

const SERVE_DEDUPE_CAP: usize = 10_000;

const SERVE_INTAKE_CAPACITY: usize = 64;
const MUTATE_QUEUE_CAPACITY: usize = 64;
const READ_QUEUE_CAPACITY: usize = 64;
const READ_POOL_SIZE: usize = 2;

pub(crate) const OP_TIMEOUT_SIMPLE: Duration = Duration::from_secs(30);
pub(crate) const OP_TIMEOUT_PAGINATED: Duration = Duration::from_secs(120);

pub(crate) struct ServeDedupe {
    seen: HashMap<(i64, i32, i32), ()>,
    order: VecDeque<(i64, i32, i32)>,
}

impl ServeDedupe {
    pub(crate) fn new() -> Self {
        Self {
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub(crate) fn check(&mut self, key: (i64, i32, i32)) -> bool {
        if self.seen.contains_key(&key) {
            return true;
        }
        if self.seen.len() >= SERVE_DEDUPE_CAP {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(key, ());
        self.order.push_back(key);
        false
    }
}

pub(crate) fn dedupe_key(chat_id: Option<i64>, msg_id: i32, pts: i32) -> (i64, i32, i32) {
    (chat_id.unwrap_or(0), msg_id, pts)
}

pub(crate) fn pts_from_state(state: &grammers_session::updates::State) -> i32 {
    match &state.message_box {
        Some(mb) => match mb {
            grammers_session::updates::MessageBox::Common { pts } => *pts,
            grammers_session::updates::MessageBox::Secondary { qts } => *qts,
            grammers_session::updates::MessageBox::Channel { pts, .. } => *pts,
        },
        None => 0,
    }
}

pub(crate) type ServeFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, serde_json::Value>> + Send>>;

pub(crate) type ServeRunner = fn(ServeShares, serde_json::Value) -> ServeFuture;

type Planner = fn(&str, serde_json::Value) -> Result<Plan, serde_json::Value>;

#[derive(Debug)]
pub(crate) enum Plan {
    DryRun(serde_json::Value),
    Execute(serde_json::Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Mutate,
    Read,
}

struct Job {
    id: u64,
    op: &'static str,
    timeout: Option<Duration>,
    future: ServeFuture,
}

pub(crate) struct OpRoute {
    pub(crate) op: &'static str,
    pub(crate) lane: Lane,
    pub(crate) timeout: Option<Duration>,
    pub(crate) planner: Planner,
    pub(crate) runner: ServeRunner,
}

#[derive(Parser)]
pub struct ServeArgs {
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "NewMessage",
        help = "events to stream (serve-A subset: NewMessage, MessageEdited)"
    )]
    events: Vec<String>,
    #[arg(
        long,
        default_value_t = false,
        help = "catch up from last persisted update state (replays missed history); default is live-only from now (no replay, no history)"
    )]
    catch_up: bool,
}

#[derive(Debug, PartialEq)]
pub enum ServeIn {
    Hello {
        protocol: u32,
    },
    Action {
        id: u64,
        op: String,
        params: serde_json::Value,
    },
}

pub(crate) fn err_json(kind: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "type": kind, "message": message.into() })
}

pub fn parse_incoming(line: &str) -> Result<ServeIn, serde_json::Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(err_json("ServeError", "empty line"));
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return Err(err_json("ParseError", format!("invalid JSON: {e}"))),
    };
    let Some(obj) = value.as_object() else {
        return Err(err_json("ParseError", "line must be a JSON object"));
    };
    let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or_default();
    if kind == "hello" {
        let protocol = obj
            .get("protocol")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| err_json("VersionMismatch", "hello requires an integer protocol"))?;
        let protocol = protocol as u32;
        if !(SERVE_PROTOCOL_MIN..=SERVE_PROTOCOL).contains(&protocol) {
            return Err(version_error(protocol));
        }
        return Ok(ServeIn::Hello { protocol });
    }
    let id = obj
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| err_json("ServeError", "request requires an integer id"))?;
    let op = obj
        .get("op")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if op.is_empty() {
        return Err(err_json("ServeError", "request requires a non-empty op"));
    }
    let params = match obj.get("params") {
        None => serde_json::json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err(err_json("ServeError", "params must be a JSON object")),
    };
    Ok(ServeIn::Action { id, op, params })
}

pub fn hello_out(account: &str, identity: Option<serde_json::Value>) -> serde_json::Value {
    let mut out = serde_json::json!({
        "type": "hello",
        "protocol": SERVE_PROTOCOL,
        "min_protocol": SERVE_PROTOCOL_MIN,
        "max_protocol": SERVE_PROTOCOL,
        "account": account,
    });
    if let Some(identity) = identity {
        out["identity"] = identity;
    }
    out
}

pub fn response_ok(id: u64, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "response",
        "id": id,
        "ok": true,
        "data": data,
    })
}

pub fn response_err(id: Option<u64>, error: serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert("type".into(), serde_json::Value::from("response"));
    if let Some(id) = id {
        out.insert("id".into(), serde_json::json!(id));
    }
    out.insert("ok".into(), serde_json::Value::from(false));
    out.insert("error".into(), error);
    serde_json::Value::Object(out)
}

fn version_error(theirs: u32) -> serde_json::Value {
    err_json(
        "VersionMismatch",
        format!(
            "script protocol {theirs} outside serve range {SERVE_PROTOCOL_MIN}-{SERVE_PROTOCOL}; update one side"
        ),
    )
}

fn identity_json(me: &grammers_client::peer::User) -> serde_json::Value {
    let phone = me.phone().map(crate::commands::account::redact_phone);
    serde_json::json!({
        "user_id": me.id().bare_id(),
        "username": me.username(),
        "first_name": me.first_name(),
        "phone_masked": phone,
    })
}

fn emit(value: &serde_json::Value) -> TeleResult<()> {
    output::print_json(value)
}

#[macro_export]
macro_rules! serve_runner {
    ($name:ident, $core:path, $params:ty) => {
        fn $name(
            shares: $crate::client::ServeShares,
            raw: serde_json::Value,
        ) -> $crate::commands::serve::ServeFuture {
            Box::pin(async move {
                let params: $params = serde_json::from_value(raw).map_err(|e| {
                    $crate::commands::serve::err_json("ServeError", format!("params: {e}"))
                })?;
                $core(&shares, params).await.map_err(|e| e.as_json())
            })
        }
    };
}

#[macro_export]
macro_rules! serve_route {
    ($op:literal, $lane:expr, $timeout:expr, $params:ty, $args:ty, $validate:path, $dry:path, $runner:expr) => {
        $crate::commands::serve::OpRoute {
            op: $op,
            lane: $lane,
            timeout: $timeout,
            planner: |op: &str, raw: serde_json::Value| {
                let parsed: $params = $crate::commands::serve::prep(op, raw.clone())?;
                let args: $args = (&parsed).into();
                $validate(&args).map_err(|e| e.as_json())?;
                if parsed.dry_run {
                    return Ok($crate::commands::serve::Plan::DryRun(
                        $dry(&args).map_err(|e| e.as_json())?,
                    ));
                }
                Ok($crate::commands::serve::Plan::Execute(raw))
            },
            runner: $runner,
        }
    };
}

pub(crate) fn serve_op_routes() -> Vec<OpRoute> {
    let mut routes = crate::commands::msg::msg_serve_routes();
    routes.extend(crate::commands::dialog::dialog_serve_routes());
    routes.extend(crate::commands::topic::topic_serve_routes());
    routes.extend(crate::commands::profile::profile_serve_routes());
    routes.extend(crate::commands::privacy::privacy_serve_routes());
    routes.extend(crate::commands::contact::contact_serve_routes());
    routes.extend(crate::commands::stickers::stickers_serve_routes());
    routes.extend(crate::commands::stories::stories_serve_routes());
    routes.extend(crate::commands::raw::raw_serve_routes());
    routes
}

pub(crate) fn prep<P: serde::de::DeserializeOwned>(
    op: &str,
    raw: serde_json::Value,
) -> Result<P, serde_json::Value> {
    serde_json::from_value(raw)
        .map_err(|e| err_json("ServeError", format!("op {op}: invalid params: {e}")))
}

fn known_op_names() -> String {
    let routes = serve_op_routes();
    let mut names: Vec<String> = routes.iter().map(|r| r.op.to_string()).collect();
    names.sort_unstable();
    names.join(", ")
}

fn not_implemented(op: &str) -> serde_json::Value {
    let ops = known_op_names();
    err_json(
        "NotImplemented",
        format!("unknown op {op}; supported ops: ping, {ops}"),
    )
}

fn find_route(op: &str) -> Option<OpRoute> {
    serve_op_routes().into_iter().find(|r| r.op == op)
}

async fn execute_job(job: Job) -> serde_json::Value {
    let Job {
        id,
        op,
        timeout,
        future,
    } = job;
    let outcome = match timeout {
        Some(limit) => match tokio::time::timeout(limit, future).await {
            Ok(outcome) => outcome,
            Err(_) => {
                return response_err(
                    Some(id),
                    err_json("Timeout", format!("op {op} timed out after {limit:?}")),
                )
            }
        },
        None => future.await,
    };
    match outcome {
        Ok(data) => response_ok(id, data),
        Err(error) => response_err(Some(id), error),
    }
}

type JobQueue = std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Job>>>;

async fn job_worker(
    jobs: JobQueue,
    responses: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
) {
    loop {
        let next = {
            let mut queue = jobs.lock().await;
            queue.recv().await
        };
        let Some(job) = next else {
            break;
        };
        let value = execute_job(job).await;
        if responses.send(value).is_err() {
            break;
        }
    }
}

fn job_queue(receiver: tokio::sync::mpsc::Receiver<Job>) -> JobQueue {
    std::sync::Arc::new(tokio::sync::Mutex::new(receiver))
}

async fn dispatch_action(
    shares: &ServeShares,
    mutations: &tokio::sync::mpsc::Sender<Job>,
    reads: &tokio::sync::mpsc::Sender<Job>,
    id: u64,
    op: &str,
    params: serde_json::Value,
) -> TeleResult<()> {
    if op == "ping" {
        return emit(&response_ok(id, serde_json::json!({ "pong": true })));
    }
    let Some(route) = find_route(op) else {
        return emit(&response_err(Some(id), not_implemented(op)));
    };
    match (route.planner)(op, params) {
        Err(error) => emit(&response_err(Some(id), error)),
        Ok(Plan::DryRun(data)) => emit(&response_ok(id, data)),
        Ok(Plan::Execute(raw)) => {
            let job = Job {
                id,
                op: route.op,
                timeout: route.timeout,
                future: (route.runner)(shares.clone(), raw),
            };
            let queue = match route.lane {
                Lane::Mutate => mutations,
                Lane::Read => reads,
            };
            if queue.send(job).await.is_err() {
                output::log_line("warn", "serve: op queue closed before dispatch");
            }
            Ok(())
        }
    }
}

fn event_kind_allowed(update: &Update, kinds: &[String]) -> Option<&'static str> {
    match update {
        Update::NewMessage(_) => kinds
            .iter()
            .any(|k| k == "NewMessage")
            .then_some("NewMessage"),
        Update::MessageEdited(_) => kinds
            .iter()
            .any(|k| k == "MessageEdited")
            .then_some("MessageEdited"),
        _ => None,
    }
}

pub async fn run(args: &ServeArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    for e in &args.events {
        if !SERVE_EVENTS.contains(&e.as_str()) {
            return Err(TeleError::Usage(format!(
                "unknown --events entry {e}; serve-A supports {} (full allowlist stays on tele listen)",
                SERVE_EVENTS.join(",")
            )));
        }
    }
    let mut names = crate::executor::select_accounts(flags)?;
    names.sort();
    names.dedup();
    if names.len() != 1 {
        return Err(TeleError::Usage(
            "tele serve owns exactly one session: pass exactly one --account <name>".to_string(),
        ));
    }
    let name = names.remove(0);
    if flags.dry_run {
        output::log_line("info", "[dry-run] would serve duplex JSONL");
        if flags.json || flags.jsonl {
            output::print_json_result(&event_row(
                "Serve",
                &name,
                None,
                None,
                Some(serde_json::json!({
                    "dry_run": true,
                    "would": format!("stream {} updates from account {name}", args.events.join(",")),
                    "protocol": SERVE_PROTOCOL,
                })),
            ))?;
        }
        return Ok(EXIT_OK);
    }

    let config_path = flags.config_path.clone();
    let creds = crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(SERVE_INTAKE_CAPACITY);
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let mut failures: u32 = 0;
    let mut greeted = false;
    let mut dedupe = ServeDedupe::new();
    loop {
        let eof = serve_connection(
            &name,
            &args.events,
            args.catch_up,
            &creds,
            config_path.as_deref(),
            &mut rx,
            &mut failures,
            greeted,
            &mut dedupe,
        )
        .await?;
        greeted = true;
        if eof {
            return Ok(EXIT_OK);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    name: &str,
    events: &[String],
    catch_up: bool,
    creds: &crate::config::Credentials,
    config_path: Option<&std::path::Path>,
    rx: &mut tokio::sync::mpsc::Receiver<String>,
    failures: &mut u32,
    greet: bool,
    dedupe: &mut ServeDedupe,
) -> TeleResult<bool> {
    let mut guard = loop {
        match crate::client::ClientGuard::connect(name, creds.api_id, config_path).await {
            Ok(guard) => break guard,
            Err(e) => {
                handle_stream_failure(name, TeleError::from(e), failures, None).await?;
            }
        }
    };
    crate::client::authorize(&guard.client).await?;
    let me = guard
        .client
        .get_me()
        .await
        .map_err(|e| TeleError::Other(format!("serve: get_me failed: {e}")))?;
    let identity = identity_json(&me);
    if let Err(e) = guard
        .client
        .invoke(&tl::functions::updates::GetState {})
        .await
    {
        let err = getstate_probe_error(e);
        if matches!(err, TeleError::Auth(_)) {
            return Err(err);
        }
        handle_stream_failure(name, err, failures, None).await?;
        return Ok(false);
    }
    let receiver = std::mem::replace(&mut guard.updates, tokio::sync::mpsc::unbounded_channel().1);
    let mut stream = guard
        .client
        .stream_updates(
            receiver,
            grammers_client::client::UpdatesConfiguration {
                catch_up,
                update_queue_limit: Some(1000),
            },
        )
        .await
        .map_err(|e| TeleError::Other(e.to_string()))?;
    *failures = 0;
    if greet {
        output::log_line("info", "serve: reconnected");
        emit(&event_row("Reconnected", name, None, None, None))?;
    } else {
        emit(&hello_out(name, Some(identity)))?;
    }
    let shares = guard.shares();
    let (mutations, mutation_rx) = tokio::sync::mpsc::channel::<Job>(MUTATE_QUEUE_CAPACITY);
    let (reads, read_rx) = tokio::sync::mpsc::channel::<Job>(READ_QUEUE_CAPACITY);
    let (response_tx, mut response_rx) =
        tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let mut workers = Vec::new();
    workers.push(tokio::spawn(job_worker(
        job_queue(mutation_rx),
        response_tx.clone(),
    )));
    let shared_reads = job_queue(read_rx);
    for _ in 0..READ_POOL_SIZE {
        workers.push(tokio::spawn(job_worker(
            std::sync::Arc::clone(&shared_reads),
            response_tx.clone(),
        )));
    }
    drop(response_tx);
    loop {
        enum Tick {
            Line(String),
            Eof,
            Update(Box<Update>),
            StreamError(grammers_client::InvocationError),
            Response(serde_json::Value),
        }
        let tick = tokio::select! {
            biased;
            line = rx.recv() => match line {
                Some(l) => Tick::Line(l),
                None => Tick::Eof,
            },
            res = tokio::time::timeout(
                poll_timeout(None, std::time::Instant::now()),
                stream.next(),
            ) => match res {
                Ok(Ok(u)) => Tick::Update(Box::new(u)),
                Ok(Err(e)) => Tick::StreamError(e),
                Err(_) => continue,
            },
            response = response_rx.recv() => match response {
                Some(value) => Tick::Response(value),
                None => continue,
            },
        };
        match tick {
            Tick::Eof => {
                output::log_line("info", "serve: stdin closed, shutting down");
                drop(mutations);
                drop(reads);
                for worker in workers.drain(..) {
                    let _ = worker.await;
                }
                while let Some(value) = response_rx.recv().await {
                    emit(&value)?;
                }
                guard.close().await;
                return Ok(true);
            }
            Tick::Line(line) => match parse_incoming(&line) {
                Err(error) => emit(&response_err(None, error))?,
                Ok(ServeIn::Hello { protocol }) => {
                    let _ = protocol;
                }
                Ok(ServeIn::Action { id, op, params }) => {
                    dispatch_action(&shares, &mutations, &reads, id, &op, params).await?
                }
            },
            Tick::Response(value) => emit(&value)?,
            Tick::StreamError(e) => {
                if crate::error::invocation_is_unauthorized(&e) {
                    return Err(crate::error::invocation_error(e));
                }
                handle_stream_failure(name, crate::error::invocation_error(e), failures, None)
                    .await?;
                return Ok(false);
            }
            Tick::Update(update) => {
                *failures = 0;
                let _ = stream.sync_update_state().await;
                let Some(kind) = event_kind_allowed(&update, events) else {
                    continue;
                };
                if is_empty_update(update.raw()) {
                    continue;
                }
                let message = match &*update {
                    Update::NewMessage(m) | Update::MessageEdited(m) => m,
                    _ => continue,
                };
                let peer = update_peer(update.raw());
                let chat_id = peer.and_then(|p| p.bare_id());
                let pts = pts_from_state(update.state());
                let key = dedupe_key(chat_id, message.id(), pts);
                if dedupe.check(key) {
                    continue;
                }
                let mut row = crate::serialize::message_to_json(message)?;
                crate::serialize::enrich_message_row(&mut row, message);
                crate::serialize::ensure_outer_peer_sender(&mut row, peer, None);
                emit(&event_row(kind, name, chat_id, None, Some(row)))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_session::Session;

    #[test]
    fn hello_roundtrip_parses_protocol() {
        let parsed = parse_incoming("{\"type\":\"hello\",\"protocol\":1}").unwrap();
        assert_eq!(parsed, ServeIn::Hello { protocol: 1 });
    }

    #[test]
    fn hello_requires_integer_protocol() {
        let err = parse_incoming("{\"type\":\"hello\",\"protocol\":\"x\"}").unwrap_err();
        assert_eq!(err["type"], "VersionMismatch");
        let err = parse_incoming("{\"type\":\"hello\"}").unwrap_err();
        assert_eq!(err["type"], "VersionMismatch");
    }

    #[test]
    fn action_parses_with_params_defaulting_to_object() {
        let parsed = parse_incoming("{\"id\":7,\"op\":\"msg.send\"}").unwrap();
        assert_eq!(
            parsed,
            ServeIn::Action {
                id: 7,
                op: "msg.send".into(),
                params: serde_json::json!({}),
            }
        );
        let parsed = parse_incoming(
            "{\"id\":7,\"op\":\"msg.send\",\"params\":{\"chat\":\"@x\",\"text\":\"hi\"}}",
        )
        .unwrap();
        assert_eq!(
            parsed,
            ServeIn::Action {
                id: 7,
                op: "msg.send".into(),
                params: serde_json::json!({"chat":"@x","text":"hi"}),
            }
        );
    }

    #[test]
    fn action_rejects_missing_or_bad_fields() {
        assert_eq!(
            parse_incoming("{\"op\":\"msg.send\"}").unwrap_err()["type"],
            "ServeError"
        );
        assert_eq!(
            parse_incoming("{\"id\":7,\"op\":\"\"}").unwrap_err()["type"],
            "ServeError"
        );
        assert_eq!(
            parse_incoming("{\"id\":7,\"op\":\"msg.send\",\"params\":[1]}").unwrap_err()["type"],
            "ServeError"
        );
    }

    #[test]
    fn malformed_lines_are_error_values_not_panics() {
        for line in ["{bad", "", "   ", "[1,2]", "\"just a string\""] {
            let err = parse_incoming(line).unwrap_err();
            assert!(
                err["type"] == "ParseError" || err["type"] == "ServeError",
                "line {line:?} gave {err}"
            );
        }
    }

    #[test]
    fn response_shapes_carry_id_and_ok() {
        let ok = response_ok(9, serde_json::json!({"sent": true}));
        assert_eq!(ok["type"], "response");
        assert_eq!(ok["id"], 9);
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["data"]["sent"], true);

        let err = response_err(Some(3), err_json("NotImplemented", "later"));
        assert_eq!(err["id"], 3);
        assert_eq!(err["ok"], false);
        assert_eq!(err["error"]["type"], "NotImplemented");

        let unattributed = response_err(None, err_json("ParseError", "nope"));
        assert!(unattributed.get("id").is_none());
        assert_eq!(unattributed["ok"], false);
    }

    #[test]
    fn hello_out_carries_protocol_account_range_and_identity() {
        let hello = hello_out(
            "work",
            Some(serde_json::json!({"user_id": 7, "username": "w"})),
        );
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["protocol"], SERVE_PROTOCOL);
        assert_eq!(hello["min_protocol"], SERVE_PROTOCOL_MIN);
        assert_eq!(hello["max_protocol"], SERVE_PROTOCOL);
        assert_eq!(hello["account"], "work");
        assert_eq!(hello["identity"]["user_id"], 7);

        let bare = hello_out("work", None);
        assert!(bare.get("identity").is_none());
    }

    #[test]
    fn version_mismatch_message_names_range() {
        let msg = version_error(SERVE_PROTOCOL + 1)["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains(&SERVE_PROTOCOL_MIN.to_string())
                && msg.contains(&SERVE_PROTOCOL.to_string())
        );
    }

    #[test]
    fn negotiated_hello_accepts_in_range_and_rejects_out_of_range() {
        let in_range = parse_incoming(r#"{"type":"hello","protocol":1}"#).unwrap();
        assert_eq!(in_range, ServeIn::Hello { protocol: 1 });
        let too_old_and_too_new = [
            r#"{"type":"hello","protocol":0}"#,
            r#"{"type":"hello","protocol":99}"#,
        ];
        for line in too_old_and_too_new {
            let err = parse_incoming(line).unwrap_err();
            assert_eq!(err["type"], "VersionMismatch", "{line}");
        }
    }

    #[test]
    fn serve_events_subset_is_locked() {
        assert_eq!(SERVE_EVENTS, &["NewMessage", "MessageEdited"]);
    }

    #[test]
    fn ping_op_is_dispatched_as_pong() {
        let parsed = parse_incoming("{\"id\":11,\"op\":\"ping\"}").unwrap();
        match parsed {
            ServeIn::Action { id, op, .. } => {
                assert_eq!((id, op.as_str()), (11, "ping"));
                assert_eq!(
                    response_ok(id, serde_json::json!({"pong": true}))["data"]["pong"],
                    true
                );
            }
            other => panic!("expected action, got {other:?}"),
        }
    }

    fn plan_for(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
        let route = find_route(op).unwrap_or_else(|| panic!("route missing for {op}"));
        let planner = route.planner;
        planner(op, params)
    }

    #[test]
    fn routes_table_is_locked_command_path_form() {
        let expected = [
            "msg click",
            "msg delete",
            "msg download",
            "msg edit",
            "msg forward",
            "msg get",
            "msg pin",
            "msg react",
            "msg read",
            "msg search",
            "msg send",
            "msg typing",
            "msg vote",
        ];
        let mut actual: Vec<String> = serve_op_routes().iter().map(|r| r.op.to_string()).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        assert!(serve_op_routes().iter().all(|r| r.op.starts_with("msg ")
            && !r.op.contains('.')
            && r.op
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == ' ' || c.is_ascii_digit())));
    }

    #[test]
    fn unknown_op_maps_to_not_implemented_listing_known_ops() {
        assert!(find_route("msg.send").is_none());
        assert!(find_route("chat join").is_none());
        assert!(find_route("ping").is_none());
        let err = not_implemented("msg.send");
        assert_eq!(err["type"], "NotImplemented");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("msg.send"), "{msg}");
        assert!(msg.contains("msg send"), "{msg}");
        assert!(msg.contains("ping"), "{msg}");
    }

    #[test]
    fn missing_required_param_yields_serve_error_naming_field() {
        let err = plan_for("msg edit", serde_json::json!({"chat": "@x"})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("msg edit"), "{msg}");
        assert!(msg.contains("id"), "{msg}");
        assert!(msg.contains("missing field"), "{msg}");

        let err = plan_for("msg react", serde_json::json!({"chat": "@x"})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err["message"].as_str().unwrap().contains("id"));
    }

    #[test]
    fn defaulted_params_missing_key_fall_through_to_validation() {
        let err = plan_for("msg send", serde_json::json!({"text": "hi"})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--chat"));

        let err = plan_for(
            "msg search",
            serde_json::json!({"global": false, "query": "x"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
    }

    #[test]
    fn wrong_typed_param_yields_serve_error_with_serde_message() {
        let err = plan_for(
            "msg edit",
            serde_json::json!({"chat": "@x", "id": "abc", "text": "t"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("msg edit"), "{msg}");
        assert!(msg.contains("invalid type"), "{msg}");
        assert!(msg.contains("i32"), "{msg}");

        let err = plan_for(
            "msg get",
            serde_json::json!({"chat": "@x", "limit": "many"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err["message"].as_str().unwrap().contains("u32"));
    }

    #[test]
    fn unknown_param_yields_serve_error_naming_field() {
        let err = plan_for(
            "msg send",
            serde_json::json!({"chat": "@x", "mesage": "typo"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("mesage"), "{msg}");
    }

    #[test]
    fn validation_failure_yields_usage_error_envelope() {
        let err = plan_for(
            "msg send",
            serde_json::json!({"chat": "@x", "text": "hi", "format": "html"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--format"),);

        let err = plan_for(
            "msg react",
            serde_json::json!({"chat": "", "id": 1, "remove": true}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("msg click", serde_json::json!({"chat": "@x", "id": 3})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--button"));
    }

    #[test]
    fn dry_run_param_roundtrips_through_planner_without_guard() {
        let plan = plan_for(
            "msg react",
            serde_json::json!({"chat": "@game", "id": 5, "reaction": "+1", "dry_run": true}),
        )
        .unwrap();
        let Plan::DryRun(data) = plan else {
            panic!("expected dry run plan");
        };
        assert_eq!(data["dry_run"], true);
        assert_eq!(data["id"], 5);
        assert_eq!(data["would"], "react +1 to message 5");

        let plan = plan_for(
            "msg delete",
            serde_json::json!({"chat": "@game", "all": true, "dry_run": true}),
        )
        .unwrap();
        let Plan::DryRun(data) = plan else {
            panic!("expected dry run plan");
        };
        assert_eq!(data["would"], "delete all messages in chat @game");
        assert_eq!(data["self_only"], false);

        let plan = plan_for(
            "msg send",
            serde_json::json!({"chat": "@game", "text": "gl hf", "dry_run": true}),
        )
        .unwrap();
        let Plan::DryRun(data) = plan else {
            panic!("expected dry run plan");
        };
        assert_eq!(data["dry_run"], true);
        assert_eq!(data["text"], "gl hf");
        assert_eq!(data["preview"], true);
        assert_eq!(data["format"], "plain");
    }

    #[test]
    fn execute_plan_carries_raw_params_to_runner() {
        let raw = serde_json::json!({"chat": "@game", "id": 7, "button_index": 2});
        let plan = plan_for("msg click", raw.clone()).unwrap();
        match plan {
            Plan::Execute(passed) => assert_eq!(passed, raw),
            other => panic!("expected execute plan, got {other:?}"),
        }
    }

    #[test]
    fn response_err_wraps_core_tele_errors_additively() {
        let flood = TeleError::Rpc(
            "rpc error 420: FLOOD_WAIT (value: 17)".to_string(),
            420,
            "FLOOD_WAIT".to_string(),
            Some(17),
        );
        let envelope = response_err(Some(9), flood.as_json());
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["id"], 9);
        assert_eq!(envelope["error"]["name"], "FLOOD_WAIT");
        assert_eq!(envelope["error"]["seconds"], 17);
    }

    #[test]
    fn serve_default_is_live_only_catch_up_off() {
        let args = ServeArgs::try_parse_from(["serve"]).unwrap();
        assert!(!args.catch_up);
        let args2 = ServeArgs::try_parse_from(["serve", "--catch-up"]).unwrap();
        assert!(args2.catch_up);
    }

    #[test]
    fn serve_help_mentions_live_only_and_catch_up() {
        use clap::CommandFactory;
        let mut cmd = ServeArgs::command();
        let mut buf = Vec::new();
        cmd.write_help(&mut buf).unwrap();
        let help = String::from_utf8(buf).unwrap();
        assert!(help.contains("--catch-up"));
        assert!(help.contains("live-only"));
        assert!(help.contains("no replay"));
    }

    #[test]
    fn serve_dedupe_suppresses_replay_and_bounces_flaky_reconnect() {
        let mut d = ServeDedupe::new();
        let k1 = dedupe_key(Some(123), 5, 10);
        let k2 = dedupe_key(Some(123), 5, 11);
        let k3 = dedupe_key(Some(456), 5, 10);
        assert!(!d.check(k1));
        assert!(d.check(k1));
        assert!(!d.check(k2));
        assert!(d.check(k2));
        assert!(!d.check(k3));
        assert!(d.check(k3));
        assert!(!d.check(dedupe_key(None, 9, 0)));
        assert!(d.check(dedupe_key(None, 9, 0)));
    }

    #[test]
    fn serve_dedupe_evicts_oldest_beyond_cap() {
        let mut d = ServeDedupe::new();
        for i in 0..(SERVE_DEDUPE_CAP as i32) {
            assert!(!d.check(dedupe_key(Some(1), i, i)));
        }
        assert_eq!(d.seen.len(), SERVE_DEDUPE_CAP);
        let first = dedupe_key(Some(1), 0, 0);
        assert!(d.check(first));
        assert!(!d.check(dedupe_key(
            Some(1),
            SERVE_DEDUPE_CAP as i32,
            SERVE_DEDUPE_CAP as i32
        )));
        assert_eq!(d.seen.len(), SERVE_DEDUPE_CAP);
    }

    #[test]
    fn pts_from_state_reads_all_box_variants() {
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
    async fn serve_state_persists_and_resumes_offline() {
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!(
            "telecli-serve-state-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        std::fs::write(dir.join("config.toml"), "[accounts.serve_test]\n").unwrap();
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let path = crate::session::session_path("serve_test");
        {
            let sess = grammers_session::storages::SqliteSession::open(&path)
                .await
                .unwrap();
            sess.set_update_state(grammers_session::types::UpdateState::All(
                grammers_session::types::UpdatesState {
                    pts: 77,
                    qts: 0,
                    date: 1000,
                    seq: 1,
                    channels: vec![],
                },
            ))
            .await
            .unwrap();
            let state = sess.updates_state().await.unwrap();
            let mbox = grammers_session::updates::MessageBoxes::load(state);
            assert!(!mbox.is_empty());
            assert_eq!(mbox.session_state().pts, 77);
        }
        {
            let sess2 = grammers_session::storages::SqliteSession::open(&path)
                .await
                .unwrap();
            let resumed = sess2.updates_state().await.unwrap();
            assert_eq!(resumed.pts, 77);
            let mbox2 = grammers_session::updates::MessageBoxes::load(resumed);
            assert_eq!(mbox2.session_state().pts, 77);
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn route_for(op: &str) -> OpRoute {
        find_route(op).unwrap_or_else(|| panic!("route missing for {op}"))
    }

    #[test]
    fn lane_and_timeout_table_is_locked() {
        let expected: &[(&str, Lane, Option<u64>)] = &[
            ("msg click", Lane::Mutate, Some(30)),
            ("msg delete", Lane::Mutate, Some(30)),
            ("msg download", Lane::Read, None),
            ("msg edit", Lane::Mutate, Some(30)),
            ("msg forward", Lane::Mutate, Some(30)),
            ("msg get", Lane::Read, Some(120)),
            ("msg pin", Lane::Mutate, Some(30)),
            ("msg read", Lane::Mutate, Some(30)),
            ("msg react", Lane::Mutate, Some(30)),
            ("msg search", Lane::Read, Some(120)),
            ("msg send", Lane::Mutate, Some(30)),
            ("msg typing", Lane::Mutate, Some(30)),
            ("msg vote", Lane::Mutate, Some(30)),
        ];
        assert_eq!(serve_op_routes().len(), expected.len());
        for (op, lane, secs) in expected {
            let route = route_for(op);
            assert_eq!(route.lane, *lane, "lane for {op}");
            assert_eq!(
                route.timeout,
                secs.map(Duration::from_secs),
                "timeout for {op}"
            );
        }
    }

    fn scripted_job(id: u64, delay_ms: u64, timeout: Option<Duration>) -> Job {
        Job {
            id,
            op: "msg send",
            timeout,
            future: Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(serde_json::json!({ "id": id }))
            }),
        }
    }

    async fn next_response(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    ) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("response overdue")
            .expect("response channel closed")
    }

    #[tokio::test]
    async fn mutate_lane_preserves_submission_order_reads_run_concurrently() {
        let (mutation_tx, mutation_rx) = tokio::sync::mpsc::channel::<Job>(8);
        let (read_tx, read_rx) = tokio::sync::mpsc::channel::<Job>(8);
        let (response_tx, mut response_rx) =
            tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let mut workers = vec![tokio::spawn(job_worker(
            job_queue(mutation_rx),
            response_tx.clone(),
        ))];
        let shared_reads = job_queue(read_rx);
        for _ in 0..READ_POOL_SIZE {
            workers.push(tokio::spawn(job_worker(
                std::sync::Arc::clone(&shared_reads),
                response_tx.clone(),
            )));
        }
        drop(response_tx);

        mutation_tx.send(scripted_job(1, 90, None)).await.unwrap();
        mutation_tx.send(scripted_job(2, 1, None)).await.unwrap();
        mutation_tx.send(scripted_job(3, 1, None)).await.unwrap();
        read_tx.send(scripted_job(4, 100, None)).await.unwrap();
        read_tx.send(scripted_job(5, 100, None)).await.unwrap();
        drop(mutation_tx);
        drop(read_tx);

        let started = std::time::Instant::now();
        let mut arrivals = Vec::new();
        for _ in 0..5 {
            let value = next_response(&mut response_rx).await;
            assert_eq!(value["ok"], true);
            arrivals.push(value["id"].as_u64().unwrap());
        }
        let elapsed = started.elapsed();

        for worker in workers {
            worker.await.unwrap();
        }
        assert!(response_rx.recv().await.is_none());

        let mutate_order: Vec<u64> = arrivals.iter().copied().filter(|id| *id <= 3).collect();
        assert_eq!(mutate_order, vec![1, 2, 3]);
        assert_eq!(arrivals.len(), 5);
        assert!(
            elapsed < Duration::from_millis(175),
            "reads did not overlap the mutation lane: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn execute_job_enforces_timeout_with_correlated_envelope() {
        let slow = scripted_job(42, 60_000, Some(Duration::from_millis(20)));
        let value = execute_job(slow).await;
        assert_eq!(value["type"], "response");
        assert_eq!(value["id"], 42);
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["type"], "Timeout");
        let message = value["error"]["message"].as_str().unwrap();
        assert!(message.contains("msg send"), "{message}");
        assert!(message.contains("20ms"), "{message}");
    }

    #[tokio::test]
    async fn execute_job_passes_core_outcomes_through_unbounded_reads() {
        let ok_job = Job {
            id: 7,
            op: "msg download",
            timeout: None,
            future: Box::pin(async { Ok(serde_json::json!({ "bytes": 11 })) }),
        };
        let value = execute_job(ok_job).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["id"], 7);
        assert_eq!(value["data"]["bytes"], 11);

        let err_job = Job {
            id: 8,
            op: "msg download",
            timeout: None,
            future: Box::pin(async { Err(err_json("ServeError", "boom")) }),
        };
        let value = execute_job(err_job).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["id"], 8);
        assert_eq!(value["error"]["type"], "ServeError");
    }
}
