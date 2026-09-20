use comfy_table::{Cell, ContentArrangement, Table};
use std::io::Write;
use std::sync::Mutex;

static OUTPUT_FIELDS: Mutex<Option<Vec<Vec<String>>>> = Mutex::new(None);

pub fn set_output_fields(spec: &str) -> crate::error::TeleResult<()> {
    let parsed = parse_field_paths(spec)?;
    *OUTPUT_FIELDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(parsed);
    Ok(())
}

fn output_fields() -> Option<Vec<Vec<String>>> {
    OUTPUT_FIELDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
pub(crate) fn clear_output_fields() {
    *OUTPUT_FIELDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

#[cfg(test)]
static FIELDS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_fields_for_test() -> std::sync::MutexGuard<'static, ()> {
    FIELDS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) struct FieldsTestGuard;

#[cfg(test)]
impl FieldsTestGuard {
    pub(crate) fn set(spec: &str) -> Self {
        set_output_fields(spec).unwrap();
        FieldsTestGuard
    }
}

#[cfg(test)]
impl Drop for FieldsTestGuard {
    fn drop(&mut self) {
        clear_output_fields();
    }
}

pub fn parse_field_paths(spec: &str) -> crate::error::TeleResult<Vec<Vec<String>>> {
    let mut paths = Vec::new();
    for raw in spec.split(',') {
        let segment = raw.trim();
        let mut path = Vec::new();
        for part in segment.split('.') {
            if part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(crate::error::TeleError::Usage(format!(
                    "invalid --fields path {segment:?}: use comma-separated dotted names"
                )));
            }
            path.push(part.to_string());
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err(crate::error::TeleError::Usage(
            "invalid --fields value: use comma-separated dotted names".to_string(),
        ));
    }
    Ok(paths)
}

fn insert_field_path(
    root: &mut serde_json::Map<String, serde_json::Value>,
    path: &[String],
    value: serde_json::Value,
) {
    let mut current = root;
    for part in &path[..path.len().saturating_sub(1)] {
        let next = current
            .entry(part.clone())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !next.is_object() {
            *next = serde_json::Value::Object(serde_json::Map::new());
        }
        current = next.as_object_mut().unwrap_or_else(|| unreachable!());
    }
    if let Some(last) = path.last() {
        current.insert(last.clone(), value);
    }
}

fn project_object(
    data: &serde_json::Value,
    paths: &[Vec<String>],
    matched: &mut [bool],
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (index, path) in paths.iter().enumerate() {
        let mut current = data;
        let mut found = true;
        for part in path {
            match current.get(part) {
                Some(next) => current = next,
                None => {
                    found = false;
                    break;
                }
            }
        }
        if found {
            matched[index] = true;
            insert_field_path(&mut out, path, current.clone());
        }
    }
    serde_json::Value::Object(out)
}

pub fn apply_output_fields(
    value: &serde_json::Value,
) -> crate::error::TeleResult<serde_json::Value> {
    let Some(paths) = output_fields() else {
        return Ok(value.clone());
    };
    let mut matched = vec![false; paths.len()];
    let mut saw_object = false;
    let mut projected_results = None;
    if let Some(results) = value.get("results").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(results.len());
        for outcome in results {
            if outcome.get("data").is_some_and(|d| d.is_object()) {
                saw_object = true;
                let next = project_object(&outcome["data"], &paths, &mut matched);
                let mut outcome = outcome.clone();
                outcome["data"] = next;
                out.push(outcome);
            } else {
                out.push(outcome.clone());
            }
        }
        projected_results = Some(serde_json::Value::Array(out));
    }
    let mut projected_accounts = None;
    if let Some(rows) = value.get("accounts").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if row.is_object() {
                saw_object = true;
                out.push(project_object(row, &paths, &mut matched));
            } else {
                out.push(row.clone());
            }
        }
        projected_accounts = Some(serde_json::Value::Array(out));
    }
    if saw_object {
        for (index, path) in paths.iter().enumerate() {
            if !matched[index] {
                return Err(crate::error::TeleError::Usage(format!(
                    "unknown --fields {:?}: no result carries that field",
                    path.join(".")
                )));
            }
        }
        let mut map = serde_json::Map::new();
        if let Some(obj) = value.as_object() {
            for (key, val) in obj {
                if key != "results" && key != "accounts" {
                    map.insert(key.clone(), val.clone());
                }
            }
        }
        if let Some(results) = projected_results {
            map.insert("results".to_string(), results);
        }
        if let Some(accounts) = projected_accounts {
            map.insert("accounts".to_string(), accounts);
        }
        return Ok(serde_json::Value::Object(map));
    }
    if value.is_object() && value.get("results").is_none() && value.get("accounts").is_none() {
        let mut row_matched = vec![false; paths.len()];
        let row = project_object(value, &paths, &mut row_matched);
        for (index, path) in paths.iter().enumerate() {
            if !row_matched[index] {
                return Err(crate::error::TeleError::Usage(format!(
                    "unknown --fields {:?}: no result carries that field",
                    path.join(".")
                )));
            }
        }
        return Ok(row);
    }
    Ok(value.clone())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    #[serde(rename = "results")]
    pub accounts: Vec<AccountOutcome>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountOutcome {
    pub account: String,
    pub ok: bool,
    pub error: Option<serde_json::Value>,
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing)]
    pub exit_code: Option<i32>,
}

