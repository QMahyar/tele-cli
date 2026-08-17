use comfy_table::{Cell, Table};

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
        _ => 0,
    };
    if lv < min {
        return;
    }
    eprintln!("[{level}] {message}");
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

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut table = Table::new();
    table.set_header(headers.iter().map(|h| Cell::new(*h)));
    for row in rows {
        table.add_row(row.iter().map(Cell::new));
    }
    println!("{table}");
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
}
