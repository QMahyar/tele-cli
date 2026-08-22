use clap::Parser;
use grammers_client::tl;
use grammers_client::update::Update;

use crate::client::ClientGuard;
use crate::commands::listen::{
    event_row, getstate_probe_error, handle_stream_failure, is_empty_update, poll_timeout,
    update_peer,
};
use crate::error::{TeleError, TeleResult, EXIT_OK};
use crate::executor::GlobalFlags;
use crate::output;

pub const SERVE_PROTOCOL: u32 = 1;

const SERVE_EVENTS: &[&str] = &["NewMessage", "MessageEdited"];

#[derive(Parser)]
pub struct ServeArgs {
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "NewMessage",
        help = "events to stream (serve-A subset: NewMessage, MessageEdited)"
    )]
    events: Vec<String>,
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
        return Ok(ServeIn::Hello {
            protocol: protocol as u32,
        });
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

pub fn hello_out(account: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "hello",
        "protocol": SERVE_PROTOCOL,
        "account": account,
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
        format!("script protocol {theirs} != serve protocol {SERVE_PROTOCOL}; update one side"),
    )
}

fn emit(value: &serde_json::Value) -> TeleResult<()> {
    output::print_json(value)
}

async fn handle_action(id: u64, op: &str) -> TeleResult<()> {
    if op == "ping" {
        return emit(&response_ok(id, serde_json::json!({ "pong": true })));
    }
    output::log_line(
        "info",
        &format!("serve: action {op} not implemented yet (serve-B)"),
    );
    emit(&response_err(
        Some(id),
        err_json(
            "NotImplemented",
            format!("op {op} arrives in serve-B; serve-A streams events only"),
        ),
    ))
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
    loop {
        let eof = serve_connection(
            &name,
            &args.events,
            &creds,
            config_path.as_deref(),
            &mut rx,
            &mut failures,
            greeted,
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
    creds: &crate::config::Credentials,
    config_path: Option<&std::path::Path>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    failures: &mut u32,
    greet: bool,
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
                catch_up: true,
                update_queue_limit: Some(1000),
            },
        )
        .await
        .map_err(|e| TeleError::Other(e.to_string()))?;
    *failures = 0;
    if greet {
        output::log_line("info", "serve: reconnected");
    } else {
        emit(&hello_out(name))?;
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
                    if protocol != SERVE_PROTOCOL {
                        emit(&response_err(None, version_error(protocol)))?;
                        output::log_line(
                            "error",
                            &format!("serve: script spoke protocol {protocol}, serve speaks {SERVE_PROTOCOL}; stopping"),
                        );
                        return Err(TeleError::Usage(
                            "serve protocol version mismatch".to_string(),
                        ));
                    }
                }
                Ok(ServeIn::Action { id, op, .. }) => handle_action(id, &op).await?,
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
                let row = crate::serialize::message_to_json(message)?;
                let chat_id = update_peer(update.raw()).and_then(|p| p.bare_id());
                emit(&event_row(kind, name, chat_id, None, Some(row)))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn hello_out_carries_protocol_and_account() {
        let hello = hello_out("work");
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["protocol"], SERVE_PROTOCOL);
        assert_eq!(hello["account"], "work");
    }

    #[test]
    fn version_mismatch_message_names_both_sides() {
        let msg = version_error(2)["message"].as_str().unwrap().to_string();
        assert!(msg.contains("2") && msg.contains(&SERVE_PROTOCOL.to_string()));
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
}