impl Envelope {
    pub fn new(accounts: Vec<AccountOutcome>, dry_run: bool, command: &str) -> Self {
        Envelope {
            ok: accounts.iter().all(|a| a.ok),
            command: Some(command.to_string()),
            dry_run,
            error: None,
            accounts,
        }
    }

    pub fn failed(dry_run: bool, command: &str, error: serde_json::Value) -> Self {
        Envelope {
            ok: false,
            command: Some(command.to_string()),
            dry_run,
            error: Some(error),
            accounts: Vec::new(),
        }
    }
}

pub fn log_line(level: &str, message: &str) {
    let Ok(parsed) = level.parse::<LogLevel>() else {
        let _ = writeln!(std::io::stderr(), "[error] log_line: unknown level");
        log_level(LogLevel::Error, message);
        return;
    };
    log_level(parsed, message);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "error" => Ok(LogLevel::Error),
            "warn" => Ok(LogLevel::Warn),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            _ => Err(()),
        }
    }
}

pub fn log_level(level: LogLevel, message: &str) {
    if level.rank() < crate::logging::min_line_level() {
        return;
    }
    let scrubbed = crate::error::scrub(message.to_string());
    let _ = writeln!(std::io::stderr(), "[{}] {scrubbed}", level.as_str());
}

pub fn print_json(value: &serde_json::Value) -> crate::error::TeleResult<()> {
    print_json_to(&mut std::io::stdout(), value)
}

pub fn print_json_to(
    w: &mut impl std::io::Write,
    value: &serde_json::Value,
) -> crate::error::TeleResult<()> {
    let projected = apply_output_fields(value)?;
    let line = serde_json::to_string(&projected)?;
    writeln!(w, "{line}")?;
    w.flush()?;
    Ok(())
}

pub fn print_json_result(value: &serde_json::Value) -> crate::error::TeleResult<()> {
    print_json(value)
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) -> crate::error::TeleResult<()> {
    print_table_to(&mut std::io::stdout(), headers, rows)?;
    Ok(())
}

fn print_table_to(
    w: &mut impl std::io::Write,
    headers: &[&str],
    rows: &[Vec<String>],
) -> crate::error::TeleResult<()> {
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(headers.iter().map(|h| Cell::new(strip_ansi(h))));
    for row in rows {
        table.add_row(row.iter().map(|cell| Cell::new(strip_ansi(cell))));
    }
    writeln!(w, "{table}")?;
    w.flush()?;
    Ok(())
}

pub fn print_line(line: &str) -> crate::error::TeleResult<()> {
    print_line_to(&mut std::io::stdout(), line)
}

/// ```
/// assert_eq!(telecli::output::strip_ansi("\x1b[31merror\x1b[0m"), "error");
/// assert_eq!(telecli::output::strip_ansi("plain"), "plain");
/// ```
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn print_line_to(w: &mut impl std::io::Write, line: &str) -> crate::error::TeleResult<()> {
    writeln!(w, "{line}")?;
    w.flush()?;
    Ok(())
}

pub fn print_raw(text: &str) -> crate::error::TeleResult<()> {
    print_raw_to(&mut std::io::stdout(), text)
}

