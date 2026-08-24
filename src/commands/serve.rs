use clap::Parser;
use grammers_client::tl;
use grammers_client::update::Update;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;

use crate::client::ClientGuard;
use crate::commands::listen::{
    event_row, getstate_probe_error, handle_stream_failure, is_empty_update, poll_timeout,
    update_peer,
};
use crate::commands::msg::{
    click_core, click_serve_dry_run, delete_core, delete_serve_dry_run, download_core,
    download_serve_dry_run, edit_core, edit_serve_dry_run, forward_core, forward_serve_dry_run,
    get_core, get_serve_dry_run, pin_core, pin_serve_dry_run, react_core, react_serve_dry_run,
    read_core, read_serve_dry_run, search_core, search_serve_dry_run, send_core,
    send_serve_dry_run, typing_core, typing_serve_dry_run, validate_click, validate_delete,
    validate_download, validate_edit, validate_forward, validate_get, validate_pin, validate_react,
    validate_read, validate_search, validate_send, validate_typing, validate_vote, vote_core,
    vote_serve_dry_run, ClickArgs, ClickParams, DeleteArgs, DeleteParams, DownloadArgs,
    DownloadParams, EditArgs, EditParams, ForwardArgs, ForwardParams, GetArgs, GetParams, PinArgs,
    PinParams, ReactArgs, ReactParams, ReadArgs, ReadParams, SearchArgs, SearchParams, SendArgs,
    SendParams, TypingArgs, TypingParams, VoteArgs, VoteParams,
};
use crate::error::{TeleError, TeleResult, EXIT_OK};
use crate::executor::GlobalFlags;
use crate::output;

pub const SERVE_PROTOCOL: u32 = 1;
pub const SERVE_PROTOCOL_MIN: u32 = 1;

const SERVE_EVENTS: &[&str] = &["NewMessage", "MessageEdited"];

const SERVE_DEDUPE_CAP: usize = 10_000;

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

type ServeRunner = for<'a> fn(
    &'a ClientGuard,
    serde_json::Value,
) -> Pin<
    Box<dyn Future<Output = Result<serde_json::Value, serde_json::Value>> + Send + 'a>,
>;

type Planner = fn(&str, serde_json::Value) -> Result<Plan, serde_json::Value>;

#[derive(Debug)]
enum Plan {
    DryRun(serde_json::Value),
    Execute(serde_json::Value),
}

