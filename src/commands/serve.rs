use clap::Parser;
use grammers_client::tl;
use grammers_client::update::Update;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use crate::client::ServeShares;
use crate::commands::listen::{
    event_row, getstate_probe_error, handle_stream_failure, is_empty_update, poll_timeout,
    update_peer,
};
use crate::error::{TeleError, TeleResult, EXIT_OK};
use crate::executor::GlobalFlags;
use crate::output;
use std::sync::OnceLock;
use std::time::Duration;

pub const SERVE_PROTOCOL: u32 = 1;
pub const SERVE_PROTOCOL_MIN: u32 = 1;

const SERVE_EVENTS: &[&str] = &["NewMessage", "MessageEdited"];

const SERVE_DEDUPE_CAP: usize = 10_000;

const SERVE_INTAKE_CAPACITY: usize = 64;
const MUTATE_QUEUE_CAPACITY: usize = 64;
const READ_QUEUE_CAPACITY: usize = 64;
const READ_POOL_SIZE: usize = 2;
const RESPONSE_CAPACITY: usize = 64;
const SERVE_MAX_ACCOUNTS: usize = 32;
const ACCOUNT_TICK_CAPACITY: usize = 256;

type ShareMap = std::sync::RwLock<HashMap<String, ServeShares>>;

pub(crate) struct ServePool {
    shares: ShareMap,
    resync: HashMap<String, tokio::sync::mpsc::Sender<()>>,
}

impl ServePool {
    fn shares_for(&self, account: &str) -> Result<ServeShares, serde_json::Value> {
        self.shares
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(account)
            .cloned()
            .ok_or_else(|| {
                err_json(
                    "ServeError",
                    format!("account {account} is not connected right now"),
                )
            })
    }

    fn publish(&self, account: &str, shares: ServeShares) {
        self.shares
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_string(), shares);
    }

    fn served(&self) -> Vec<String> {
        let mut names: Vec<String> = self.resync.keys().cloned().collect();
        names.sort();
        names
    }
}

enum AccountTick {
    Ready {
        name: String,
        identity: Option<serde_json::Value>,
    },
    Reconnected {
        name: String,
    },
    Event {
        ev: serde_json::Value,
    },
    Fatal {
        err: TeleError,
    },
}

pub(crate) const OP_TIMEOUT_SIMPLE: Duration = Duration::from_secs(30);
pub(crate) const OP_TIMEOUT_PAGINATED: Duration = Duration::from_secs(120);

pub(crate) struct ServeDedupe {
    seen: HashMap<(String, i64, i32, i32), ()>,
    order: VecDeque<(String, i64, i32, i32)>,
}

impl ServeDedupe {
    pub(crate) fn new() -> Self {
        Self {
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub(crate) fn check(&mut self, key: (String, i64, i32, i32)) -> bool {
        if self.seen.contains_key(&key) {
            return true;
        }
        if self.seen.len() >= SERVE_DEDUPE_CAP {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(key.clone(), ());
        self.order.push_back(key);
        false
    }
}

pub(crate) fn dedupe_key(
    account: &str,
    chat_id: Option<i64>,
    msg_id: i32,
    pts: i32,
) -> (String, i64, i32, i32) {
    (account.to_string(), chat_id.unwrap_or(0), msg_id, pts)
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

pub(crate) type Planner = fn(&str, serde_json::Value) -> Result<Plan, serde_json::Value>;

pub(crate) type SchemaFn = fn() -> serde_json::Value;

pub(crate) fn params_schema<P: rmcp::schemars::JsonSchema>() -> serde_json::Value {
    use rmcp::schemars::generate::SchemaSettings;
    let gen = SchemaSettings::draft2020_12().into_generator();
    let root = gen.into_root_schema_for::<P>();
    serde_json::to_value(root).unwrap_or_else(|_| serde_json::json!({}))
}

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

pub(crate) struct JobCompletionGuard {
    pub(crate) id: u64,
    pub(crate) op: &'static str,
    pub(crate) responses: tokio::sync::mpsc::Sender<serde_json::Value>,
    pub(crate) completed: bool,
}

impl Drop for JobCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self
                .responses
                .try_send(undelivered_envelope(self.id, self.op));
        }
    }
}

struct Job {
    id: u64,
    op: &'static str,
    timeout: Option<Duration>,
    future: ServeFuture,
    guard: JobCompletionGuard,
}

#[allow(unpredictable_function_pointer_comparisons)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpRoute {
    pub(crate) op: &'static str,
    pub(crate) lane: Lane,
    pub(crate) timeout: Option<Duration>,
    pub(crate) read_only: bool,
    pub(crate) destructive: bool,
    pub(crate) retry_safe: bool,
    pub(crate) summary: &'static str,
    pub(crate) planner: Planner,
    pub(crate) runner: ServeRunner,
    pub(crate) schema_fn: SchemaFn,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RouteTable {
    map: HashMap<&'static str, OpRoute>,
    names: String,
}

impl RouteTable {
    pub(crate) fn names(&self) -> &str {
        &self.names
    }
    pub(crate) fn find(&self, op: &str) -> Option<OpRoute> {
        self.map.get(op).copied()
    }
}

pub(crate) fn build_route_table() -> RouteTable {
    let routes = serve_op_routes();
    let mut map = HashMap::with_capacity(routes.len());
    let mut names: Vec<String> = Vec::with_capacity(routes.len());
    for r in routes {
        names.push(r.op.to_string());
        map.insert(r.op, r);
    }
    names.sort_unstable();
    let names = names.join(", ");
    RouteTable { map, names }
}

pub(crate) fn route_table() -> &'static RouteTable {
    static TABLE: OnceLock<RouteTable> = OnceLock::new();
    TABLE.get_or_init(build_route_table)
}

pub(crate) fn undelivered_envelope(id: u64, op: &str) -> serde_json::Value {
    response_err(
        Some(id),
        err_json(
            "StreamDown",
            format!("op {op} abandoned: stream down before response"),
        ),
    )
}

const INLINE_OPS: &[(&str, &str, bool, bool, bool)] = &[
    (
        "ops.list",
        "list every serve op with its hints and summary",
        true,
        false,
        true,
    ),
    ("ping", "liveness probe returning pong", true, false, true),
    (
        "stream.resync",
        "restart the update stream with catch-up replay",
        true,
        false,
        true,
    ),
];

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
        let protocol = match u32::try_from(protocol) {
            Ok(p) => p,
            Err(_) => return Err(version_error(u32::MAX)),
        };
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

pub(crate) fn apply_seq(row: &mut serde_json::Value, counter: &mut u64) -> u64 {
    *counter += 1;
    row["seq"] = serde_json::json!(*counter);
    *counter
}

pub fn hello_out_accounts(
    accounts: &[(String, Option<serde_json::Value>)],
    last_seq: Option<u64>,
) -> serde_json::Value {
    let (default_name, default_identity) = accounts
        .first()
        .cloned()
        .unwrap_or_else(|| (String::new(), None));
    let mut out = serde_json::json!({
        "type": "hello",
        "protocol": SERVE_PROTOCOL,
        "min_protocol": SERVE_PROTOCOL_MIN,
        "max_protocol": SERVE_PROTOCOL,
        "account": default_name,
        "last_seq": last_seq,
    });
    if let Some(identity) = default_identity {
        out["identity"] = identity;
    }
    let listed: Vec<serde_json::Value> = accounts
        .iter()
        .map(|(name, identity)| {
            let mut row = serde_json::json!({ "name": name });
            if let Some(identity) = identity {
                row["identity"] = identity.clone();
            }
            row
        })
        .collect();
    out["accounts"] = serde_json::Value::Array(listed);
    out
}

pub(crate) fn select_op_account(
    served: &[String],
    params: &mut serde_json::Value,
) -> Result<String, serde_json::Value> {
    let requested = match params.get("account") {
        None => None,
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(err_json(
                    "ServeError",
                    "account must be a non-empty string when present",
                ));
            }
            Some(trimmed.to_string())
        }
        Some(_) => {
            return Err(err_json(
                "ServeError",
                "account must be a string naming one of the served accounts",
            ))
        }
    };
    if let Some(obj) = params.as_object_mut() {
        obj.remove("account");
    }
    match requested {
        None => {
            if served.len() > 1 {
                Err(err_json(
                    "ServeError",
                    format!(
                        "account is required when multiple accounts are served; this connection serves: {}",
                        served.join(", ")
                    ),
                ))
            } else {
                Ok(served.first().cloned().unwrap_or_default())
            }
        }
        Some(name) => {
            if served.iter().any(|s| s == &name) {
                Ok(name)
            } else {
                Err(err_json(
                    "ServeError",
                    format!(
                        "unknown account {name}; this connection serves: {}",
                        served.join(", ")
                    ),
                ))
            }
        }
    }
}