/// ```
/// let mut buf: Vec<u8> = Vec::new();
/// telecli::output::print_raw_to(&mut buf, "a\nb").unwrap();
/// assert_eq!(buf, b"a\nb");
/// ```
pub fn print_raw_to(w: &mut impl std::io::Write, text: &str) -> crate::error::TeleResult<()> {
    w.write_all(text.as_bytes())?;
    w.flush()?;
    Ok(())
}

pub fn print_account_table(
    account: &str,
    multi: bool,
    headers: &[&str],
    rows: &[Vec<String>],
) -> crate::error::TeleResult<()> {
    print_account_table_to(&mut std::io::stdout(), account, multi, headers, rows)?;
    Ok(())
}

fn print_account_table_to(
    w: &mut impl std::io::Write,
    account: &str,
    multi: bool,
    headers: &[&str],
    rows: &[Vec<String>],
) -> crate::error::TeleResult<()> {
    if multi {
        writeln!(w, "== {account} ==")?;
    }
    print_table_to(w, headers, rows)
}

/// ```
/// assert!(telecli::output::machine_mode(true, false));
/// assert!(telecli::output::machine_mode(false, true));
/// assert!(!telecli::output::machine_mode(false, false));
/// ```
pub fn machine_mode(json: bool, jsonl: bool) -> bool {
    json || jsonl
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_outcome(account: &str) -> AccountOutcome {
        AccountOutcome {
            account: account.to_string(),
            ok: true,
            error: None,
            data: Some(serde_json::json!({"test": true})),
            exit_code: None,
        }
    }

    fn failure_outcome(account: &str) -> AccountOutcome {
        AccountOutcome {
            account: account.to_string(),
            ok: false,
            error: Some(serde_json::json!({"message": "failed"})),
            data: None,
            exit_code: Some(3),
        }
    }

    #[test]
    fn envelope_all_success_is_ok() {
        let env = Envelope::new(
            vec![success_outcome("a"), success_outcome("b")],
            false,
            "test",
        );
        assert!(env.ok);
        assert_eq!(env.accounts.len(), 2);
    }

    #[test]
    fn envelope_all_failure_is_not_ok() {
        let env = Envelope::new(
            vec![failure_outcome("a"), failure_outcome("b")],
            false,
            "test",
        );
        assert!(!env.ok);
    }

    #[test]
    fn envelope_mixed_is_not_ok() {
        let env = Envelope::new(
            vec![success_outcome("a"), failure_outcome("b")],
            false,
            "test",
        );
        assert!(!env.ok);
    }

    #[test]
    fn envelope_empty_is_ok() {
        let env = Envelope::new(vec![], false, "test");
        assert!(env.ok);
    }

    #[test]
    fn envelope_dry_run_field() {
        let env = Envelope::new(vec![], true, "test");
        assert!(env.dry_run);
        let env = Envelope::new(vec![], false, "test");
        assert!(!env.dry_run);
    }

    #[test]
    fn envelope_command_field() {
        let env = Envelope::new(vec![], false, "msg send");
        assert_eq!(env.command.as_deref(), Some("msg send"));
    }

    #[test]
    fn failed_envelope_shape() {
        let env = Envelope::failed(
            true,
            "account list",
            serde_json::json!({"type": "ConfigError", "message": "boom"}),
        );
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["ok"], serde_json::json!(false));
        assert_eq!(json["command"], serde_json::json!("account list"));
        assert_eq!(json["dry_run"], serde_json::json!(true));
        assert_eq!(json["results"], serde_json::json!([]));
        assert_eq!(json["error"]["type"], serde_json::json!("ConfigError"));
        assert_eq!(json["error"]["message"], serde_json::json!("boom"));
    }

    #[test]
    fn success_envelope_omits_error_key() {
        let env = Envelope::new(vec![success_outcome("a")], false, "test");
        let json = serde_json::to_value(&env).unwrap();
        assert!(json.get("error").is_none(), "stdout: {json}");
    }

    #[test]
    fn machine_mode_json() {
        assert!(machine_mode(true, false));
    }

    #[test]
    fn machine_mode_jsonl() {
        assert!(machine_mode(false, true));
    }

    #[test]
    fn machine_mode_both() {
        assert!(machine_mode(true, true));
    }

    #[test]
    fn machine_mode_neither() {
        assert!(!machine_mode(false, false));
    }

    #[test]
    fn envelope_serializes_ok_field() {
        let env = Envelope::new(vec![success_outcome("a")], false, "test");
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["ok"], serde_json::json!(true));
        assert_eq!(json["dry_run"], serde_json::json!(false));
        assert_eq!(json["command"], serde_json::json!("test"));
        assert!(json["results"].is_array());
    }

    #[test]
    fn envelope_serializes_results_array() {
        let env = Envelope::new(
            vec![success_outcome("a"), failure_outcome("b")],
            false,
            "test",
        );
        let json = serde_json::to_value(&env).unwrap();
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["account"], "a");
        assert_eq!(results[1]["account"], "b");
    }

    #[test]
    fn log_line_does_not_panic() {
        log_line("info", "test message");
        log_line("error", "test error");
        log_line("warn", "test warn");
        log_line("debug", "test debug");
    }

    #[test]
    fn log_level_typed_does_not_panic() {
        log_level(LogLevel::Info, "typed info");
        log_level(LogLevel::Error, "typed error");
    }

    #[test]
    fn log_line_unknown_level_does_not_panic() {
        log_line("verbose", "test verbose");
        log_line("", "test empty");
    }

    #[test]
    fn log_level_parses_all_names() {
        assert_eq!("error".parse::<LogLevel>(), Ok(LogLevel::Error));
        assert_eq!("warn".parse::<LogLevel>(), Ok(LogLevel::Warn));
        assert_eq!("info".parse::<LogLevel>(), Ok(LogLevel::Info));
        assert_eq!("debug".parse::<LogLevel>(), Ok(LogLevel::Debug));
        assert_eq!("ERROR".parse::<LogLevel>(), Err(()));
        assert_eq!("verbose".parse::<LogLevel>(), Err(()));
        assert_eq!("".parse::<LogLevel>(), Err(()));
    }

    #[test]
    fn log_level_rank_matches_line_floor_order() {
        assert!(LogLevel::Debug.rank() < LogLevel::Info.rank());
        assert!(LogLevel::Info.rank() < LogLevel::Warn.rank());
        assert!(LogLevel::Warn.rank() < LogLevel::Error.rank());
        assert_eq!(LogLevel::Error.as_str(), "error");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Debug.as_str(), "debug");
    }

    #[test]
    fn print_raw_to_preserves_exact_bytes() {
        let mut buf: Vec<u8> = Vec::new();
        print_raw_to(&mut buf, "a\nb\n").unwrap();
        assert_eq!(buf, b"a\nb\n");
        let mut empty: Vec<u8> = Vec::new();
        print_raw_to(&mut empty, "").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn print_raw_to_failing_writer_propagates_broken_pipe() {
        let mut w = FailingWriter {
            kind: std::io::ErrorKind::BrokenPipe,
        };
        let err = print_raw_to(&mut w, "boom").unwrap_err();
        assert!(err.is_broken_pipe());
        assert_eq!(err.exit_code(), crate::error::EXIT_OK);
    }

    #[test]
    fn print_json_to_closed_pipe_returns_err() {
        let _lock = lock_fields_for_test();
        clear_output_fields();
        let (reader, mut writer) = std::io::pipe().unwrap();
        drop(reader);
        let res = print_json_to(&mut writer, &serde_json::json!({"a": 1}));
        assert!(res.is_err(), "expected Err from closed pipe");
    }

    #[test]
    fn print_json_to_open_writer_succeeds() {
        let _lock = lock_fields_for_test();
        clear_output_fields();
        let mut buf: Vec<u8> = Vec::new();
        print_json_to(&mut buf, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "{\"a\":1}\n");
    }

    struct FailingWriter {
        kind: std::io::ErrorKind,
    }

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.kind, "sink failed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn print_table_to_failing_writer_returns_err_without_panic() {
        let mut w = FailingWriter {
            kind: std::io::ErrorKind::BrokenPipe,
        };
        let err = print_table_to(&mut w, &["a"], &sample_rows()).unwrap_err();
        assert!(err.is_broken_pipe(), "expected BrokenPipe");
    }

    #[test]
    fn print_line_to_failing_writer_propagates_broken_pipe() {
        let mut w = FailingWriter {
            kind: std::io::ErrorKind::BrokenPipe,
        };
        let err = print_line_to(&mut w, "boom").unwrap_err();
        assert!(
            err.is_broken_pipe(),
            "failing writer's error must be the broken-pipe variant"
        );
        assert_eq!(err.exit_code(), crate::error::EXIT_OK);
    }

    #[test]
    fn print_line_to_open_writer_emits_line() {
        let mut buf: Vec<u8> = Vec::new();
        print_line_to(&mut buf, "hello").unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn other_io_error_stays_other() {
        let err: crate::error::TeleError =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied").into();
        assert!(!err.is_broken_pipe());
    }

    fn sample_rows() -> Vec<Vec<String>> {
        vec![vec!["x".to_string(), "y".to_string()]]
    }

    use super::{lock_fields_for_test, FieldsTestGuard};

    fn envelope_with_data(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "ok": true,
            "command": "msg send",
            "dry_run": true,
            "results": [{"account": "work", "ok": true, "error": null, "data": data}],
        })
    }

    #[test]
    fn account_table_multi_prints_header_before_table() {
        let mut buf: Vec<u8> = Vec::new();
        print_account_table_to(&mut buf, "work", true, &["a", "b"], &sample_rows()).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("== work =="), "stdout: {out}");
        let header_pos = out.find("== work ==").unwrap();
        let table_pos = out.find('a').unwrap();
        assert!(header_pos < table_pos, "header must precede table: {out}");
    }

    #[test]
    fn account_table_single_prints_no_header() {
        let mut buf: Vec<u8> = Vec::new();
        print_account_table_to(&mut buf, "work", false, &["a", "b"], &sample_rows()).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("== work =="), "stdout: {out}");
    }

    #[test]
    fn account_table_single_matches_plain_table_bytes() {
        let mut with: Vec<u8> = Vec::new();
        let mut plain: Vec<u8> = Vec::new();
        print_account_table_to(&mut with, "work", false, &["a", "b"], &sample_rows()).unwrap();
        print_table_to(&mut plain, &["a", "b"], &sample_rows()).unwrap();
        assert_eq!(with, plain);
    }

    #[test]
    fn account_table_multi_still_prints_rows() {
        let mut buf: Vec<u8> = Vec::new();
        print_account_table_to(&mut buf, "work", true, &["a", "b"], &sample_rows()).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains('x') && out.contains('y'), "stdout: {out}");
    }

    #[test]
    fn parse_field_paths_accepts_simple_and_dotted() {
        assert_eq!(
            parse_field_paths("would").unwrap(),
            vec![vec!["would".to_string()]]
        );
        assert_eq!(
            parse_field_paths("id, peer.id ,message").unwrap(),
            vec![
                vec!["id".to_string()],
                vec!["peer".to_string(), "id".to_string()],
                vec!["message".to_string()],
            ]
        );
    }

    #[test]
    fn parse_field_paths_dedupes_repeats() {
        assert_eq!(
            parse_field_paths("id,id").unwrap(),
            vec![vec!["id".to_string()]]
        );
    }

    #[test]
    fn parse_field_paths_rejects_empty_and_bad_chars() {
        for bad in [
            "", "   ", "id,,date", "id..date", ".id", "id.", "a b", "a/b", "a*",
        ] {
            let err = parse_field_paths(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::TeleError::Usage(_)),
                "{bad:?}: {err}"
            );
            assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        }
    }

    #[test]
    fn apply_output_fields_without_setting_passes_through() {
        let _lock = lock_fields_for_test();
        clear_output_fields();
        let value = envelope_with_data(serde_json::json!({"a": 1}));
        assert_eq!(apply_output_fields(&value).unwrap(), value);
    }

    #[test]
    fn apply_output_fields_projects_results_data() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("would");
        let value = envelope_with_data(
            serde_json::json!({"would": "send message to chat me", "chat": "me", "dry_run": true}),
        );
        let projected = apply_output_fields(&value).unwrap();
        assert_eq!(
            projected["results"][0]["data"],
            serde_json::json!({"would": "send message to chat me"})
        );
        assert_eq!(projected["ok"], serde_json::json!(true));
        assert_eq!(projected["command"], serde_json::json!("msg send"));
    }

    #[test]
    fn apply_output_fields_preserves_mixed_rows_and_siblings() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("would");
        let value = serde_json::json!({
            "ok": false,
            "command": "msg send",
            "dry_run": true,
            "results": [
                {"account": "a", "ok": true, "data": {"would": "x", "chat": "me"}, "error": null},
                {"account": "b", "ok": false, "data": null, "error": {"type": "Error"}},
                {"account": "c", "ok": true, "data": {"would": "y", "chat": "you"}, "error": null}
            ]
        });
        let projected = apply_output_fields(&value).unwrap();
        assert_eq!(projected["ok"], serde_json::json!(false));
        assert_eq!(projected["command"], serde_json::json!("msg send"));
        assert_eq!(projected["dry_run"], serde_json::json!(true));
        assert_eq!(
            projected["results"][0]["data"],
            serde_json::json!({"would": "x"})
        );
        assert_eq!(projected["results"][0]["account"], serde_json::json!("a"));
        assert_eq!(projected["results"][1]["data"], serde_json::Value::Null);
        assert_eq!(
            projected["results"][1]["error"],
            serde_json::json!({"type": "Error"})
        );
        assert_eq!(
            projected["results"][2]["data"],
            serde_json::json!({"would": "y"})
        );
    }

    #[test]
    fn apply_output_fields_supports_dotted_paths() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("peer.id,message");
        let value = envelope_with_data(
            serde_json::json!({"peer": {"id": 7, "kind": "user"}, "message": "hi", "date": "x"}),
        );
        let projected = apply_output_fields(&value).unwrap();
        assert_eq!(
            projected["results"][0]["data"],
            serde_json::json!({"peer": {"id": 7}, "message": "hi"})
        );
    }

    #[test]
    fn apply_output_fields_merges_overlapping_paths() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("peer,peer.id");
        let value = envelope_with_data(serde_json::json!({"peer": {"id": 7, "kind": "user"}}));
        let projected = apply_output_fields(&value).unwrap();
        assert_eq!(
            projected["results"][0]["data"],
            serde_json::json!({"peer": {"id": 7, "kind": "user"}})
        );
    }

    #[test]
    fn apply_output_fields_unknown_field_is_usage() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("would,nosuchfield");
        let value = envelope_with_data(serde_json::json!({"would": "send message to chat me"}));
        let err = apply_output_fields(&value).unwrap_err();
        assert!(
            matches!(err, crate::error::TeleError::Usage(_)),
            "err: {err}"
        );
        assert!(err.message().contains("nosuchfield"), "err: {err}");
    }

    #[test]
    fn apply_output_fields_projects_streaming_rows() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("event");
        let row = serde_json::json!({"event": "NewMessage", "account": "work", "chat_id": 7});
        assert_eq!(
            apply_output_fields(&row).unwrap(),
            serde_json::json!({"event": "NewMessage"})
        );
    }

    #[test]
    fn apply_output_fields_unknown_field_on_streaming_row_is_usage() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("event,nosuchfield");
        let row = serde_json::json!({"event": "NewMessage", "account": "work", "chat_id": 7});
        let err = apply_output_fields(&row).unwrap_err();
        assert!(
            matches!(err, crate::error::TeleError::Usage(_)),
            "err: {err}"
        );
        assert!(err.message().contains("nosuchfield"), "err: {err}");
    }

    #[test]
    fn apply_output_fields_empty_results_pass_through() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("would");
        let value = serde_json::json!({"ok": false, "results": []});
        assert_eq!(apply_output_fields(&value).unwrap(), value);
    }

    #[test]
    fn apply_output_fields_projects_accounts_array() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("name");
        let value = serde_json::json!({
            "ok": true,
            "results": [{"account": "work", "ok": true, "error": null, "data": {"name": "work", "tags": ""}}],
            "accounts": [{"name": "work", "tags": "", "session": "present"}],
        });
        let projected = apply_output_fields(&value).unwrap();
        assert_eq!(
            projected["results"][0]["data"],
            serde_json::json!({"name": "work"})
        );
        assert_eq!(
            projected["accounts"][0],
            serde_json::json!({"name": "work"})
        );
    }

    #[test]
    fn print_json_to_applies_configured_fields() {
        let _lock = lock_fields_for_test();
        let _guard = FieldsTestGuard::set("would");
        let mut buf: Vec<u8> = Vec::new();
        print_json_to(
            &mut buf,
            &envelope_with_data(
                serde_json::json!({"would": "send message to chat me", "chat": "me"}),
            ),
        )
        .unwrap();
        let back: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(
            back["results"][0]["data"],
            serde_json::json!({"would": "send message to chat me"})
        );
    }
}
