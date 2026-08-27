use comfy_table::{Cell, ContentArrangement, Table};
use std::io::Write;

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
    let min = crate::logging::min_line_level();
    let lv = match level {
        "error" => 3,
        "warn" => 2,
        "info" => 1,
        "debug" => 0,
        other => {
            let _ = writeln!(
                std::io::stderr(),
                "[error] log_line: unknown level \"{other}\""
            );
            3
        }
    };
    if lv < min {
        return;
    }
    let _ = writeln!(std::io::stderr(), "[{level}] {message}");
}

pub fn print_json(value: &serde_json::Value) -> crate::error::TeleResult<()> {
    print_json_to(&mut std::io::stdout(), value)
}

pub fn print_json_to(
    w: &mut impl std::io::Write,
    value: &serde_json::Value,
) -> crate::error::TeleResult<()> {
    let line = serde_json::to_string(value)?;
    writeln!(w, "{line}")?;
    w.flush()?;
    Ok(())
}

pub fn print_json_result(value: &serde_json::Value) -> crate::error::TeleResult<()> {
    print_json_to(&mut std::io::stdout(), value)
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) -> crate::error::TeleResult<()> {
    print_table_to(&mut std::io::stdout(), headers, rows)?;
    Ok(())
}

fn print_table_to(
    w: &mut impl std::io::Write,
    headers: &[&str],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(headers.iter().map(|h| Cell::new(*h)));
    for row in rows {
        table.add_row(row.iter().map(Cell::new));
    }
    writeln!(w, "{table}")?;
    w.flush()?;
    Ok(())
}

pub fn print_line(line: &str) -> crate::error::TeleResult<()> {
    print_line_to(&mut std::io::stdout(), line)
}

pub fn print_line_to(w: &mut impl std::io::Write, line: &str) -> crate::error::TeleResult<()> {
    writeln!(w, "{line}")?;
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
) -> std::io::Result<()> {
    if multi {
        writeln!(w, "== {account} ==")?;
    }
    print_table_to(w, headers, rows)
}

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
    fn log_line_unknown_level_does_not_panic() {
        log_line("verbose", "test verbose");
        log_line("", "test empty");
    }

    #[test]
    fn print_json_to_closed_pipe_returns_err() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        drop(reader);
        let res = print_json_to(&mut writer, &serde_json::json!({"a": 1}));
        assert!(res.is_err(), "expected Err from closed pipe");
    }

    #[test]
    fn print_json_to_open_writer_succeeds() {
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
        let res = print_table_to(&mut w, &["a"], &sample_rows());
        assert!(res.is_err(), "expected Err from failing writer");
    }

    #[test]
    fn print_line_to_failing_writer_propagates_broken_pipe() {
        let mut w = FailingWriter {
            kind: std::io::ErrorKind::BrokenPipe,
        };
        let err = print_line_to(&mut w, "boom").unwrap_err();
        assert!(
            crate::error::TeleError::BrokenPipe.is_broken_pipe(),
            "variant exists"
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
}