struct OpRoute {
    op: &'static str,
    planner: Planner,
    runner: ServeRunner,
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

fn err_json(kind: &str, message: impl Into<String>) -> serde_json::Value {
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

macro_rules! serve_runner {
    ($name:ident, $core:path, $params:ty) => {
        fn $name<'a>(
            guard: &'a ClientGuard,
            raw: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, serde_json::Value>> + Send + 'a>>
        {
            Box::pin(async move {
                let params: $params = serde_json::from_value(raw)
                    .map_err(|e| err_json("ServeError", format!("params: {e}")))?;
                $core(guard, params).await.map_err(|e| e.as_json())
            })
        }
    };
}

serve_runner!(run_send, send_core, SendParams);
serve_runner!(run_edit, edit_core, EditParams);
serve_runner!(run_delete, delete_core, DeleteParams);
serve_runner!(run_forward, forward_core, ForwardParams);
serve_runner!(run_pin, pin_core, PinParams);
serve_runner!(run_get, get_core, GetParams);
serve_runner!(run_read, read_core, ReadParams);
serve_runner!(run_react, react_core, ReactParams);
serve_runner!(run_search, search_core, SearchParams);
serve_runner!(run_download, download_core, DownloadParams);
serve_runner!(run_vote, vote_core, VoteParams);
serve_runner!(run_typing, typing_core, TypingParams);
serve_runner!(run_click, click_core, ClickParams);

macro_rules! serve_route {
    ($op:literal, $params:ty, $args:ty, $validate:path, $dry:path, $runner:expr) => {
        OpRoute {
            op: $op,
            planner: |op: &str, raw: serde_json::Value| {
                let parsed: $params = prep(op, raw.clone())?;
                let args: $args = (&parsed).into();
                $validate(&args).map_err(|e| e.as_json())?;
                if parsed.dry_run {
                    return Ok(Plan::DryRun($dry(&args).map_err(|e| e.as_json())?));
                }
                Ok(Plan::Execute(raw))
            },
            runner: $runner,
        }
    };
}

const SERVE_OPS: &[OpRoute] = &[
    serve_route!(
        "msg click",
        ClickParams,
        ClickArgs,
        validate_click,
        click_serve_dry_run,
        run_click
    ),
    serve_route!(
        "msg delete",
        DeleteParams,
        DeleteArgs,
        validate_delete,
        delete_serve_dry_run,
        run_delete
    ),
    serve_route!(
        "msg download",
        DownloadParams,
        DownloadArgs,
        validate_download,
        download_serve_dry_run,
        run_download
    ),
    serve_route!(
        "msg edit",
        EditParams,
        EditArgs,
        validate_edit,
        edit_serve_dry_run,
        run_edit
    ),
    serve_route!(
        "msg forward",
        ForwardParams,
        ForwardArgs,
        validate_forward,
        forward_serve_dry_run,
        run_forward
    ),
    serve_route!(
        "msg get",
        GetParams,
        GetArgs,
        validate_get,
        get_serve_dry_run,
        run_get
    ),
    serve_route!(
        "msg pin",
        PinParams,
        PinArgs,
        validate_pin,
        pin_serve_dry_run,
        run_pin
    ),
    serve_route!(
        "msg read",
        ReadParams,
        ReadArgs,
        validate_read,
        read_serve_dry_run,
        run_read
    ),
    serve_route!(
        "msg react",
        ReactParams,
        ReactArgs,
        validate_react,
        react_serve_dry_run,
        run_react
    ),
    serve_route!(
        "msg search",
        SearchParams,
        SearchArgs,
        validate_search,
        search_serve_dry_run,
        run_search
    ),
    serve_route!(
        "msg send",
        SendParams,
        SendArgs,
        validate_send,
        send_serve_dry_run,
        run_send
    ),
    serve_route!(
        "msg typing",
        TypingParams,
        TypingArgs,
        validate_typing,
        typing_serve_dry_run,
        run_typing
    ),
    serve_route!(
        "msg vote",
        VoteParams,
        VoteArgs,
        validate_vote,
        vote_serve_dry_run,
        run_vote
    ),
];

fn prep<P: serde::de::DeserializeOwned>(
    op: &str,
    raw: serde_json::Value,
) -> Result<P, serde_json::Value> {
    serde_json::from_value(raw)
        .map_err(|e| err_json("ServeError", format!("op {op}: invalid params: {e}")))
}

fn known_op_names() -> String {
    let mut names: Vec<&str> = SERVE_OPS.iter().map(|r| r.op).collect();
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

fn find_route(op: &str) -> Option<&'static OpRoute> {
    SERVE_OPS.iter().find(|r| r.op == op)
}

async fn handle_action(
    guard: &ClientGuard,
    id: u64,
    op: &str,
    params: serde_json::Value,
) -> TeleResult<()> {
    if op == "ping" {
        return emit(&response_ok(id, serde_json::json!({ "pong": true })));
    }
    let response = match find_route(op) {
        None => response_err(Some(id), not_implemented(op)),
        Some(route) => match (route.planner)(op, params) {
            Err(error) => response_err(Some(id), error),
            Ok(Plan::DryRun(data)) => response_ok(id, data),
            Ok(Plan::Execute(raw)) => match (route.runner)(guard, raw).await {
                Ok(data) => response_ok(id, data),
                Err(error) => response_err(Some(id), error),
            },
        },
    };
    emit(&response)
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

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
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
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    failures: &mut u32,
    greet: bool,
    dedupe: &mut ServeDedupe,
) -> TeleResult<bool> {
    let mut guard = loop {
        match ClientGuard::connect(name, creds.api_id, config_path).await {
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
    loop {
        enum Tick {
            Line(String),
            Eof,
            Update(Box<Update>),
            StreamError(grammers_client::InvocationError),
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
        };
        match tick {
            Tick::Eof => {
                output::log_line("info", "serve: stdin closed, shutting down");
                guard.close().await;
                return Ok(true);
            }
            Tick::Line(line) => match parse_incoming(&line) {
                Err(error) => emit(&response_err(None, error))?,
                Ok(ServeIn::Hello { protocol }) => {
                    let _ = protocol;
                }
                Ok(ServeIn::Action { id, op, params }) => {
                    handle_action(&guard, id, &op, params).await?
                }
            },
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
        let mut actual: Vec<&str> = SERVE_OPS.iter().map(|r| r.op).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        assert!(SERVE_OPS.iter().all(|r| r.op.starts_with("msg ")
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
}