pub(crate) fn resync_targets(
    served: &[String],
    params: &mut serde_json::Value,
) -> Result<Vec<String>, serde_json::Value> {
    if params.get("account").is_some() || served.len() <= 1 {
        select_op_account(served, params).map(|a| vec![a])
    } else {
        Ok(served.to_vec())
    }
}

pub(crate) fn validate_serve_selection(names: &[String]) -> TeleResult<()> {
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
    if names.len() > SERVE_MAX_ACCOUNTS {
        return Err(TeleError::Usage(format!(
            "tele serve supports at most {SERVE_MAX_ACCOUNTS} accounts per process, got {}",
            names.len()
        )));
    }
    Ok(())
}

fn last_seq_of(seq: &u64) -> Option<u64> {
    if *seq == 0 {
        None
    } else {
        Some(*seq)
    }
}

pub(crate) struct DeserFailure {
    pub(crate) message: String,
    pub(crate) param: Option<String>,
}

fn field_from_serde_message(msg: &str) -> Option<String> {
    for pat in ["unknown field `", "missing field `", "duplicate field `"] {
        if let Some((_, rest)) = msg.split_once(pat) {
            if let Some(name) = rest.split('`').next() {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

fn is_type_error(msg: &str) -> bool {
    msg.contains("invalid type") || msg.contains("invalid value")
}

fn probe_typed_offender<P: serde::de::DeserializeOwned>(raw: &serde_json::Value) -> Option<String> {
    let obj = raw.as_object()?;
    for key in obj.keys() {
        let mut probe = obj.clone();
        probe.remove(key);
        let blames_key = match P::deserialize(&serde_json::Value::Object(probe)) {
            Ok(_) => true,
            Err(e) => !is_type_error(&e.to_string()),
        };
        if blames_key {
            return Some(key.clone());
        }
    }
    None
}

pub(crate) fn deser_params<P: serde::de::DeserializeOwned>(
    raw: &serde_json::Value,
) -> Result<P, DeserFailure> {
    P::deserialize(raw).map_err(|e| {
        let message = e.to_string();
        let mut param = field_from_serde_message(&message);
        if param.is_none() && is_type_error(&message) {
            param = probe_typed_offender::<P>(raw);
        }
        DeserFailure { message, param }
    })
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

async fn emit(value: &serde_json::Value) -> TeleResult<()> {
    let line = serde_json::to_string(value)?;
    tokio::task::spawn_blocking(move || {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")?;
        out.flush()
    })
    .await
    .map_err(|e| TeleError::TaskPanic(e.to_string()))??;
    Ok(())
}

#[macro_export]
macro_rules! serve_runner {
    ($name:ident, $core:path, $params:ty) => {
        fn $name(
            shares: $crate::client::ServeShares,
            raw: serde_json::Value,
        ) -> $crate::commands::serve::ServeFuture {
            Box::pin(async move {
                let params: $params = $crate::commands::serve::deser_params(&raw).map_err(|f| {
                    let mut err = $crate::commands::serve::err_json(
                        "ServeError",
                        format!("params: {}", f.message),
                    );
                    if let Some(p) = f.param {
                        err["param"] = serde_json::Value::from(p);
                    }
                    err
                })?;
                $core(&shares, params).await.map_err(|e| e.as_json())
            })
        }
    };
}

#[macro_export]
macro_rules! serve_route {
    ($op:literal, $lane:expr, $timeout:expr, $read_only:expr, $destructive:expr, $retry_safe:expr, $summary:literal, $params:ty, $args:ty, $validate:expr, $dry:expr, $runner:expr, $schema:expr) => {
        $crate::commands::serve::OpRoute {
            op: $op,
            lane: $lane,
            timeout: $timeout,
            read_only: $read_only,
            destructive: $destructive,
            retry_safe: $retry_safe,
            summary: $summary,
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
            schema_fn: $schema,
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
    routes.extend(crate::commands::chat::chat_serve_routes());
    routes.extend(crate::commands::account::account_serve_routes());
    routes
}

pub(crate) fn prep<P: serde::de::DeserializeOwned>(
    op: &str,
    raw: serde_json::Value,
) -> Result<P, serde_json::Value> {
    deser_params(&raw).map_err(|f| {
        let mut err = err_json(
            "ServeError",
            format!("op {op}: invalid params: {}", f.message),
        );
        if let Some(p) = f.param {
            err["param"] = serde_json::Value::from(p);
        }
        err
    })
}

fn not_implemented(op: &str) -> serde_json::Value {
    err_json(
        "NotImplemented",
        format!(
            "unknown op {op}; supported ops: ping, stream.resync, ops.list, {}",
            route_table().names()
        ),
    )
}

fn find_route(op: &str) -> Option<OpRoute> {
    route_table().find(op)
}

async fn execute_job(job: Job) -> serde_json::Value {
    let Job {
        id,
        op,
        timeout,
        future,
        mut guard,
    } = job;
    let outcome = match timeout {
        Some(limit) => match tokio::time::timeout(limit, future).await {
            Ok(outcome) => outcome,
            Err(_) => {
                guard.completed = true;
                return response_err(
                    Some(id),
                    err_json("Timeout", format!("op {op} timed out after {limit:?}")),
                );
            }
        },
        None => future.await,
    };
    let value = match outcome {
        Ok(data) => response_ok(id, data),
        Err(error) => response_err(Some(id), error),
    };
    guard.completed = true;
    value
}

async fn job_worker(
    mut jobs: tokio::sync::mpsc::Receiver<Job>,
    responses: tokio::sync::mpsc::Sender<serde_json::Value>,
) {
    while let Some(job) = jobs.recv().await {
        let value = execute_job(job).await;
        if responses.send(value).await.is_err() {
            break;
        }
    }
}

fn validate_no_params(op: &str, params: &serde_json::Value) -> Result<(), serde_json::Value> {
    let empty = params.as_object().map(|o| o.is_empty()).unwrap_or(false);
    if empty {
        Ok(())
    } else {
        Err(err_json("ServeError", format!("{op} takes no parameters")))
    }
}

fn op_group(op: &str) -> &str {
    if op.contains('.') {
        "transport"
    } else {
        op.split(' ').next().unwrap_or(op)
    }
}

pub(crate) fn ops_list_data() -> serde_json::Value {
    let mut rows: Vec<(String, serde_json::Value)> = Vec::new();
    for r in serve_op_routes() {
        rows.push((
            r.op.to_string(),
            serde_json::json!({
                "op": r.op,
                "summary": r.summary,
                "group": op_group(r.op),
                "read_only": r.read_only,
                "destructive": r.destructive,
                "retry_safe": r.retry_safe,
            }),
        ));
    }
    for (op, summary, read_only, destructive, retry_safe) in INLINE_OPS {
        rows.push((
            (*op).to_string(),
            serde_json::json!({
                "op": op,
                "summary": summary,
                "group": "transport",
                "read_only": read_only,
                "destructive": destructive,
                "retry_safe": retry_safe,
            }),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    serde_json::json!({ "ops": rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>() })
}

fn confirm_requested(raw: &serde_json::Value) -> bool {
    matches!(raw.get("confirm"), Some(serde_json::Value::Bool(true)))
}

fn strip_confirm(raw: &mut serde_json::Value) {
    if let Some(obj) = raw.as_object_mut() {
        obj.remove("confirm");
    }
}

fn confirm_required_error(op: &str, would: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "ConfirmRequired",
        "message": format!("op {op} is destructive and requires confirm:true"),
        "would": would,
    })
}

pub(crate) fn apply_confirm_gate(
    op: &str,
    destructive: bool,
    planner: Planner,
    raw: &mut serde_json::Value,
) -> Result<(), serde_json::Value> {
    if destructive && !confirm_requested(raw) {
        let mut probe = raw.clone();
        if let Some(obj) = probe.as_object_mut() {
            obj.insert("dry_run".to_string(), serde_json::Value::Bool(true));
        }
        let would = match planner(op, probe) {
            Ok(Plan::DryRun(data)) => data,
            _ => serde_json::json!({}),
        };
        strip_confirm(raw);
        return Err(confirm_required_error(op, would));
    }
    strip_confirm(raw);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_action(
    pool: &ServePool,
    served: &[String],
    mutations: &tokio::sync::mpsc::Sender<Job>,
    reads: &[tokio::sync::mpsc::Sender<Job>],
    read_counter: &std::sync::atomic::AtomicUsize,
    responses: &tokio::sync::mpsc::Sender<serde_json::Value>,
    id: u64,
    op: &str,
    params: serde_json::Value,
) -> TeleResult<()> {
    if op == "ping" {
        let _ = responses
            .send(response_ok(id, serde_json::json!({ "pong": true })))
            .await;
        return Ok(());
    }
    if op == "stream.resync" {
        let mut params = params;
        let selected = match resync_targets(served, &mut params) {
            Ok(a) => a,
            Err(error) => {
                let _ = responses.send(response_err(Some(id), error)).await;
                return Ok(());
            }
        };
        if let Err(error) = validate_no_params(op, &params) {
            let _ = responses.send(response_err(Some(id), error)).await;
            return Ok(());
        }
        for name in &selected {
            if let Some(tx) = pool.resync.get(name) {
                let _ = tx.send(()).await;
            }
        }
        let _ = responses
            .send(response_ok(id, serde_json::json!({ "resync": "started" })))
            .await;
        return Ok(());
    }
    if op == "ops.list" {
        if let Err(error) = validate_no_params(op, &params) {
            let _ = responses.send(response_err(Some(id), error)).await;
            return Ok(());
        }
        let _ = responses.send(response_ok(id, ops_list_data())).await;
        return Ok(());
    }
    let Some(route) = find_route(op) else {
        let _ = responses
            .send(response_err(Some(id), not_implemented(op)))
            .await;
        return Ok(());
    };
    let mut raw = params;
    let account = match select_op_account(served, &mut raw) {
        Ok(a) => a,
        Err(error) => {
            let _ = responses.send(response_err(Some(id), error)).await;
            return Ok(());
        }
    };
    if let Err(error) = apply_confirm_gate(op, route.destructive, route.planner, &mut raw) {
        let _ = responses.send(response_err(Some(id), error)).await;
        return Ok(());
    }
    match (route.planner)(op, raw) {
        Err(error) => {
            let _ = responses.send(response_err(Some(id), error)).await;
            Ok(())
        }
        Ok(Plan::DryRun(data)) => {
            let _ = responses.send(response_ok(id, data)).await;
            Ok(())
        }
        Ok(Plan::Execute(raw)) => {
            let shares = match pool.shares_for(&account) {
                Ok(shares) => shares,
                Err(error) => {
                    let _ = responses.send(response_err(Some(id), error)).await;
                    return Ok(());
                }
            };
            let job = Job {
                id,
                op: route.op,
                timeout: route.timeout,
                future: (route.runner)(shares, raw),
                guard: JobCompletionGuard {
                    id,
                    op: route.op,
                    responses: responses.clone(),
                    completed: false,
                },
            };
            let send_res = match route.lane {
                Lane::Mutate => mutations.send(job).await,
                Lane::Read => {
                    let idx = read_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        % reads.len().max(1);
                    if reads.is_empty() {
                        Err(tokio::sync::mpsc::error::SendError(job))
                    } else {
                        reads[idx].send(job).await
                    }
                }
            };
            if send_res.is_err() {
                output::log_line("warn", "serve: op queue closed before dispatch");
            }
            Ok(())
        }
    }
}
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

async fn drain_and_flush(
    workers: &mut Vec<tokio::task::JoinHandle<()>>,
    response_rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
) -> TeleResult<()> {
    let deadline = tokio::time::sleep(DRAIN_BUDGET);
    tokio::pin!(deadline);
    for mut worker in workers.drain(..) {
        tokio::select! {
            _ = &mut worker => {}
            _ = &mut deadline => worker.abort(),
        }
    }
    while let Some(value) = response_rx.recv().await {
        match emit(&value).await {
            Ok(()) => {}
            Err(e) if e.is_broken_pipe() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
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
    validate_serve_selection(&names)?;
    if flags.dry_run {
        output::log_line("info", "[dry-run] would serve duplex JSONL");
        if flags.json || flags.jsonl {
            for name in &names {
                output::print_json_result(&event_row(
                    "Serve",
                    name,
                    None,
                    None,
                    Some(serde_json::json!({
                        "dry_run": true,
                        "would": format!(
                            "stream {} updates from account {name}",
                            args.events.join(",")
                        ),
                        "protocol": SERVE_PROTOCOL,
                    })),
                ))?;
            }
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

    let mut pool = ServePool {
        shares: std::sync::RwLock::new(HashMap::new()),
        resync: HashMap::new(),
    };
    let mut resync_rxs: HashMap<String, tokio::sync::mpsc::Receiver<()>> = HashMap::new();
    for name in &names {
        let (resync_tx, resync_rx) = tokio::sync::mpsc::channel::<()>(1);
        pool.resync.insert(name.clone(), resync_tx);
        resync_rxs.insert(name.clone(), resync_rx);
    }
    let pool = std::sync::Arc::new(pool);

    let (tick_tx, mut tick_rx) = tokio::sync::mpsc::channel::<AccountTick>(ACCOUNT_TICK_CAPACITY);
    for name in &names {
        tokio::spawn(account_task(
            name.clone(),
            args.events.clone(),
            args.catch_up,
            creds.clone(),
            config_path.clone(),
            std::sync::Arc::clone(&pool),
            resync_rxs.remove(name).expect("resync receiver"),
            tick_tx.clone(),
        ));
    }
    drop(tick_tx);

    let (mutations, mutation_rx) = tokio::sync::mpsc::channel::<Job>(MUTATE_QUEUE_CAPACITY);
    let mut read_senders = Vec::with_capacity(READ_POOL_SIZE);
    let mut read_receivers = Vec::with_capacity(READ_POOL_SIZE);
    for _ in 0..READ_POOL_SIZE {
        let (tx, rx) = tokio::sync::mpsc::channel::<Job>(READ_QUEUE_CAPACITY);
        read_senders.push(tx);
        read_receivers.push(rx);
    }
    let (response_tx, mut response_rx) =
        tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
    let mut workers = Vec::new();
    workers.push(tokio::spawn(job_worker(mutation_rx, response_tx.clone())));
    for rx in read_receivers {
        workers.push(tokio::spawn(job_worker(rx, response_tx.clone())));
    }
    let (dispatch_tx, mut dispatch_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u64, String, serde_json::Value)>();
    let disp_response_tx = response_tx.clone();
    let disp_pool = std::sync::Arc::clone(&pool);
    let disp_mutations = mutations.clone();
    let disp_read_senders = read_senders.clone();
    let disp_read_counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disp_read_counter_clone = std::sync::Arc::clone(&disp_read_counter);
    let dispatcher = tokio::spawn(async move {
        while let Some((id, op, params)) = dispatch_rx.recv().await {
            let served = disp_pool.served();
            let _ = dispatch_action(
                &disp_pool,
                &served,
                &disp_mutations,
                &disp_read_senders,
                &disp_read_counter_clone,
                &disp_response_tx,
                id,
                &op,
                params,
            )
            .await;
        }
    });
    let main_response_tx = response_tx.clone();
    drop(response_tx);

    let mut identities: HashMap<String, Option<serde_json::Value>> = HashMap::new();
    let mut greeted = false;
    let mut seq: u64 = 0;
    let hello_entries = |identities: &HashMap<String, Option<serde_json::Value>>| {
        let mut entries: Vec<(String, Option<serde_json::Value>)> = pool
            .served()
            .into_iter()
            .map(|name| (name.clone(), identities.get(&name).cloned().flatten()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    };
    loop {
        enum Tick {
            Line(String),
            Eof,
            Account(AccountTick),
            Response(serde_json::Value),
        }
        let tick = tokio::select! {
            line = rx.recv() => match line {
                Some(l) => Tick::Line(l),
                None => Tick::Eof,
            },
            tick = tick_rx.recv() => match tick {
                Some(t) => Tick::Account(t),
                None => continue,
            },
            response = response_rx.recv() => match response {
                Some(value) => Tick::Response(value),
                None => continue,
            },
        };
        match tick {
            Tick::Eof => {
                output::log_line("info", "serve: stdin closed, shutting down");
                drop(dispatch_tx);
                drop(mutations);
                drop(read_senders);
                drop(main_response_tx);
                let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                drain_and_flush(&mut workers, &mut response_rx).await?;
                return Ok(EXIT_OK);
            }
            Tick::Line(line) => match parse_incoming(&line) {
                Err(error) => {
                    let _ = main_response_tx.try_send(response_err(None, error));
                }
                Ok(ServeIn::Hello { protocol }) => {
                    let _ = protocol;
                    let _ = main_response_tx.try_send(hello_out_accounts(
                        &hello_entries(&identities),
                        last_seq_of(&seq),
                    ));
                }
                Ok(ServeIn::Action { id, op, params }) => {
                    if dispatch_tx.send((id, op, params)).is_err() {
                        output::log_line("warn", "serve: dispatch queue closed");
                    }
                }
            },
            Tick::Response(value) => match emit(&value).await {
                Ok(()) => {}
                Err(e) if e.is_broken_pipe() => {
                    drop(dispatch_tx);
                    drop(mutations);
                    drop(read_senders);
                    drop(main_response_tx);
                    let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                    drain_and_flush(&mut workers, &mut response_rx).await?;
                    return Ok(EXIT_OK);
                }
                Err(e) => {
                    drop(dispatch_tx);
                    drop(mutations);
                    drop(read_senders);
                    drop(main_response_tx);
                    let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                    drain_and_flush(&mut workers, &mut response_rx).await?;
                    return Err(e);
                }
            },
            Tick::Account(AccountTick::Fatal { err }) => {
                dispatcher.abort();
                for w in &mut workers {
                    w.abort();
                }
                drop(dispatch_tx);
                drop(mutations);
                drop(read_senders);
                drop(main_response_tx);
                return Err(err);
            }
            Tick::Account(AccountTick::Ready { name, identity }) => {
                identities.insert(name, identity);
                if greeted {
                    continue;
                }
                if identities.len() < pool.served().len() {
                    continue;
                }
                match emit(&hello_out_accounts(
                    &hello_entries(&identities),
                    last_seq_of(&seq),
                ))
                .await
                {
                    Ok(()) => greeted = true,
                    Err(e) if e.is_broken_pipe() => {
                        drop(dispatch_tx);
                        drop(mutations);
                        drop(read_senders);
                        drop(main_response_tx);
                        let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                        drain_and_flush(&mut workers, &mut response_rx).await?;
                        return Ok(EXIT_OK);
                    }
                    Err(e) => return Err(e),
                }
            }
            Tick::Account(AccountTick::Reconnected { name }) => {
                if !greeted {
                    continue;
                }
                output::log_line("info", &format!("serve: account {name} reconnected"));
                let mut row = event_row("Reconnected", &name, None, None, None);
                apply_seq(&mut row, &mut seq);
                match emit(&row).await {
                    Ok(()) => {}
                    Err(e) if e.is_broken_pipe() => {
                        drop(dispatch_tx);
                        drop(mutations);
                        drop(read_senders);
                        drop(main_response_tx);
                        let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                        drain_and_flush(&mut workers, &mut response_rx).await?;
                        return Ok(EXIT_OK);
                    }
                    Err(e) => return Err(e),
                }
            }
            Tick::Account(AccountTick::Event { mut ev }) => {
                apply_seq(&mut ev, &mut seq);
                if let Err(e) = emit(&ev).await {
                    if e.is_broken_pipe() {
                        drop(dispatch_tx);
                        drop(mutations);
                        drop(read_senders);
                        drop(main_response_tx);
                        let _ = tokio::time::timeout(DRAIN_BUDGET, dispatcher).await;
                        drain_and_flush(&mut workers, &mut response_rx).await?;
                        return Ok(EXIT_OK);
                    }
                    output::log_line("warn", &format!("serve: emit failed: {}", e.message()));
                    continue;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn account_task(
    name: String,
    events: Vec<String>,
    catch_up: bool,
    creds: crate::config::Credentials,
    config_path: Option<std::path::PathBuf>,
    pool: std::sync::Arc<ServePool>,
    resync_rx: tokio::sync::mpsc::Receiver<()>,
    ticks: tokio::sync::mpsc::Sender<AccountTick>,
) {
    let initial_catch_up = catch_up;
    let mut catch_up = catch_up;
    let mut resync_rx = resync_rx;
    let mut failures: u32 = 0;
    let mut greeted = false;
    let mut dedupe = ServeDedupe::new();
    loop {
        let mut guard = loop {
            match crate::client::ClientGuard::connect(&name, creds.api_id, config_path.as_deref())
                .await
            {
                Ok(guard) => break guard,
                Err(e) => {
                    if let Err(fatal) =
                        handle_stream_failure(&name, TeleError::from(e), &mut failures, None).await
                    {
                        let _ = ticks.send(AccountTick::Fatal { err: fatal }).await;
                        return;
                    }
                }
            }
        };
        if let Err(e) = crate::client::authorize(&guard.client).await {
            guard.close().await;
            let _ = ticks.send(AccountTick::Fatal { err: e }).await;
            return;
        }
        let me = match guard.client.get_me().await {
            Ok(me) => me,
            Err(e) => {
                guard.close().await;
                let _ = ticks
                    .send(AccountTick::Fatal {
                        err: TeleError::Other(format!("serve: get_me failed: {e}")),
                    })
                    .await;
                return;
            }
        };
        let identity = identity_json(&me);
        if let Err(e) = guard
            .client
            .invoke(&tl::functions::updates::GetState {})
            .await
        {
            let err = getstate_probe_error(e);
            guard.close().await;
            if matches!(err, TeleError::Auth(_)) {
                let _ = ticks.send(AccountTick::Fatal { err }).await;
                return;
            }
            if let Err(fatal) = handle_stream_failure(&name, err, &mut failures, None).await {
                let _ = ticks.send(AccountTick::Fatal { err: fatal }).await;
                return;
            }
            continue;
        }
        let receiver =
            std::mem::replace(&mut guard.updates, tokio::sync::mpsc::unbounded_channel().1);
        let mut stream = match guard
            .client
            .stream_updates(
                receiver,
                grammers_client::client::UpdatesConfiguration {
                    catch_up,
                    update_queue_limit: Some(1000),
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                guard.close().await;
                let _ = ticks
                    .send(AccountTick::Fatal {
                        err: TeleError::Other(e.to_string()),
                    })
                    .await;
                return;
            }
        };
        catch_up = initial_catch_up;
        let shares = guard.shares();
        pool.publish(&name, shares);
        if greeted {
            output::log_line("info", &format!("serve: account {name} reconnected"));
            let _ = ticks
                .send(AccountTick::Reconnected { name: name.clone() })
                .await;
        } else {
            let _ = ticks
                .send(AccountTick::Ready {
                    name: name.clone(),
                    identity: Some(identity),
                })
                .await;
        }
        greeted = true;
        failures = 0;
        loop {
            enum Tick {
                Update(Box<Update>),
                StreamError(grammers_client::InvocationError),
                Resync,
            }
            let tick = tokio::select! {
                res = tokio::time::timeout(
                    poll_timeout(None, std::time::Instant::now()),
                    stream.next(),
                ) => match res {
                    Ok(Ok(u)) => Tick::Update(Box::new(u)),
                    Ok(Err(e)) => Tick::StreamError(e),
                    Err(_) => continue,
                },
                _ = resync_rx.recv() => Tick::Resync,
            };
            match tick {
                Tick::Resync => break,
                Tick::StreamError(e) => {
                    if let Err(fatal) = handle_stream_failure(
                        &name,
                        crate::error::invocation_error(e),
                        &mut failures,
                        None,
                    )
                    .await
                    {
                        let _ = ticks.send(AccountTick::Fatal { err: fatal }).await;
                        return;
                    }
                    break;
                }
                Tick::Update(update) => {
                    if let Err(e) = stream.sync_update_state().await {
                        output::log_line("warn", &format!("serve: sync_update_state failed: {e}"));
                    }
                    let Some(kind) = event_kind_allowed(&update, &events) else {
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
                    let key = dedupe_key(&name, chat_id, message.id(), pts);
                    if dedupe.check(key) {
                        continue;
                    }
                    let row = match crate::serialize::message_to_json(message) {
                        Ok(mut r) => {
                            crate::serialize::enrich_message_row(&mut r, message);
                            crate::serialize::ensure_outer_peer_sender(&mut r, peer, None);
                            r
                        }
                        Err(e) => {
                            output::log_line(
                                "warn",
                                &format!("serve: message_to_json failed: {}", e.message()),
                            );
                            continue;
                        }
                    };
                    let ev = event_row(kind, &name, chat_id, None, Some(row));
                    if ticks.send(AccountTick::Event { ev }).await.is_err() {
                        return;
                    }
                }
            }
        }
        guard.close().await;
    }
}
#[cfg(test)]
#[allow(clippy::await_holding_lock)]
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
        let hello = hello_out_accounts(
            &[(
                "work".to_string(),
                Some(serde_json::json!({"user_id": 7, "username": "w"})),
            )],
            None,
        );
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["protocol"], SERVE_PROTOCOL);
        assert_eq!(hello["min_protocol"], SERVE_PROTOCOL_MIN);
        assert_eq!(hello["max_protocol"], SERVE_PROTOCOL);
        assert_eq!(hello["account"], "work");
        assert_eq!(hello["identity"]["user_id"], 7);

        let bare = hello_out_accounts(&[("work".to_string(), None)], None);
        assert!(bare.get("identity").is_none());
    }

    #[test]
    fn hello_out_echoes_last_seq_null_then_value() {
        let first = hello_out_accounts(&[("work".to_string(), None)], last_seq_of(&0));
        assert_eq!(first["last_seq"], serde_json::Value::Null);
        let later = hello_out_accounts(&[("work".to_string(), None)], last_seq_of(&7));
        assert_eq!(later["last_seq"], 7);
    }

    #[test]
    fn apply_seq_stamps_monotonic_per_connection_counter() {
        let mut counter = 0u64;
        let mut row = event_row("NewMessage", "work", Some(123), None, None);
        assert_eq!(apply_seq(&mut row, &mut counter), 1);
        assert_eq!(row["seq"], 1);
        let mut edited = event_row("MessageEdited", "work", Some(123), None, None);
        assert_eq!(apply_seq(&mut edited, &mut counter), 2);
        assert_eq!(edited["seq"], 2);
        let mut reconn = event_row("Reconnected", "work", None, None, None);
        assert_eq!(apply_seq(&mut reconn, &mut counter), 3);
        assert_eq!(reconn["seq"], 3);
        assert_eq!(counter, 3);
        let mut fresh_counter = 0u64;
        let mut other = serde_json::json!({});
        assert_eq!(apply_seq(&mut other, &mut fresh_counter), 1);
        assert_eq!(other["seq"], 1);
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

    #[test]
    fn stream_resync_parses_with_empty_or_missing_params() {
        for line in [
            r#"{"id":3,"op":"stream.resync"}"#,
            r#"{"id":3,"op":"stream.resync","params":{}}"#,
        ] {
            match parse_incoming(line).unwrap() {
                ServeIn::Action { id, op, params } => {
                    assert_eq!((id, op.as_str()), (3, "stream.resync"));
                    assert_eq!(params, serde_json::json!({}));
                }
                other => panic!("expected action for {line}, got {other:?}"),
            }
        }
    }

    #[test]
    fn resync_rejects_non_empty_params_with_serve_error() {
        assert!(validate_no_params("stream.resync", &serde_json::json!({})).is_ok());
        let err =
            validate_no_params("stream.resync", &serde_json::json!({"force": true})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err["message"].as_str().unwrap().contains("no parameters"));
    }

    #[test]
    fn resync_account_param_survives_target_selection_then_validation() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"account": "beta"});
        let selected = resync_targets(&served, &mut params).unwrap();
        assert_eq!(selected, vec!["beta".to_string()]);
        assert!(
            validate_no_params("stream.resync", &params).is_ok(),
            "account must be stripped before validate_no_params"
        );

        let mut ambiguous = serde_json::json!({});
        let targets = resync_targets(&served, &mut ambiguous).unwrap();
        assert_eq!(targets.len(), 2);
        assert!(validate_no_params("stream.resync", &ambiguous).is_ok());
    }

    #[test]
    fn ops_list_rejects_non_empty_params_with_serve_error() {
        assert!(validate_no_params("ops.list", &serde_json::json!({})).is_ok());
        let err = validate_no_params("ops.list", &serde_json::json!({"lane": "read"})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("ops.list"), "{msg}");
        assert!(msg.contains("no parameters"), "{msg}");
    }

    fn plan_for(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
        let route = find_route(op).unwrap_or_else(|| panic!("route missing for {op}"));
        let planner = route.planner;
        planner(op, params)
    }

    #[test]
    fn routes_table_is_locked_command_path_form() {
        let expected = [
            "account sessions list",
            "account sessions web",
            "account status",
            "account ttl get",
            "account ttl set",
            "chat admin",
            "chat admin-log",
            "chat create",
            "chat edit",
            "chat invite",
            "chat join",
            "chat kick",
            "chat leave",
            "chat link",
            "chat participants",
            "chat requests",
            "chat settings",
            "chat stats",
            "contact add",
            "contact block",
            "contact list",
            "contact remove",
            "contact unblock",
            "dialog archive",
            "dialog delete",
            "dialog draft",
            "dialog drafts",
            "dialog list",
            "dialog pin",
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
            "privacy get",
            "privacy set",
            "profile emoji-status",
            "profile get",
            "profile photo",
            "profile set",
            "raw",
            "sticker install",
            "sticker list",
            "sticker remove",
            "sticker search",
            "sticker show",
            "story delete",
            "story list",
            "story pin",
            "story read",
            "story send",
            "story unpin",
            "topic close",
            "topic create",
            "topic delete",
            "topic edit",
            "topic list",
            "topic pin",
            "topic reopen",
        ];
        let mut actual: Vec<String> = serve_op_routes().iter().map(|r| r.op.to_string()).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        let group_ok = |op: &str| {
            op.starts_with("msg ")
                || op.starts_with("dialog ")
                || op.starts_with("topic ")
                || op.starts_with("profile ")
                || op.starts_with("privacy ")
                || op.starts_with("contact ")
                || op.starts_with("sticker ")
                || op.starts_with("story ")
                || op.starts_with("chat ")
                || op.starts_with("account ")
                || op == "raw"
        };
        assert!(serve_op_routes().iter().all(|r| {
            group_ok(r.op)
                && !r.op.contains('.')
                && r.op
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == ' ' || c == '-' || c.is_ascii_digit())
        }));
    }

    #[test]
    fn unknown_op_maps_to_not_implemented_listing_known_ops() {
        assert!(find_route("msg.send").is_none());
        assert!(find_route("chat join").is_some());
        assert!(find_route("ping").is_none());
        assert!(find_route("stream.resync").is_none());
        assert!(find_route("ops.list").is_none());
        let err = not_implemented("msg.send");
        assert_eq!(err["type"], "NotImplemented");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("msg.send"), "{msg}");
        assert!(msg.contains("msg send"), "{msg}");
        assert!(msg.contains("ping"), "{msg}");
        assert!(msg.contains("stream.resync"), "{msg}");
        assert!(msg.contains("ops.list"), "{msg}");
    }

    #[test]
    fn route_table_build_is_idempotent_and_lookup_is_stable() {
        let a = build_route_table();
        let b = build_route_table();
        assert_eq!(a, b);
        let mut expected: Vec<String> =
            serve_op_routes().iter().map(|r| r.op.to_string()).collect();
        expected.sort_unstable();
        let joined = expected.join(", ");
        assert_eq!(a.names(), joined);
        assert_eq!(b.names(), joined);
        for op in &expected {
            let ra = a.find(op);
            let rb = b.find(op);
            assert!(ra.is_some(), "{op} missing from table");
            assert_eq!(ra, rb, "lookup drift for {op}");
            let again = a.find(op).unwrap();
            assert_eq!(ra.unwrap(), again, "repeated lookup drift for {op}");
        }
        assert!(std::ptr::eq(route_table(), route_table()));
    }

    #[test]
    fn undelivered_envelope_is_id_correlated_stream_down_response() {
        let env = undelivered_envelope(41, "msg send");
        assert_eq!(env["type"], "response");
        assert_eq!(env["id"], 41);
        assert_eq!(env["ok"], false);
        assert_eq!(env["error"]["type"], "StreamDown");
        let message = env["error"]["message"].as_str().unwrap();
        assert!(message.contains("msg send"), "{message}");
    }

    #[tokio::test]
    async fn dropped_queued_job_emits_stream_down_envelope_without_running_future() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let job = Job {
            id: 12,
            op: "msg get",
            timeout: None,
            future: Box::pin(async {
                std::future::pending::<Result<serde_json::Value, serde_json::Value>>().await
            }),
            guard: JobCompletionGuard {
                id: 12,
                op: "msg get",
                responses: tx.clone(),
                completed: false,
            },
        };
        drop(tx);
        drop(job);
        let value = rx.recv().await.expect("guard must emit on drop");
        assert_eq!(value["id"], 12);
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["type"], "StreamDown");
        assert!(rx.try_recv().is_err(), "exactly one failure envelope");
    }

    #[tokio::test]
    async fn completed_job_disarms_guard_so_no_duplicate_envelope() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let job = Job {
            id: 15,
            op: "msg get",
            timeout: None,
            future: Box::pin(async { Ok(serde_json::json!({ "done": true })) }),
            guard: JobCompletionGuard {
                id: 15,
                op: "msg get",
                responses: tx.clone(),
                completed: false,
            },
        };
        drop(tx);
        let value = execute_job(job).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["done"], true);
        assert!(rx.try_recv().is_err(), "disarmed guard must stay silent");
    }

    #[tokio::test]
    async fn aborted_job_worker_emits_single_failure_envelope_for_in_flight_job() {
        let (mutation_tx, mutation_rx) = tokio::sync::mpsc::channel::<Job>(4);
        let (response_tx, mut response_rx) =
            tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let worker_tx = response_tx.clone();
        let guard_tx = response_tx.clone();
        drop(response_tx);
        let worker = tokio::spawn(job_worker(mutation_rx, worker_tx));
        let job = Job {
            id: 21,
            op: "story send",
            timeout: None,
            future: Box::pin(async {
                std::future::pending::<Result<serde_json::Value, serde_json::Value>>().await
            }),
            guard: JobCompletionGuard {
                id: 21,
                op: "story send",
                responses: guard_tx,
                completed: false,
            },
        };
        mutation_tx.send(job).await.unwrap();
        drop(mutation_tx);
        tokio::time::sleep(Duration::from_millis(50)).await;
        worker.abort();
        let _ = worker.await;
        let value = tokio::time::timeout(Duration::from_secs(2), response_rx.recv())
            .await
            .expect("aborted job must emit")
            .expect("envelope present");
        assert_eq!(value["id"], 21);
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["type"], "StreamDown");
        assert!(response_rx.try_recv().is_err());
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
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("chat"), "{msg}");
        assert!(msg.contains("missing field"), "{msg}");

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
    fn field_from_serde_message_extracts_backtick_patterns_only() {
        assert_eq!(
            field_from_serde_message("unknown field `mesage`, expected one of `chat`, `text`"),
            Some("mesage".to_string())
        );
        assert_eq!(
            field_from_serde_message("missing field `id`"),
            Some("id".to_string())
        );
        assert_eq!(
            field_from_serde_message("duplicate field `chat`"),
            Some("chat".to_string())
        );
        assert_eq!(
            field_from_serde_message(r#"invalid type: string "abc", expected i32"#),
            None
        );
        assert_eq!(field_from_serde_message("boom"), None);
    }

    #[test]
    fn param_key_extracted_for_unknown_missing_and_type_errors() {
        let err = plan_for(
            "msg edit",
            serde_json::json!({"chat": "@x", "id": 1, "text": "t", "extra": true}),
        )
        .unwrap_err();
        assert_eq!(err["param"], "extra");

        let err = plan_for("msg edit", serde_json::json!({"chat": "@x"})).unwrap_err();
        assert_eq!(err["param"], "id");

        let err = plan_for(
            "msg edit",
            serde_json::json!({"chat": "@x", "id": "abc", "text": "t"}),
        )
        .unwrap_err();
        assert_eq!(err["param"], "id");

        let err = plan_for(
            "msg get",
            serde_json::json!({"chat": "@x", "limit": "many"}),
        )
        .unwrap_err();
        assert_eq!(err["param"], "limit");
    }

    #[test]
    fn param_key_absent_when_no_offending_field_is_parsable() {
        let err = plan_for(
            "msg edit",
            serde_json::json!({"chat": "@x", "id": "a", "text": []}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err.get("param").is_none(), "{}", err);

        let ok_shape = prep::<EditLikeProbe>("op", serde_json::json!([1, 2])).unwrap_err();
        assert!(ok_shape.get("param").is_none());
    }

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct EditLikeProbe {
        chat: String,
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
        assert!(!d.check(dedupe_key("work", Some(123), 5, 10)));
        assert!(d.check(dedupe_key("work", Some(123), 5, 10)));
        assert!(!d.check(dedupe_key("work", Some(123), 5, 11)));
        assert!(d.check(dedupe_key("work", Some(123), 5, 11)));
        assert!(!d.check(dedupe_key("work", Some(456), 5, 10)));
        assert!(d.check(dedupe_key("work", Some(456), 5, 10)));
        assert!(!d.check(dedupe_key("work", None, 9, 0)));
        assert!(d.check(dedupe_key("work", None, 9, 0)));
    }

    #[test]
    fn serve_dedupe_key_distinguishes_accounts() {
        let a = dedupe_key("alpha", Some(777), 5, 10);
        let b = dedupe_key("beta", Some(777), 5, 10);
        assert_ne!(a, b, "same chat/msg/pts on two accounts must not collide");
    }

    #[test]
    fn select_op_account_defaults_to_first_when_absent() {
        let served = vec!["alpha".to_string()];
        let mut params = serde_json::json!({"chat": "@x"});
        let picked = select_op_account(&served, &mut params).unwrap();
        assert_eq!(picked, "alpha");
        assert!(
            params.get("account").is_none(),
            "params must stay untouched"
        );
    }

    #[test]
    fn select_op_account_requires_account_when_ambiguous() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"chat": "@x"});
        let err = select_op_account(&served, &mut params).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("alpha") && msg.contains("beta"), "msg: {msg}");
        assert!(
            params.get("account").is_none(),
            "params must stay untouched"
        );
    }

    #[test]
    fn stream_resync_selects_all_served_accounts_when_absent() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({});
        let selected = resync_targets(&served, &mut params).unwrap();
        assert_eq!(selected, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(params.get("account").is_none());
    }

    #[test]
    fn stream_resync_single_account_target_strips_param() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"account": "beta"});
        let selected = resync_targets(&served, &mut params).unwrap();
        assert_eq!(selected, vec!["beta".to_string()]);
        assert!(params.get("account").is_none());
    }

    #[test]
    fn stream_resync_rejects_unknown_account() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"account": "ghost"});
        let err = resync_targets(&served, &mut params).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("ghost"), "msg: {msg}");
    }

    #[test]
    fn select_op_account_routes_requested_and_strips_param() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"account": "beta", "chat": "@x"});
        let picked = select_op_account(&served, &mut params).unwrap();
        assert_eq!(picked, "beta");
        assert!(
            params.get("account").is_none(),
            "account param must be stripped before op parsing"
        );
        assert_eq!(params["chat"], "@x");
    }

    #[test]
    fn select_op_account_rejects_unknown_account_with_served_list() {
        let served = vec!["alpha".to_string(), "beta".to_string()];
        let mut params = serde_json::json!({"account": "ghost"});
        let err = select_op_account(&served, &mut params).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("ghost"), "msg: {msg}");
        assert!(msg.contains("alpha") && msg.contains("beta"), "msg: {msg}");
    }

    #[test]
    fn select_op_account_rejects_non_string_account() {
        let served = vec!["alpha".to_string()];
        let mut params = serde_json::json!({"account": 7});
        let err = select_op_account(&served, &mut params).unwrap_err();
        assert_eq!(err["type"], "ServeError");
    }

    #[test]
    fn select_op_account_blank_account_is_an_error() {
        let served = vec!["alpha".to_string()];
        let mut params = serde_json::json!({"account": "   "});
        assert!(select_op_account(&served, &mut params).is_err());
    }

    #[test]
    fn hello_out_accounts_keeps_legacy_fields_and_adds_accounts() {
        let id_a = serde_json::json!({"user_id": 1});
        let id_b = serde_json::json!({"user_id": 2});
        let hello = hello_out_accounts(
            &[
                ("alpha".to_string(), Some(id_a.clone())),
                ("beta".to_string(), Some(id_b.clone())),
            ],
            Some(3),
        );
        assert_eq!(hello["type"], "hello");
        assert_eq!(
            hello["account"], "alpha",
            "legacy field stays first account"
        );
        assert_eq!(hello["identity"], id_a);
        assert_eq!(hello["last_seq"], 3);
        let listed = hello["accounts"].as_array().expect("accounts array");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "alpha");
        assert_eq!(listed[0]["identity"], id_a);
        assert_eq!(listed[1]["name"], "beta");
        assert_eq!(listed[1]["identity"], id_b);
    }

    #[test]
    fn hello_out_accounts_single_account_omits_identity_when_none() {
        let hello = hello_out_accounts(&[("work".to_string(), None)], None);
        assert_eq!(hello["account"], "work");
        assert!(hello.get("identity").is_none());
        assert_eq!(hello["accounts"][0]["name"], "work");
        assert!(hello["accounts"][0].get("identity").is_none());
    }

    #[test]
    fn serve_selection_accepts_multi_and_caps_at_32() {
        let two: Vec<String> = ["a".to_string(), "b".to_string()].into();
        assert!(validate_serve_selection(&two).is_ok());
        let many: Vec<String> = (0..33).map(|i| format!("a{i}")).collect();
        let err = validate_serve_selection(&many).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "err: {err}");
        assert!(validate_serve_selection(&[]).is_err());
    }

    #[test]
    fn serve_dedupe_evicts_oldest_beyond_cap() {
        let mut d = ServeDedupe::new();
        for i in 0..(SERVE_DEDUPE_CAP as i32) {
            assert!(!d.check(dedupe_key("work", Some(1), i, i)));
        }
        assert_eq!(d.seen.len(), SERVE_DEDUPE_CAP);
        assert!(d.check(dedupe_key("work", Some(1), 0, 0)));
        assert!(!d.check(dedupe_key(
            "work",
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
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            ("contact add", Lane::Mutate, Some(30)),
            ("contact block", Lane::Mutate, Some(30)),
            ("contact list", Lane::Read, Some(120)),
            ("contact remove", Lane::Mutate, Some(30)),
            ("contact unblock", Lane::Mutate, Some(30)),
            ("dialog archive", Lane::Mutate, Some(30)),
            ("dialog delete", Lane::Mutate, Some(30)),
            ("dialog draft", Lane::Mutate, Some(30)),
            ("dialog drafts", Lane::Read, Some(120)),
            ("dialog list", Lane::Read, Some(120)),
            ("dialog pin", Lane::Mutate, Some(30)),
            ("msg click", Lane::Mutate, Some(30)),
            ("msg delete", Lane::Mutate, Some(30)),
            ("msg download", Lane::Read, None),
            ("msg edit", Lane::Mutate, Some(30)),
            ("msg forward", Lane::Mutate, Some(30)),
            ("msg get", Lane::Read, Some(120)),
            ("msg pin", Lane::Mutate, Some(30)),
            ("msg react", Lane::Mutate, Some(30)),
            ("msg read", Lane::Mutate, Some(30)),
            ("msg search", Lane::Read, Some(120)),
            ("msg send", Lane::Mutate, Some(30)),
            ("msg typing", Lane::Mutate, Some(30)),
            ("msg vote", Lane::Mutate, Some(30)),
            ("privacy get", Lane::Read, Some(120)),
            ("privacy set", Lane::Mutate, Some(30)),
            ("profile emoji-status", Lane::Mutate, Some(30)),
            ("profile get", Lane::Read, Some(120)),
            ("profile photo", Lane::Mutate, Some(30)),
            ("profile set", Lane::Mutate, Some(30)),
            ("raw", Lane::Mutate, Some(120)),
            ("sticker install", Lane::Mutate, Some(30)),
            ("sticker list", Lane::Read, Some(120)),
            ("sticker remove", Lane::Mutate, Some(30)),
            ("sticker search", Lane::Read, Some(120)),
            ("sticker show", Lane::Read, Some(120)),
            ("story delete", Lane::Mutate, Some(30)),
            ("story list", Lane::Read, Some(120)),
            ("story pin", Lane::Mutate, Some(30)),
            ("story read", Lane::Mutate, Some(30)),
            ("story send", Lane::Mutate, Some(600)),
            ("story unpin", Lane::Mutate, Some(30)),
            ("topic close", Lane::Mutate, Some(30)),
            ("topic create", Lane::Mutate, Some(30)),
            ("topic delete", Lane::Mutate, Some(30)),
            ("topic edit", Lane::Mutate, Some(30)),
            ("topic list", Lane::Read, Some(120)),
            ("topic pin", Lane::Mutate, Some(30)),
            ("topic reopen", Lane::Mutate, Some(30)),
            ("account sessions list", Lane::Read, Some(120)),
            ("account sessions web", Lane::Read, Some(120)),
            ("account status", Lane::Read, Some(120)),
            ("account ttl get", Lane::Read, Some(120)),
            ("account ttl set", Lane::Mutate, Some(30)),
            ("chat admin", Lane::Mutate, Some(30)),
            ("chat admin-log", Lane::Read, Some(120)),
            ("chat create", Lane::Mutate, Some(30)),
            ("chat edit", Lane::Mutate, Some(30)),
            ("chat invite", Lane::Mutate, Some(30)),
            ("chat join", Lane::Mutate, Some(30)),
            ("chat kick", Lane::Mutate, Some(30)),
            ("chat leave", Lane::Mutate, Some(30)),
            ("chat link", Lane::Mutate, Some(30)),
            ("chat participants", Lane::Read, Some(120)),
            ("chat requests", Lane::Mutate, Some(30)),
            ("chat settings", Lane::Mutate, Some(30)),
            ("chat stats", Lane::Read, Some(120)),
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

    #[test]
    fn route_hints_table_is_locked() {
        let expected: &[(&str, bool, bool, bool)] = &[
            ("contact add", false, false, true),
            ("contact block", false, false, true),
            ("contact list", true, false, true),
            ("contact remove", false, true, true),
            ("contact unblock", false, false, true),
            ("dialog archive", false, false, true),
            ("dialog delete", false, true, true),
            ("dialog draft", false, false, true),
            ("dialog drafts", true, false, true),
            ("dialog list", true, false, true),
            ("dialog pin", false, false, true),
            ("msg click", false, false, false),
            ("msg delete", false, true, true),
            ("msg download", true, false, false),
            ("msg edit", false, false, true),
            ("msg forward", false, false, false),
            ("msg get", true, false, true),
            ("msg pin", false, false, true),
            ("msg read", false, false, true),
            ("msg react", false, false, true),
            ("msg search", true, false, true),
            ("msg send", false, false, false),
            ("msg typing", false, false, true),
            ("msg vote", false, false, false),
            ("privacy get", true, false, true),
            ("privacy set", false, false, true),
            ("profile emoji-status", false, false, true),
            ("profile get", true, false, true),
            ("profile photo", false, false, true),
            ("profile set", false, false, true),
            ("raw", false, false, false),
            ("sticker install", false, false, true),
            ("sticker list", true, false, true),
            ("sticker remove", false, true, true),
            ("sticker search", true, false, true),
            ("sticker show", true, false, true),
            ("story delete", false, true, true),
            ("story list", true, false, true),
            ("story pin", false, false, true),
            ("story read", false, false, true),
            ("story send", false, false, true),
            ("story unpin", false, false, true),
            ("topic close", false, false, true),
            ("topic create", false, false, true),
            ("topic delete", false, true, true),
            ("topic edit", false, false, true),
            ("topic list", true, false, true),
            ("topic pin", false, false, true),
            ("topic reopen", false, false, true),
            ("account sessions list", true, false, true),
            ("account sessions web", true, false, true),
            ("account status", true, false, true),
            ("account ttl get", true, false, true),
            ("account ttl set", false, false, true),
            ("chat admin", false, false, true),
            ("chat admin-log", true, false, true),
            ("chat create", false, false, true),
            ("chat edit", false, false, true),
            ("chat invite", false, false, true),
            ("chat join", false, false, true),
            ("chat kick", false, true, true),
            ("chat leave", false, true, true),
            ("chat link", false, false, true),
            ("chat participants", true, false, true),
            ("chat requests", false, false, true),
            ("chat settings", false, false, true),
            ("chat stats", true, false, true),
        ];
        let mut destructive: Vec<&str> = Vec::new();
        let mut retry_unsafe: Vec<&str> = Vec::new();
        for (op, read_only, is_destructive, retry_safe) in expected {
            let route = route_for(op);
            assert_eq!(route.read_only, *read_only, "read_only for {op}");
            assert_eq!(route.destructive, *is_destructive, "destructive for {op}");
            assert_eq!(route.retry_safe, *retry_safe, "retry_safe for {op}");
            if *is_destructive {
                destructive.push(op);
            }
            if !*retry_safe {
                retry_unsafe.push(op);
            }
        }
        assert_eq!(
            destructive,
            vec![
                "contact remove",
                "dialog delete",
                "msg delete",
                "sticker remove",
                "story delete",
                "topic delete",
                "chat kick",
                "chat leave"
            ]
        );
        assert_eq!(
            retry_unsafe,
            vec![
                "msg click",
                "msg download",
                "msg forward",
                "msg send",
                "msg vote",
                "raw"
            ]
        );
    }

    #[test]
    fn every_route_carries_non_empty_summary() {
        for route in serve_op_routes() {
            assert!(!route.summary.is_empty(), "summary missing on {}", route.op);
            assert!(
                route.summary.chars().next().unwrap().is_ascii_lowercase(),
                "summary not lowercase sentence on {}: {}",
                route.op,
                route.summary
            );
        }
    }

    fn ops_list_rows() -> Vec<serde_json::Value> {
        ops_list_data()["ops"]
            .as_array()
            .cloned()
            .expect("ops array")
    }

    #[test]
    fn ops_list_covers_all_routes_plus_transport_inline_sorted() {
        let rows = ops_list_rows();
        let routed: Vec<String> = serve_op_routes().iter().map(|r| r.op.to_string()).collect();
        assert_eq!(rows.len(), routed.len() + INLINE_OPS.len());
        let names: Vec<String> = rows
            .iter()
            .map(|r| r["op"].as_str().unwrap().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted, "ops.list must be sorted and unique");
        for op in &routed {
            assert!(names.contains(op), "ops.list missing routed op {op}");
        }
        for (op, _, _, _, _) in INLINE_OPS {
            let name = op.to_string();
            assert!(names.contains(&name), "ops.list missing inline op {op}");
        }
    }

    #[test]
    fn ops_list_row_hints_match_routes_and_transport_group() {
        let by_op: std::collections::HashMap<String, serde_json::Value> = ops_list_rows()
            .into_iter()
            .map(|r| (r["op"].as_str().unwrap().to_string(), r))
            .collect();
        let spot = |op: &str, ro: bool, destr: bool, retry: bool, group: &str| {
            let row = &by_op[op];
            assert_eq!(row["read_only"], ro, "{op} read_only");
            assert_eq!(row["destructive"], destr, "{op} destructive");
            assert_eq!(row["retry_safe"], retry, "{op} retry_safe");
            assert_eq!(row["group"], group, "{op} group");
            assert!(!row["summary"].as_str().unwrap().is_empty(), "{op} summary");
        };
        spot("dialog delete", false, true, true, "dialog");
        spot("dialog list", true, false, true, "dialog");
        spot("msg delete", false, true, true, "msg");
        spot("msg send", false, false, false, "msg");
        spot("raw", false, false, false, "raw");
        spot("contact remove", false, true, true, "contact");
        spot("topic delete", false, true, true, "topic");
        spot("story delete", false, true, true, "story");
        spot("sticker remove", false, true, true, "sticker");
        spot("ping", true, false, true, "transport");
        spot("stream.resync", true, false, true, "transport");
        spot("ops.list", true, false, true, "transport");
    }

    #[test]
    fn destructive_without_confirm_yields_confirm_required_with_dry_would() {
        let params = serde_json::json!({"chat": "@game", "all": true});
        let mut raw = params.clone();
        let err = apply_confirm_gate(
            "msg delete",
            true,
            route_for("msg delete").planner,
            &mut raw,
        )
        .unwrap_err();
        assert_eq!(err["type"], "ConfirmRequired");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("msg delete"), "{msg}");
        assert!(msg.contains("requires confirm:true"), "{msg}");
        let dry_via_planner = match plan_for(
            "msg delete",
            serde_json::json!({"chat": "@game", "all": true, "dry_run": true}),
        )
        .unwrap()
        {
            Plan::DryRun(data) => data,
            other => panic!("expected dry run plan, got {other:?}"),
        };
        assert_eq!(err["would"], dry_via_planner);
        assert_eq!(err["would"]["would"], "delete all messages in chat @game");
    }

    #[test]
    fn destructive_with_confirm_executes_and_confirm_is_stripped() {
        let mut raw = serde_json::json!({"chat": "@game", "all": true, "confirm": true});
        apply_confirm_gate(
            "msg delete",
            true,
            route_for("msg delete").planner,
            &mut raw,
        )
        .unwrap_or_else(|e| panic!("gate should pass with confirm, got {e}"));
        assert!(raw.get("confirm").is_none(), "confirm leaked into params");
        match plan_for("msg delete", raw).unwrap() {
            Plan::Execute(_) => {}
            other => panic!("expected execute plan after confirm, got {other:?}"),
        }

        let mut raw = serde_json::json!({"chat": "@game", "all": true, "confirm": false});
        let gate = apply_confirm_gate(
            "msg delete",
            true,
            route_for("msg delete").planner,
            &mut raw,
        );
        assert!(gate.is_err(), "confirm:false must not satisfy the gate");
    }

    #[test]
    fn destructive_dry_run_still_requires_confirm_first() {
        let mut raw = serde_json::json!({"chat": "@game", "all": true, "dry_run": true});
        let err = apply_confirm_gate(
            "msg delete",
            true,
            route_for("msg delete").planner,
            &mut raw,
        )
        .unwrap_err();
        assert_eq!(err["type"], "ConfirmRequired");

        let mut raw =
            serde_json::json!({"chat": "@game", "all": true, "dry_run": true, "confirm": true});
        apply_confirm_gate(
            "msg delete",
            true,
            route_for("msg delete").planner,
            &mut raw,
        )
        .unwrap();
        assert!(raw.get("confirm").is_none());
        match plan_for("msg delete", raw).unwrap() {
            Plan::DryRun(data) => assert_eq!(data["dry_run"], true),
            other => panic!("expected dry run plan, got {other:?}"),
        }
    }

    #[test]
    fn confirm_key_never_leaks_into_params_parsing() {
        let mut raw = serde_json::json!({"chat": "@x", "text": "hi", "confirm": true});
        apply_confirm_gate("msg send", false, route_for("msg send").planner, &mut raw).unwrap();
        assert!(raw.get("confirm").is_none());
        match plan_for("msg send", raw).unwrap() {
            Plan::Execute(_) => {}
            other => panic!("expected execute plan, got {other:?}"),
        }

        let mut raw = serde_json::json!({"chat": "@x", "id": 1, "text": "t", "confirm": true});
        apply_confirm_gate("msg edit", false, route_for("msg edit").planner, &mut raw).unwrap();
        assert!(raw.get("confirm").is_none());

        let unstripped = serde_json::json!({"chat": "@x", "text": "hi", "confirm": true});
        let err = plan_for("msg send", unstripped).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        assert!(err["message"].as_str().unwrap().contains("unknown field"));
    }

    #[test]
    fn non_destructive_ops_pass_gate_unchanged_without_confirm() {
        let mut raw = serde_json::json!({"chat": "@game", "id": 5, "reaction": "+1"});
        let before = raw.clone();
        apply_confirm_gate("msg react", false, route_for("msg react").planner, &mut raw).unwrap();
        assert_eq!(raw, before);

        let mut raw = serde_json::json!({"chat": "@game"});
        let before = raw.clone();
        apply_confirm_gate(
            "dialog archive",
            false,
            route_for("dialog archive").planner,
            &mut raw,
        )
        .unwrap();
        assert_eq!(raw, before);
    }

    fn scripted_job(id: u64, delay_ms: u64, timeout: Option<Duration>) -> Job {
        let (tx, _) = tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        Job {
            id,
            op: "msg send",
            timeout,
            future: Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(serde_json::json!({ "id": id }))
            }),
            guard: JobCompletionGuard {
                id,
                op: "msg send",
                responses: tx,
                completed: false,
            },
        }
    }

    async fn next_response(
        rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
    ) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("response overdue")
            .expect("response channel closed")
    }

    #[tokio::test]
    async fn mutate_lane_preserves_submission_order_reads_run_concurrently() {
        let (mutation_tx, mutation_rx) = tokio::sync::mpsc::channel::<Job>(8);
        let (response_tx, mut response_rx) =
            tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let (read_tx_a, read_rx_a) = tokio::sync::mpsc::channel::<Job>(8);
        let (read_tx_b, read_rx_b) = tokio::sync::mpsc::channel::<Job>(8);
        let workers = vec![
            tokio::spawn(job_worker(mutation_rx, response_tx.clone())),
            tokio::spawn(job_worker(read_rx_a, response_tx.clone())),
            tokio::spawn(job_worker(read_rx_b, response_tx.clone())),
        ];
        drop(response_tx);

        mutation_tx.send(scripted_job(1, 90, None)).await.unwrap();
        mutation_tx.send(scripted_job(2, 1, None)).await.unwrap();
        mutation_tx.send(scripted_job(3, 1, None)).await.unwrap();
        read_tx_a.send(scripted_job(4, 100, None)).await.unwrap();
        read_tx_b.send(scripted_job(5, 100, None)).await.unwrap();
        drop(mutation_tx);
        drop(read_tx_a);
        drop(read_tx_b);

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
        let (tx1, _) = tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let ok_job = Job {
            id: 7,
            op: "msg download",
            timeout: None,
            future: Box::pin(async { Ok(serde_json::json!({ "bytes": 11 })) }),
            guard: JobCompletionGuard {
                id: 7,
                op: "msg download",
                responses: tx1,
                completed: false,
            },
        };
        let value = execute_job(ok_job).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["id"], 7);
        assert_eq!(value["data"]["bytes"], 11);

        let (tx2, _) = tokio::sync::mpsc::channel::<serde_json::Value>(RESPONSE_CAPACITY);
        let err_job = Job {
            id: 8,
            op: "msg download",
            timeout: None,
            future: Box::pin(async { Err(err_json("ServeError", "boom")) }),
            guard: JobCompletionGuard {
                id: 8,
                op: "msg download",
                responses: tx2,
                completed: false,
            },
        };
        let value = execute_job(err_job).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["id"], 8);
        assert_eq!(value["error"]["type"], "ServeError");
    }
}
