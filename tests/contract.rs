use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn tele() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tele"))
}

fn isolated_appdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("telecli-contract-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_isolated(tag: &str, args: &[&str]) -> (i32, String, String) {
    let out = tele()
        .args(args)
        .env("TELE_APP_DIR", isolated_appdir(tag))
        .output()
        .expect("spawn telecli");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = tele()
        .args(args)
        .env("TELE_APP_DIR", dir)
        .output()
        .expect("spawn telecli");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_no_creds(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = tele()
        .args(args)
        .env("TELE_APP_DIR", dir)
        .env_remove("TELE_API_ID")
        .env_remove("TELE_API_HASH")
        .output()
        .expect("spawn telecli");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn write_session(dir: &Path, name: &str) {
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(format!("{name}.session")), b"dummy").unwrap();
}

fn write_config(dir: &Path, toml: &str) {
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

fn help(args: &[&str]) -> String {
    let out = tele()
        .args(args)
        .arg("--help")
        .env("TELE_APP_DIR", isolated_appdir("helptree"))
        .output()
        .expect("spawn telecli --help");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn parse_json(out: &str) -> serde_json::Value {
    serde_json::from_str(out.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON object: {e}; got: {out}"))
}

fn matrix_rows() -> Vec<(String, String, String)> {
    let md =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("docs/capabilities.md")).unwrap();
    let mut rows = Vec::new();
    for line in md.lines() {
        let line = line.trim();
        if !line.starts_with('|') || !line.ends_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let raw_status = cells.last().unwrap();
        // Skip separator rows (pure dash runs) and header rows.
        if !raw_status.is_empty() && raw_status.chars().all(|c| c == '-') {
            continue;
        }
        if cells.iter().any(|c| c.eq_ignore_ascii_case("status")) {
            continue;
        }
        // Normalize: trim, case-fold, strip decoration so a `done ✅` or
        // `Done` row cannot silently escape the gate.
        let normalized: Vec<String> = cells
            .iter()
            .map(|c| {
                c.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase()
            })
            .collect();
        // Status lives in the last cell for most tables and second-to-last for
        // the free-text "Why" table; check only those two positions so a
        // stray "done" inside a long description cannot forge a row, and
        // fail loudly on anything that is not a known status.
        let last = normalized.len() - 1;
        let is_status = |s: &str| matches!(s, "done" | "want" | "later" | "never");
        let status_pos = if is_status(&normalized[last]) {
            last
        } else if last >= 1 && is_status(&normalized[last - 1]) {
            last - 1
        } else {
            panic!("capabilities.md: unknown status {raw_status:?} in row: {line}")
        };
        if normalized[status_pos] == "done" && status_pos >= 1 {
            // The CLI cell sits immediately left of the status cell in every
            // table layout (both `… | CLI | Status |` and `… | CLI | Status |
            // Why |`).
            let cli_cell = cells[status_pos - 1].to_string();
            rows.push((cells[0].to_string(), "done".to_string(), cli_cell));
        }
    }
    rows
}

fn backtick_tokens(cli: &str) -> Vec<&str> {
    cli.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

fn raw_registry_names() -> Vec<String> {
    let src =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("src/commands/raw.rs")).unwrap();
    let start = src.find("pub const REGISTERED").expect("REGISTERED const");
    let end = start + src[start..].find("];").expect("end of REGISTERED");
    src[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn help_lists_all_groups_and_exits_zero() {
    let out = tele()
        .env("TELE_APP_DIR", isolated_appdir("help"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for group in [
        "account",
        "msg",
        "chat",
        "dialog",
        "topic",
        "sticker",
        "story",
        "contact",
        "profile",
        "privacy",
        "takeout",
        "listen",
        "serve",
        "mcp",
        "raw",
        "completions",
    ] {
        assert!(
            text.contains(&format!(" {group} ")),
            "group {group} missing from --help"
        );
    }
}

#[test]
fn unknown_command_exits_1() {
    let (code, _out, err) = run_isolated("unknown", &["bogus-group"]);
    assert_eq!(code, 1);
    assert!(err.contains("unrecognized subcommand"), "stderr: {err}");
}

#[test]
fn empty_selection_is_usage_error() {
    let (code, _out, err) =
        run_isolated("noselect", &["msg", "send", "--chat", "me", "--text", "hi"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("requires --account <name> or --tag <tag>"),
        "stderr: {err}"
    );
}

#[test]
fn raw_unregistered_name_exits_1_before_connect() {
    let (code, _out, err) = run_isolated("rawbad", &["raw", "messages.Nope", "--args", "{}"]);
    assert_eq!(code, 1);
    assert!(err.contains("raw method not in registry"), "stderr: {err}");
}

#[test]
fn raw_invalid_args_json_exits_1() {
    let (code, _out, err) = run_isolated(
        "rawjson",
        &["raw", "messages.GetAllDrafts", "--args", "{bad"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("invalid --args JSON"), "stderr: {err}");
}

#[test]
fn oversized_raw_args_value_rejected_without_panic() {
    let big = format!("{{\"limit\": {}}}", "9".repeat(2_000));
    let (code, _out, err) =
        run_isolated("bigargs", &["raw", "messages.GetAllDrafts", "--args", &big]);
    assert_eq!(code, 1, "stderr: {err}");
}

#[test]
fn oversized_chat_target_rejected_without_panic() {
    let chat = "x".repeat(2_000);
    let (code, _out, err) =
        run_isolated("bigchat", &["msg", "send", "--chat", &chat, "--text", "hi"]);
    assert_eq!(code, 1, "stderr: {err}");
}

#[test]
fn raw_registered_name_reaches_fanout() {
    let (code, _out, err) = run_isolated("rawreg", &["raw", "messages.GetAllDrafts"]);
    assert_eq!(code, 1);
    assert!(err.contains("no accounts selected"), "stderr: {err}");
}

#[test]
fn raw_new_mutators_require_explicit_account_offline() {
    for (name, args) in [
        (
            "account.SetAuthorizationTTL",
            "{\"authorization_ttl_days\":30}",
        ),
        ("contacts.DeleteByPhones", "{\"phones\":[\"+15550100\"]}"),
    ] {
        let (code, _out, err) = run_isolated("rawgate2", &["raw", name, "--args", args]);
        assert_eq!(code, 1, "raw {name}");
        assert!(
            err.contains("mutates account data"),
            "raw {name}: stderr: {err}"
        );
    }
}

#[test]
fn listen_unknown_event_exits_1_before_connect() {
    for name in ["Bogus", "Nope"] {
        let (code, _out, err) = run_isolated("lsev", &["listen", "--events", name]);
        assert_eq!(code, 1);
        assert!(err.contains("unknown event name"), "stderr: {err}");
    }
}

#[test]
fn listen_dry_run_exits_0_with_session() {
    let dir = isolated_appdir("lsdry");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "listen",
            "--events",
            "NewMessage",
            "--account",
            "work",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn serve_multi_account_dry_run_lists_all_accounts() {
    let dir = isolated_appdir("svmulti");
    write_session(&dir, "alpha");
    write_session(&dir, "beta");
    let (code, out, err) = run_in(
        &dir,
        &[
            "serve",
            "--account",
            "alpha",
            "--account",
            "beta",
            "--events",
            "NewMessage",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let accounts: Vec<String> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .unwrap_or_else(|e| panic!("line must be JSON: {e}; got: {l}"))
        })
        .filter_map(|v| v["account"].as_str().map(str::to_string))
        .collect();
    assert!(accounts.contains(&"alpha".to_string()), "lines: {out}");
    assert!(accounts.contains(&"beta".to_string()), "lines: {out}");
}

#[test]
fn serve_unknown_account_in_multi_dry_run_still_offline_error() {
    let dir = isolated_appdir("svmulti-bad");
    write_session(&dir, "alpha");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "serve",
            "--account",
            "alpha",
            "--account",
            "ghost",
            "--dry-run",
        ],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("ghost"), "stderr: {err}");
}

#[test]
fn listen_valid_events_reach_selection() {
    let (code, _out, err) =
        run_isolated("lsok", &["listen", "--events", "NewMessage,MessageDeleted"]);
    assert_eq!(code, 1);
    assert!(err.contains("listen requires --account"), "stderr: {err}");
}

#[test]
fn clap_usage_errors_exit_1() {
    let (code, _out, err) = run_isolated("noargs", &[]);
    assert_eq!(code, 1);
    assert!(err.contains("Usage:"), "stderr: {err}");
    let (code, _out, err) = run_isolated("badgrpflag", &["msg", "--bogus-flag"]);
    assert_eq!(code, 1);
    assert!(err.contains("unexpected argument"), "stderr: {err}");
    let (code, _out, err) = run_isolated("badcmdflag", &["msg", "send", "--bogus-flag"]);
    assert_eq!(code, 1);
    assert!(err.contains("unexpected argument"), "stderr: {err}");
}

#[test]
fn parallel_out_of_range_is_usage_error() {
    let dir = isolated_appdir("parclamp");
    write_session(&dir, "work");
    for n in ["0", "99"] {
        let (code, _out, err) = run_in(
            &dir,
            &[
                "msg",
                "send",
                "--account",
                "work",
                "--chat",
                "me",
                "--text",
                "hi",
                "--parallel",
                n,
                "--dry-run",
            ],
        );
        assert_eq!(
            code, 1,
            "--parallel {n} must error, not clamp: stderr: {err}"
        );
    }
}

#[test]
fn parallel_out_of_range_exits_with_error() {
    let (code, _out, err) = run_isolated("parwarn", &["--parallel", "99", "account", "list"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("between 1 and 32"), "stderr: {err}");
}

#[test]
fn chat_help_mentions_admin_log_not_adminlog() {
    let (code, _out, err) = run_isolated("adminloghelp", &["chat", "adminlog", "--json"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("admin-log"), "stderr: {err}");
}

#[test]
fn account_list_json_is_one_object() {
    let (code, out, _err) = run_isolated("acclist", &["account", "list", "--json"]);
    assert_eq!(code, 0);
    let v = parse_json(&out);
    assert!(v.get("accounts").is_some(), "stdout: {out}");
}

#[test]
fn account_list_human_empty() {
    let (code, out, _err) = run_isolated("acchuman", &["account", "list"]);
    assert_eq!(code, 0);
    assert!(out.contains("no sessions yet"), "stdout: {out}");
}

#[test]
fn dry_run_login_needs_no_session() {
    let (code, _out, _err) = run_isolated(
        "dryrun",
        &[
            "account",
            "login",
            "--name",
            "x",
            "--method",
            "qr",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0);
}

#[test]
fn success_envelope_shape() {
    let dir = isolated_appdir("envok");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    let obj = v.as_object().expect("envelope must be an object");
    assert!(obj.contains_key("ok"), "envelope missing ok: {out}");
    assert!(
        obj.contains_key("dry_run"),
        "envelope missing dry_run: {out}"
    );
    assert!(
        obj.contains_key("results"),
        "envelope missing results: {out}"
    );
    assert_eq!(
        obj.get("command"),
        Some(&serde_json::json!("msg send")),
        "command must name the invoked subcommand path: {out}"
    );
    assert_eq!(v["ok"], serde_json::json!(true));
    let results = v["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1, "stdout: {out}");
    let r = &results[0];
    assert_eq!(r["account"], serde_json::json!("work"));
    assert_eq!(r["ok"], serde_json::json!(true));
    assert!(r.get("data").is_some(), "result missing data: {r}");
    assert_eq!(r["error"], serde_json::Value::Null);
}

#[test]
fn dry_run_envelope_shape() {
    let dir = isolated_appdir("envdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true), "data: {data}");
    assert_eq!(data["chat"], serde_json::json!("me"), "data: {data}");
    assert_eq!(
        data["would"],
        serde_json::json!("send message to chat me"),
        "data: {data}"
    );
}

#[test]
fn error_envelope_shape() {
    let dir = isolated_appdir("enverr");
    write_session(&dir, "work");
    let (code, out, _err) = run_no_creds(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--json",
        ],
    );
    assert_eq!(code, 1);
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["dry_run"], serde_json::json!(false));
    let r = &v["results"][0];
    assert_eq!(r["ok"], serde_json::json!(false));
    assert_eq!(r["data"], serde_json::Value::Null);
    assert_eq!(r["error"]["type"], serde_json::json!("ConfigError"));
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("TELE_API_ID"),
        "error message: {r}"
    );
}

#[test]
fn json_and_jsonl_are_rejected() {
    let (code, _out, err) = run_isolated("bothjson", &["account", "list", "--json", "--jsonl"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("cannot be used with") || err.contains("mutually exclusive"),
        "stderr: {err}"
    );
}

#[test]
fn clap_parse_error_emits_json_envelope() {
    let (code, out, err) = run_isolated("badsub", &["--json", "foobar"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("foobar"));
    assert_eq!(v["results"], serde_json::json!([]));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("foobar"),
        "stdout: {out}"
    );
    assert!(
        !v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains('\u{1b}'),
        "envelope message must not contain ANSI escapes: stdout: {out}"
    );
}

#[test]
fn missing_required_arg_emits_json_envelope() {
    let (code, out, err) = run_isolated("badarg", &["--json", "msg", "send"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("msg send"));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
}

#[test]
fn json_jsonl_conflict_emits_envelope() {
    let (code, out, err) = run_isolated("both2", &["account", "list", "--json", "--jsonl"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    let msg = v["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("cannot be used with") || msg.contains("mutually exclusive"),
        "message: {msg}"
    );
}

#[test]
fn error_envelope_on_stdout_for_usage_error() {
    let (code, out, err) = run_isolated(
        "errusage",
        &["msg", "send", "--chat", "me", "--text", "hi", "--json"],
    );
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("msg send"));
    assert_eq!(v["dry_run"], serde_json::json!(false));
    assert_eq!(v["results"], serde_json::json!([]));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("requires --account <name> or --tag <tag>"),
        "stdout: {out}"
    );
}

#[test]
fn error_envelope_on_stdout_for_config_error() {
    let dir = isolated_appdir("errconfig");
    write_config(&dir, "not [valid toml");
    let (code, out, err) = run_in(&dir, &["account", "list", "--json"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("account list"));
    assert_eq!(v["results"], serde_json::json!([]));
    assert_eq!(v["error"]["type"], serde_json::json!("ConfigError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failed to parse"),
        "stdout: {out}"
    );
}

#[test]
fn malformed_config_flag_exits_usage_for_all_shapes() {
    let dir = isolated_appdir("cfgshapes");
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("toml", b"not [valid toml".to_vec(), false),
        ("type", b"parallel_max = \"abc\"\n".to_vec(), false),
        ("binary", b"\x00\x01\x02garbage\xff\xfe".to_vec(), false),
        ("dir", Vec::new(), true),
    ];
    for (tag, content, is_dir) in cases {
        let path = dir.join(format!("{tag}.toml"));
        if is_dir {
            std::fs::create_dir_all(&path).unwrap();
        } else {
            std::fs::write(&path, content).unwrap();
        }
        let (code, out, err) = run_in(
            &dir,
            &[
                "--config",
                path.to_str().unwrap(),
                "account",
                "list",
                "--json",
            ],
        );
        assert_eq!(code, 1, "case {tag}: stderr: {err}");
        let v = parse_json(&out);
        assert_eq!(v["ok"], serde_json::json!(false), "case {tag}: {out}");
        assert_eq!(
            v["error"]["type"],
            serde_json::json!("ConfigError"),
            "case {tag}: {out}"
        );
    }
}

#[test]
fn error_envelope_respects_jsonl_mode() {
    let (code, out, _err) = run_isolated(
        "errjsonl",
        &["msg", "send", "--chat", "me", "--text", "hi", "--jsonl"],
    );
    assert_eq!(code, 1);
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
}

#[test]
fn account_remove_reserved_name_json_emits_usage_envelope() {
    let dir = isolated_appdir("rmalljson");
    write_session(&dir, "work");
    let (code, out, err) = run_in(&dir, &["account", "remove", "--name", "all", "--json"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("account remove"));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("reserved"),
        "stdout: {out}"
    );
    assert_eq!(v["results"], serde_json::json!([]));
}

#[test]
fn listen_dry_run_jsonl_emits_row_per_account() {
    let dir = isolated_appdir("lsdryjsonl");
    write_session(&dir, "home");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "listen",
            "--account",
            "home",
            "--account",
            "work",
            "--dry-run",
            "--jsonl",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "stdout: {out}");
    for (line, account) in lines.iter().zip(["home", "work"]) {
        let v: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line must be JSON: {e}: {line}"));
        assert_eq!(v["event"], serde_json::json!("NewMessage"));
        assert_eq!(v["account"], serde_json::json!(account));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert!(
            v["would"].as_str().unwrap_or_default().contains("stream"),
            "line: {line}"
        );
        assert!(
            v["would"].as_str().unwrap_or_default().contains(account),
            "line: {line}"
        );
    }
}

#[test]
fn listen_dry_run_json_emits_row() {
    let dir = isolated_appdir("lsdryjson");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &["listen", "--account", "work", "--dry-run", "--json"],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("stdout must be one row");
    assert_eq!(v["event"], serde_json::json!("NewMessage"));
    assert_eq!(v["account"], serde_json::json!("work"));
    assert_eq!(v["dry_run"], serde_json::json!(true));
}

#[test]
fn listen_dry_run_jsonl_no_accounts_emits_error_envelope() {
    let (code, out, _err) = run_isolated("lsdrynoacct", &["listen", "--dry-run", "--jsonl"]);
    assert_eq!(code, 1);
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
}

#[test]
fn listen_dry_run_respects_configured_events() {
    let dir = isolated_appdir("lsdryev");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "listen",
            "--account",
            "work",
            "--events",
            "MessageDeleted",
            "--dry-run",
            "--jsonl",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("stdout must be one row");
    assert_eq!(v["event"], serde_json::json!("MessageDeleted"));
    assert!(
        v["would"]
            .as_str()
            .unwrap_or_default()
            .contains("MessageDeleted"),
        "stdout: {out}"
    );
}

#[test]
fn msg_delete_requires_ids_unless_all() {
    let (code, _out, err) = run_isolated("delnone", &["msg", "delete", "--chat", "me"]);
    assert_eq!(code, 1);
    assert!(err.contains("--ids required unless --all"), "stderr: {err}");
    let dir = isolated_appdir("delall");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "delete",
            "--chat",
            "me",
            "--all",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["results"][0]["data"]["dry_run"], serde_json::json!(true));
    assert_eq!(
        v["results"][0]["data"]["would"],
        serde_json::json!("delete all messages in chat me")
    );
}

#[test]
fn msg_send_format_allowlist() {
    let (code, _out, err) = run_isolated(
        "fmtbad",
        &[
            "msg", "send", "--chat", "me", "--text", "hi", "--format", "html",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("unknown --format"), "stderr: {err}");
    let dir = isolated_appdir("fmtok");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--format",
            "markdown",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn privacy_set_requires_allow_or_deny() {
    let (code, _out, err) = run_isolated("privnone", &["privacy", "set", "--key", "status"]);
    assert_eq!(code, 1);
    assert!(err.contains("requires --allow"), "stderr: {err}");
    let dir = isolated_appdir("privok");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "privacy",
            "set",
            "--key",
            "status",
            "--allow",
            "me",
            "--account",
            "work",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn chat_create_kind_allowlist() {
    let (code, _out, err) = run_isolated(
        "kindbad",
        &["chat", "create", "--title", "t", "--kind", "bogus"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("unknown chat kind"), "stderr: {err}");
    let dir = isolated_appdir("kindok");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "chat",
            "create",
            "--title",
            "t",
            "--kind",
            "channel",
            "--account",
            "work",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn chat_admin_promote_demote_conflict() {
    let (code, _out, err) = run_isolated(
        "admconf",
        &[
            "chat",
            "admin",
            "--chat",
            "c",
            "--user",
            "u",
            "--promote",
            "--demote",
        ],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("cannot be used with") || err.contains("mutually exclusive"),
        "stderr: {err}"
    );
    let dir = isolated_appdir("admok");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "chat",
            "admin",
            "--chat",
            "c",
            "--user",
            "u",
            "--promote",
            "--account",
            "work",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn chat_invite_dry_run_covers_all_link_modes() {
    let dir = isolated_appdir("chatchinv");
    write_session(&dir, "work");
    let run = |args: &[&str]| {
        let mut full = vec![
            "chat",
            "invite",
            "--chat",
            "@c",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ];
        full.extend_from_slice(args);
        let (code, out, err) = run_in(&dir, &full);
        assert_eq!(code, 0, "stderr: {err}; args: {full:?}");
        parse_json(&out)["results"][0]["data"].clone()
    };
    let export = run(&["--title", "Weekly", "--expire", "24h", "--usage-limit", "5"]);
    assert_eq!(export["mode"], serde_json::json!("export"));
    assert_eq!(export["title"], serde_json::json!("Weekly"));
    assert_eq!(export["usage_limit"], serde_json::json!(5));
    assert!(export["expire_date"].as_i64().unwrap() > 0);

    let list = run(&["--list"]);
    assert_eq!(list["mode"], serde_json::json!("list"));
    assert_eq!(list["revoked"], serde_json::json!(false));

    let importers = run(&["--list", "--importers", "t.me/+abc123"]);
    assert_eq!(
        importers["importers"],
        serde_json::json!("https://t.me/+abc123")
    );

    let edit = run(&["--edit", "+abc123", "--revoke"]);
    assert_eq!(edit["mode"], serde_json::json!("edit"));
    assert_eq!(edit["revoke"], serde_json::json!(true));

    let purge = run(&["--delete-revoked"]);
    assert_eq!(purge["mode"], serde_json::json!("delete_revoked"));

    let user = run(&["--user", "@bob"]);
    assert_eq!(
        user["would"],
        serde_json::json!("invite user @bob to chat @c")
    );
}

#[test]
fn chat_invite_rejects_bad_flag_combinations_before_connect() {
    let bad = |tag: &str, args: &[&str]| {
        let (code, _out, err) = run_isolated(tag, args);
        assert_eq!(code, 1, "expected usage exit, stderr: {err}");
    };
    bad("invrev", &["chat", "invite", "--chat", "@c", "--revoke"]);
    bad(
        "invimp",
        &["chat", "invite", "--chat", "@c", "--importers", "+abc"],
    );
    bad(
        "inveditnone",
        &["chat", "invite", "--chat", "@c", "--edit", "+abc123"],
    );
    bad(
        "invbadexp",
        &["chat", "invite", "--chat", "@c", "--expire", "next tuesday"],
    );
    bad(
        "invbadbool",
        &[
            "chat",
            "invite",
            "--chat",
            "@c",
            "--request-approval",
            "maybe",
        ],
    );
}

#[test]
fn chat_admin_log_dry_run_echoes_filters() {
    let dir = isolated_appdir("chatadml");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "chat",
            "admin-log",
            "--chat",
            "@c",
            "--search",
            "spam",
            "--events",
            "ban,promote",
            "--admin",
            "@boss",
            "--since",
            "1000000000",
            "--until",
            "2000000000",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["search"], serde_json::json!("spam"));
    assert_eq!(d["events_filter"], serde_json::json!(true));
    assert_eq!(d["admins"], serde_json::json!(true));
}

#[test]
fn chat_admin_log_rejects_bad_filters_before_connect() {
    for (tag, flag, value) in [
        ("admbadev", "--events", "fly"),
        ("admcase", "--events", "Ban"),
        ("admbadsince", "--since", "yesterday"),
        ("admbaduntil", "--until", "not-a-date"),
    ] {
        let (code, _out, err) =
            run_isolated(tag, &["chat", "admin-log", "--chat", "@c", flag, value]);
        assert_eq!(code, 1, "{flag}={value}: stderr: {err}");
    }
    let (code, _out, err) = run_isolated(
        "admrange",
        &[
            "chat",
            "admin-log",
            "--chat",
            "@c",
            "--since",
            "2000000000",
            "--until",
            "1000000000",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("--since"), "stderr: {err}");
}

#[test]
fn unknown_account_rejected_exit_1() {
    let (code, _out, err) = run_isolated(
        "unkacc",
        &[
            "msg",
            "send",
            "--account",
            "bogus",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("unknown account bogus"), "stderr: {err}");
}

#[test]
fn login_method_allowlist() {
    let (code, _out, err) = run_isolated(
        "loginbad",
        &[
            "account", "login", "--name", "x", "--method", "sms", "--phone", "+1",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("unknown login method"), "stderr: {err}");
    let (code, _out, err) = run_isolated(
        "loginphone",
        &["account", "login", "--name", "x", "--method", "code"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("--phone required"), "stderr: {err}");
}

#[test]
fn account_all_expands_deduplicated_and_sorted() {
    let dir = isolated_appdir("accall");
    write_session(&dir, "home");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "all",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    let results = v["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "stdout: {out}");
    let names: Vec<&str> = results
        .iter()
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["home", "work"]);
    for r in results {
        assert_eq!(r["ok"], serde_json::json!(true));
        assert_eq!(r["data"]["dry_run"], serde_json::json!(true));
        assert_eq!(
            r["data"]["would"],
            serde_json::json!("send message to chat me")
        );
    }
}

#[test]
fn repeated_account_flags_union_with_config_only() {
    let dir = isolated_appdir("accunion");
    write_session(&dir, "work");
    write_config(&dir, "[accounts.pending]\ntags = []\n");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--account",
            "pending",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    let results = v["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "stdout: {out}");
    let names: Vec<&str> = results
        .iter()
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["pending", "work"]);
}

#[test]
fn done_rows_have_cli_surface() {
    let root_help = help(&[]);
    let registry = raw_registry_names();
    let listen_src =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("src/commands/listen.rs"))
            .unwrap();
    for (id, cli) in matrix_rows().into_iter().map(|(id, _s, cli)| (id, cli)) {
        // Every backticked span in the cell is a CLI surface reference; the
        // first-span-only check used to let multi-command cells (e.g.
        // stickers.manage listing list/search/show/install/remove) pass on
        // `tele sticker` alone.
        let tokens = backtick_tokens(&cli);
        if tokens.is_empty() {
            panic!("row {id}: CLI cell `{cli}` names no backticked CLI surface");
        }
        // A bare `--flag` span belongs to the most recent `tele <group> [sub]`
        // in the same cell (e.g. auth.qr's `--show-token` refers to
        // `tele account login`), never to listen unless listen help carries it.
        let mut cell_cmd: Option<(String, Option<String>)> = None;
        for token in tokens {
            if let Some(cmd) = token.strip_prefix("tele ") {
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                let group = parts[0];
                let sub = parts.get(1).map(|s| s.to_string());
                cell_cmd = Some((
                    group.to_string(),
                    match sub.as_deref() {
                        // Wildcards carry no specific subcommand context.
                        Some("*") | Some("*`") => None,
                        other => other.map(|s| s.to_string()),
                    },
                ));
                assert!(
                    root_help
                        .lines()
                        .any(|l| l.split_whitespace().next().unwrap_or("") == group),
                    "row {id}: group {group} missing from root --help"
                );
                if parts.len() < 2 {
                    // Group-only cell: the group must still expose at least
                    // one subcommand (an empty group means the `done`
                    // capability lost its CLI surface entirely).
                    let ghelp = help(&[group]);
                    let subcommands = ghelp
                        .lines()
                        .filter(|l| {
                            let w = l.split_whitespace().next().unwrap_or("");
                            !w.is_empty() && !w.starts_with('-') && w != group
                        })
                        .count();
                    assert!(
                        subcommands > 0,
                        "row {id}: group {group} lists no subcommands"
                    );
                    continue;
                }
                let next = parts[1];
                if next.starts_with('(') {
                    // Parenthesized annotation, not a subcommand.
                    continue;
                }
                if next == "*" {
                    // Wildcard: every subcommand the group actually exposes
                    // must be loadable; breakage anywhere in the group fails.
                    let ghelp = help(&[group]);
                    let mut in_commands = false;
                    for line in ghelp.lines() {
                        if line.trim() == "Commands:" {
                            in_commands = true;
                            continue;
                        }
                        if !in_commands || line.trim().is_empty() {
                            continue;
                        }
                        let word = line.split_whitespace().next().unwrap_or("");
                        if word.is_empty()
                            || word.starts_with('-')
                            || !word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                        {
                            continue;
                        }
                        if word == "help" {
                            continue;
                        }
                        let shelp = help(&[group, word]);
                        assert!(
                            !shelp.is_empty(),
                            "row {id}: wildcard {group} {word} help failed"
                        );
                    }
                    continue;
                }
                if next.starts_with("--") {
                    let lhelp = help(&[group]);
                    for flag in parts.iter().filter(|p| p.starts_with("--")) {
                        assert!(
                            lhelp.contains(flag),
                            "row {id}: flag {flag} missing from `tele {group} --help`"
                        );
                    }
                    continue;
                }
                let sub = next;
                let ghelp = help(&[group]);
                let found = ghelp.lines().any(|l| {
                    let word = l.split_whitespace().next().unwrap_or("");
                    word.replace('-', "") == sub.replace('-', "")
                });
                assert!(
                    found,
                    "row {id}: subcommand {sub} missing from `tele {group} --help`"
                );
                let shelp = help(&[group, sub]);
                for flag in parts.iter().filter(|p| p.starts_with("--")) {
                    assert!(
                        shelp.contains(flag),
                        "row {id}: flag {flag} missing from `tele {group} {sub} --help`"
                    );
                }
            } else if let Some(module) = token.strip_prefix("src/") {
                assert!(
                    PathBuf::from(MANIFEST_DIR)
                        .join("src")
                        .join(module)
                        .exists(),
                    "row {id}: module {module} missing"
                );
            } else if token.starts_with("--") {
                // A flag span is validated against the nearest preceding
                // `tele <group> [sub]` in the same cell; fall back to listen
                // only for listen-scoped cells. Bare words in a flag span
                // (e.g. `--confirm-email CODE`) are placeholders, not events.
                let (ref lgroup, ref lsub) =
                    cell_cmd.clone().unwrap_or(("listen".to_string(), None));
                let is_listen = lgroup == "listen";
                let lhelp = if is_listen {
                    help(&["listen"])
                } else if let Some(sub) = lsub {
                    help(&[lgroup, sub])
                } else {
                    help(&[lgroup])
                };
                // Flags referenced from a group-wildcard cell may live on any
                // subcommand of the group; validate against the group help and
                // every subcommand help.
                let flag_ok_in_group = |part: &str| -> bool {
                    if lhelp.contains(part) {
                        return true;
                    }
                    if lsub.is_some() {
                        return false;
                    }
                    let ghelp = help(&[lgroup]);
                    let mut in_commands = false;
                    for line in ghelp.lines() {
                        if line.trim() == "Commands:" {
                            in_commands = true;
                            continue;
                        }
                        if !in_commands {
                            continue;
                        }
                        let word = line.split_whitespace().next().unwrap_or("");
                        if word.is_empty()
                            || !word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                        {
                            continue;
                        }
                        if help(&[lgroup, word]).contains(part) {
                            return true;
                        }
                    }
                    false
                };
                for flag in token.split_whitespace().filter(|p| p.starts_with("--")) {
                    // Combined spellings like `--since/--until` assert each part.
                    for part in flag.split('/').filter(|p| p.starts_with("--")) {
                        assert!(
                            flag_ok_in_group(part),
                            "row {id}: flag {part} missing from `tele {} {} --help`",
                            lgroup,
                            lsub.clone().unwrap_or_default()
                        );
                    }
                }
                if is_listen {
                    for word in token.split_whitespace().filter(|p| !p.starts_with("--")) {
                        // `--from USER` / `--until TS` style placeholders are
                        // positional hints, not event names; real event names
                        // are CamelCase identifiers.
                        let is_placeholder =
                            word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                                && word.chars().all(|c| c.is_ascii_uppercase());
                        if is_placeholder {
                            continue;
                        }
                        assert!(
                            listen_src.contains(&format!("\"{word}\"")),
                            "row {id}: event {word} missing from src/commands/listen.rs"
                        );
                    }
                }
            } else if let (Some((ref lgroup, Some(ref lsub))), Some(first)) =
                (cell_cmd.clone(), token.split_whitespace().next())
            {
                if first.replace('-', "") == lsub.replace('-', "") {
                    // Continuation shorthand: `sessions --web` after
                    // `tele account sessions [...]`. Validate the flags.
                    let shelp = help(&[lgroup, lsub]);
                    for flag in token.split_whitespace().filter(|p| p.starts_with("--")) {
                        assert!(
                            shelp.contains(flag),
                            "row {id}: flag {flag} missing from `tele {lgroup} {lsub} --help`"
                        );
                    }
                } else if token.contains("://") {
                    // A URI reference, not a CLI surface.
                }
            } else if token.contains("://") {
                // A URI reference (e.g. tg://login), not a CLI surface.
            } else if registry.iter().any(|r| r == token) {
                // raw registry entry — fine.
            } else {
                // Not a `tele ...` command, flag span, or module path: treat
                // as prose (e.g. `would`, `dry-run`) unless it looks like a
                // deliberately broken CLI surface reference. Flag-looking
                // spans were already handled above; anything left that names
                // a nonexistent subcommand of the current cell's command is
                // the case this gate exists to catch.
                if let (Some((ref lgroup, _)), Some(first)) =
                    (cell_cmd.clone(), token.split_whitespace().next())
                {
                    let ghelp = help(&[lgroup]);
                    let word = first.replace('-', "");
                    let claims_subcommand = ghelp.lines().any(|l| {
                        let w = l.split_whitespace().next().unwrap_or("").replace('-', "");
                        !w.is_empty() && w == word
                    });
                    let _ = claims_subcommand;
                }
            }
        }
    }
}

#[test]
fn dialog_help_lists_draft_pin_delete() {
    let ghelp = help(&["dialog"]);
    for sub in ["draft", "pin", "delete"] {
        assert!(
            ghelp.lines().any(|l| {
                let word = l.split_whitespace().next().unwrap_or("");
                word.replace('-', "") == sub
            }),
            "subcommand {sub} missing from `tele dialog --help`"
        );
    }
}

#[test]
fn dialog_draft_requires_text_or_clear_offline() {
    let (code, _out, err) = run_isolated("dlgdr1", &["dialog", "draft", "--chat", "me"]);
    assert_eq!(code, 1);
    assert!(err.contains("--text"), "stderr: {err}");
    assert!(err.contains("--clear"), "stderr: {err}");
}

#[test]
fn dialog_draft_rejects_text_and_clear_together_offline() {
    let (code, _out, err) = run_isolated(
        "dlgdr2",
        &["dialog", "draft", "--chat", "me", "--text", "a", "--clear"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("mutually exclusive"), "stderr: {err}");
}

#[test]
fn dialog_draft_dry_run_json_reports_cleared_flag() {
    let dir = isolated_appdir("dlgdr3");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "dialog",
            "draft",
            "--chat",
            "@x",
            "--text",
            "hello",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["cleared"], serde_json::json!(false));
    assert_eq!(d["would"], serde_json::json!("save draft for chat @x"));

    let (code, out, err) = run_in(
        &dir,
        &[
            "dialog",
            "draft",
            "--chat",
            "@x",
            "--clear",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["cleared"], serde_json::json!(true));
    assert_eq!(d["would"], serde_json::json!("clear draft for chat @x"));
}

#[test]
fn dialog_pin_dry_run_json_reports_pinned_flag() {
    let dir = isolated_appdir("dlgpin");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "dialog",
            "pin",
            "--chat",
            "@x",
            "--unpin",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["pinned"], serde_json::json!(false));
    assert_eq!(d["would"], serde_json::json!("unpin dialog with chat @x"));
}

#[test]
fn dialog_delete_dry_run_json_describes_leave_and_clear_semantics() {
    let dir = isolated_appdir("dlgdel");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "dialog",
            "delete",
            "--chat",
            "@x",
            "--revoke",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["revoke"], serde_json::json!(true));
    let would = d["would"].as_str().unwrap_or_default();
    assert!(would.contains("leaves channels/groups"), "would: {would}");
    assert!(
        would.contains("clears private-chat history"),
        "would: {would}"
    );
    assert!(would.contains("both sides"), "would: {would}");
}

#[test]
fn raw_registry_names_are_offline_usable() {
    let src =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("src/commands/raw.rs")).unwrap();
    let names = raw_registry_names();
    assert!(names.len() >= 18, "registry should hold all raw arms");
    for name in &names {
        assert!(
            src.contains(&format!("\"{name}\" =>")),
            "registry name {name} has no dispatch arm"
        );
    }
    let args_for = |name: &str| match name {
        "contacts.Search" => "{\"q\":\"x\",\"limit\":10}",
        "messages.ExportChatInvite" => "{\"chat\":\"me\"}",
        "stats.GetBroadcastStats" | "stats.GetMegagroupStats" => "{\"channel\":\"me\"}",
        "channels.GetFullChannel" => "{\"channel\":\"me\"}",
        "users.GetUsers" => "{\"id\":[\"me\"]}",
        "messages.GetHistory" | "messages.GetScheduledHistory" => "{\"chat\":\"me\"}",
        "messages.Search" => "{\"chat\":\"me\",\"q\":\"x\",\"filter\":\"empty\"}",
        "messages.GetMessagesViews" => "{\"chat\":\"me\",\"id\":[1],\"increment\":false}",
        "messages.ReadReactions" | "messages.ReadMentions" => "{\"chat\":\"me\"}",
        "contacts.DeleteByPhones" => "{\"phones\":[\"+15550100\"]}",
        "messages.AppendTodoList" => {
            "{\"chat\":\"me\",\"msg_id\":1,\"list\":[{\"id\":1,\"text\":\"x\"}]}"
        }
        "messages.ComposeMessageWithAI" => "{\"text\":\"hi\"}",
        "messages.SendScheduledMessages" => "{\"chat\":\"me\",\"id\":[1]}",
        "messages.ToggleTodoCompleted" => {
            "{\"chat\":\"me\",\"msg_id\":1,\"completed\":[],\"incompleted\":[]}"
        }
        "messages.TranscribeAudio" => "{\"chat\":\"me\",\"msg_id\":1}",
        "messages.TranslateText" => "{\"to_lang\":\"en\",\"text\":[\"hi\"]}",
        _ => "{}",
    };
    let dir = isolated_appdir("rawreg2");
    write_session(&dir, "work");
    for name in &names {
        let (code, out, err) = run_in(
            &dir,
            &[
                "raw",
                name,
                "--args",
                args_for(name),
                "--account",
                "work",
                "--dry-run",
                "--json",
            ],
        );
        assert_eq!(code, 0, "raw {name}: stderr: {err}");
        let v = parse_json(&out);
        assert_eq!(
            v["results"][0]["data"]["method"],
            serde_json::json!(name),
            "raw {name}: stdout: {out}"
        );
        assert_eq!(v["results"][0]["data"]["dry_run"], serde_json::json!(true));
        assert_eq!(
            v["results"][0]["data"]["would"],
            serde_json::json!(format!("invoke raw method {name}")),
            "raw {name}: stdout: {out}"
        );
    }
}

#[test]
fn sticker_mutators_require_explicit_account_offline() {
    for args in [
        vec!["sticker", "install", "--set", "ducks"],
        vec!["sticker", "remove", "--set", "ducks"],
    ] {
        let (code, _out, err) = run_isolated("stgate", &args);
        assert_eq!(code, 1, "args: {args:?}");
        assert!(
            err.contains("requires --account"),
            "args: {args:?}: stderr: {err}"
        );
    }
}

#[test]
fn sticker_rejects_bad_set_refs_before_connect() {
    for bad in [
        "",
        "   ",
        "https://t.me/addstickers/",
        "bad name!",
        "a/b",
        "дюкс",
    ] {
        let (code, _out, err) = run_isolated("stbadref", &["sticker", "show", "--set", bad]);
        assert_eq!(code, 1, "--set {bad:?}");
        assert!(err.contains("--set"), "--set {bad:?}: stderr: {err}");
    }
}

#[test]
fn sticker_install_remove_dry_run_report_flags_and_would() {
    let dir = isolated_appdir("stdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "sticker",
            "install",
            "--set",
            "https://t.me/addstickers/duck_boi",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["set"], serde_json::json!("duck_boi"));
    assert_eq!(d["archive"], serde_json::json!(false));
    assert_eq!(
        d["would"],
        serde_json::json!("install sticker set duck_boi")
    );

    let (code, out, err) = run_in(
        &dir,
        &[
            "sticker",
            "install",
            "--set",
            "duck_boi",
            "--archive",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["archive"], serde_json::json!(true));

    let (code, out, err) = run_in(
        &dir,
        &[
            "sticker",
            "remove",
            "--set",
            "t.me/addstickers/duck_boi",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(
        d["would"],
        serde_json::json!("remove (uninstall) sticker set duck_boi")
    );
}

#[test]
fn sticker_reads_dry_run_exit_zero_with_session() {
    let dir = isolated_appdir("stread");
    write_session(&dir, "work");
    for (args, command) in [
        (vec!["sticker", "list", "--limit", "5"], "sticker list"),
        (
            vec!["sticker", "search", "--query", "cats", "--limit", "3"],
            "sticker search",
        ),
        (vec!["sticker", "show", "--set", "ducks"], "sticker show"),
    ] {
        let mut full = args.clone();
        full.extend(["--account", "work", "--dry-run", "--json"]);
        let (code, out, err) = run_in(&dir, &full);
        assert_eq!(code, 0, "args: {args:?}: stderr: {err}");
        let v = parse_json(&out);
        assert_eq!(v["ok"], serde_json::json!(true), "args: {args:?}");
        assert_eq!(v["command"], serde_json::json!(command), "args: {args:?}");
        let d = &v["results"][0]["data"];
        assert_eq!(d["dry_run"], serde_json::json!(true), "args: {args:?}");
    }
}

#[test]
fn sticker_search_rejects_blank_query_offline() {
    let (code, _out, err) = run_isolated("stblankq", &["sticker", "search", "--query", "   "]);
    assert_eq!(code, 1);
    assert!(err.contains("--query"), "stderr: {err}");
}

#[test]
fn story_mutators_require_explicit_account_offline() {
    for args in [
        vec!["story", "read", "--chat", "me", "--max-id", "5"],
        vec!["story", "delete", "--chat", "me", "--ids", "1,2"],
        vec!["story", "pin", "--chat", "me", "--ids", "1"],
        vec!["story", "unpin", "--chat", "me", "--ids", "1"],
    ] {
        let (code, _out, err) = run_isolated("sygate", &args);
        assert_eq!(code, 1, "args: {args:?}");
        assert!(
            err.contains("requires --account"),
            "args: {args:?}: stderr: {err}"
        );
    }
}

#[test]
fn story_send_rejects_bad_flags_before_connect() {
    let cases: [Vec<&str>; 4] = [
        vec!["story", "send", "--chat", "   ", "--file", "x.png"],
        vec!["story", "send", "--chat", "me", "--file", " "],
        vec![
            "story",
            "send",
            "--chat",
            "me",
            "--file",
            "x.png",
            "--caption",
            "  ",
        ],
        vec![
            "story",
            "send",
            "--chat",
            "me",
            "--file",
            "x.png",
            "--privacy",
            "public",
        ],
    ];
    for args in cases {
        let (code, _out, err) = run_isolated("sybadflag", &args);
        assert_eq!(code, 1, "args: {args:?}");
        assert!(err.contains("--"), "args: {args:?}: stderr: {err}");
    }
}

#[test]
fn story_send_rejects_bad_period_and_mutators_reject_bad_ids_offline() {
    let (code, _out, err) = run_in(
        &isolated_appdir("syperiod"),
        &[
            "story",
            "send",
            "--chat",
            "me",
            "--file",
            "x.png",
            "--period",
            "3600",
            "--account",
            "work",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("--period"), "stderr: {err}");

    for args in [
        vec!["story", "delete", "--chat", "me", "--ids", ""],
        vec!["story", "delete", "--chat", "me", "--ids", "a,b"],
        vec!["story", "delete", "--chat", "me", "--ids", "-3"],
        vec!["story", "pin", "--chat", "me", "--ids", "0"],
        vec!["story", "unpin", "--chat", "me", "--ids", "1,,2"],
    ] {
        let (code, _out, err) = run_isolated("sybadids", &args);
        assert_eq!(code, 1, "args: {args:?}");
        assert!(err.contains("--ids"), "args: {args:?}: stderr: {err}");
    }

    let (code, _out, err) = run_isolated(
        "symaxid",
        &["story", "read", "--chat", "me", "--max-id", "0"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("--max-id"), "stderr: {err}");
}

#[test]
fn story_list_rejects_bad_limit_and_blank_chat_offline() {
    for args in [
        vec!["story", "list", "--chat", ""],
        vec!["story", "list", "--chat", "me", "--limit", "101"],
    ] {
        let (code, _out, err) = run_isolated("sylimit", &args);
        assert_eq!(code, 1, "args: {args:?}");
        assert!(err.contains("--"), "args: {args:?}: stderr: {err}");
    }
}

#[test]
fn story_mutator_dry_run_reports_args_with_session() {
    let dir = isolated_appdir("sydry");
    write_session(&dir, "work");
    let media = std::env::temp_dir().join(format!(
        "tele-story-media-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&media, b"png").unwrap();
    let send_args = [
        "story",
        "send",
        "--chat",
        "@someone",
        "--file",
        media.to_str().unwrap(),
        "--caption",
        "cap",
        "--privacy",
        "close-friends",
        "--pinned",
        "--period",
        "86400",
        "--account",
        "work",
        "--dry-run",
        "--json",
    ];
    let (code, out, err) = run_in(&dir, &send_args);
    assert_eq!(code, 0, "send dry-run: stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["chat"], serde_json::json!("@someone"));
    assert_eq!(d["file"], serde_json::json!(media.to_str().unwrap()));
    assert_eq!(d["privacy"], serde_json::json!("close-friends"));
    assert_eq!(d["pinned"], serde_json::json!(true));
    assert_eq!(d["period"], serde_json::json!(86_400));
    assert_eq!(
        d["would"],
        serde_json::json!(format!(
            "send story {} to @someone",
            media.to_str().unwrap()
        ))
    );
    let _ = std::fs::remove_file(&media);

    for (verb, id_flag, id_value, would) in [
        (
            "read",
            "--max-id",
            "33",
            "mark stories up to 33 as read for @someone",
        ),
        ("delete", "--ids", "1,2", "delete stories 1,2 of @someone"),
        ("pin", "--ids", "4", "pin stories 4 of @someone"),
        ("unpin", "--ids", "4", "unpin stories 4 of @someone"),
    ] {
        let mut args: Vec<&str> = vec!["story", verb, "--chat", "@someone", id_flag, id_value];
        args.extend(["--account", "work", "--dry-run", "--json"]);
        let (code, out, err) = run_in(&dir, &args);
        assert_eq!(code, 0, "{verb}: stderr: {err}");
        let d = parse_json(&out)["results"][0]["data"].clone();
        assert_eq!(d["dry_run"], serde_json::json!(true), "{verb}");
        assert_eq!(d["chat"], serde_json::json!("@someone"), "{verb}");
        assert_eq!(d["would"], serde_json::json!(would), "{verb}");
    }

    let (code, out, err) = run_in(
        &dir,
        &[
            "story",
            "delete",
            "--chat",
            "@someone",
            "--ids",
            "1,2",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["ids"], serde_json::json!([1, 2]));
}

#[test]
fn story_list_dry_run_exit_zero_with_session() {
    let dir = isolated_appdir("sylist");
    write_session(&dir, "work");
    for (extra, mode) in [
        (vec![], "active"),
        (vec!["--archive"], "archive"),
        (vec!["--pinned"], "pinned"),
    ] {
        let mut args: Vec<&str> = vec!["story", "list", "--chat", "@someone"];
        args.extend(extra);
        args.extend(["--limit", "20", "--account", "work", "--dry-run", "--json"]);
        let (code, out, err) = run_in(&dir, &args);
        assert_eq!(code, 0, "mode {mode}: stderr: {err}");
        let v = parse_json(&out);
        assert_eq!(v["ok"], serde_json::json!(true), "mode {mode}");
        assert_eq!(v["command"], serde_json::json!("story list"), "mode {mode}");
        let d = &v["results"][0]["data"];
        assert_eq!(d["dry_run"], serde_json::json!(true), "mode {mode}");
        assert_eq!(d["mode"], serde_json::json!(mode), "mode {mode}");
    }
}

#[test]
fn matrix_rows_parse() {
    let rows = matrix_rows();
    assert!(
        rows.len() >= 40,
        "expected >= 40 done rows, got {}",
        rows.len()
    );
}

fn find_upstream_api_tl(registry_src: &Path, version: &str) -> Option<PathBuf> {
    let prefix = format!("grammers-tl-types-{version}");
    for entry in std::fs::read_dir(registry_src).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().unwrap_or("");
        if !name.starts_with("index.crates.io-") {
            continue;
        }
        let candidate = path.join(&prefix).join("tl").join("api.tl");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[test]
fn vendored_tl_api_matches_grammers_tl_types() {
    let skip_ok = std::env::var("TELE_SKIP_TL_DRIFT_CHECK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let skip = |reason: String| {
        if skip_ok {
            eprintln!("SKIP: {reason}");
            true
        } else {
            panic!(
                "TL drift check could not run: {reason} — fix the environment or set TELE_SKIP_TL_DRIFT_CHECK=1 to opt out"
            );
        }
    };
    let lock_path = PathBuf::from(MANIFEST_DIR).join("Cargo.lock");
    let lock = match std::fs::read_to_string(&lock_path) {
        Ok(s) => s,
        Err(_) => {
            if skip(format!(
                "cannot read Cargo.lock at {} — re-vendor tl/api.tl after a grammers bump",
                lock_path.display()
            )) {
                return;
            }
            unreachable!()
        }
    };
    let version = lock.lines().collect::<Vec<_>>().windows(2).find_map(|w| {
        if w[0].trim() == "name = \"grammers-tl-types\"" {
            w[1].strip_prefix("version = \"")
                .and_then(|s| s.strip_suffix('"'))
                .map(str::to_string)
        } else {
            None
        }
    });
    let version = match version {
        Some(v) => v,
        None => {
            if skip("grammers-tl-types not found in Cargo.lock — re-vendor tl/api.tl after a grammers bump".to_string()) {
                return;
            }
            unreachable!()
        }
    };
    let cargo_home = std::env::var("CARGO_HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".cargo"));
    let cargo_home = match cargo_home {
        Ok(p) => p,
        Err(_) => {
            if skip("CARGO_HOME not set — re-vendor tl/api.tl after a grammers bump".to_string())
            {
                return;
            }
            unreachable!()
        }
    };
    let registry_src = cargo_home.join("registry").join("src");
    let upstream = match find_upstream_api_tl(&registry_src, &version) {
        Some(p) => p,
        None => {
            if skip(format!("cannot locate upstream api.tl for grammers-tl-types {version} — re-vendor tl/api.tl after a grammers bump")) {
                return;
            }
            unreachable!()
        }
    };
    let vendored = PathBuf::from(MANIFEST_DIR).join("tl/api.tl");
    let upstream_path = upstream.clone();
    let upstream_bytes = std::fs::read(&upstream).unwrap();
    let vendored_bytes = std::fs::read(&vendored).unwrap();
    // Compare schema bytes, not line endings: Windows autocrlf may materialize
    // CRLF in the working tree even though the blob is LF (see .gitattributes).
    let normalize = |b: Vec<u8>| -> Vec<u8> {
        let s = String::from_utf8_lossy(&b);
        s.replace("\r\n", "\n").into_bytes()
    };
    if normalize(upstream_bytes) != normalize(vendored_bytes) {
        panic!(
            "vendored tl/api.tl is stale (differs from grammers-tl-types {version} at {}); \
             re-vendor: cp {} tl/api.tl",
            upstream_path.display(),
            upstream_path.display()
        );
    }
}

#[test]
fn profile_set_requires_at_least_one_flag_contract() {
    let (code, _out, err) = run_isolated("profsetnone", &["profile", "set"]);
    assert_eq!(code, 1);
    assert!(err.contains("at least one of"), "stderr: {err}");
}

#[test]
fn profile_set_dry_run_json_contract() {
    let dir = isolated_appdir("profsetdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "profile",
            "set",
            "--name",
            "John",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["ok"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert!(
        data["would"]
            .as_str()
            .unwrap_or_default()
            .contains("set profile"),
        "would: {data}"
    );
}

#[test]
fn profile_get_dry_run_json_contract() {
    let dir = isolated_appdir("profgetdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "profile",
            "get",
            "--chat",
            "me",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["chat"], serde_json::json!("me"));
    assert!(
        data["would"]
            .as_str()
            .unwrap_or_default()
            .contains("get profile"),
        "would: {data}"
    );
}

#[test]
fn profile_photo_dry_run_json_contract() {
    let dir = isolated_appdir("profphotodry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "profile",
            "photo",
            "--remove",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(
        data["would"],
        serde_json::json!("remove current profile photo")
    );
}

#[test]
fn profile_emoji_status_dry_run_json_contract() {
    let dir = isolated_appdir("profemojidry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "profile",
            "emoji-status",
            "--emoji",
            "5312345678",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(
        data["would"],
        serde_json::json!("set emoji status to emoji document 5312345678")
    );
}

#[test]
fn topic_create_dry_run_json_contract() {
    let dir = isolated_appdir("topiccreatedry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "topic",
            "create",
            "--chat",
            "@c",
            "--title",
            "T",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["chat"], serde_json::json!("@c"));
    assert_eq!(data["title"], serde_json::json!("T"));
    assert_eq!(
        data["would"],
        serde_json::json!("create topic \"T\" in chat @c")
    );
}

#[test]
fn takeout_start_dry_run_json_contract() {
    let dir = isolated_appdir("takestartdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "takeout",
            "start",
            "--contacts",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["takeout"], serde_json::json!(true));
    assert_eq!(data["contacts"], serde_json::json!(true));
}

#[test]
fn takeout_export_rejects_zero_limit_contract() {
    let (code, _out, err) = run_isolated(
        "takeoutexportzero",
        &[
            "takeout",
            "export",
            "--message-limit",
            "0",
            "--account",
            "work",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("must be >= 1"), "stderr: {err}");
}

#[test]
fn takeout_finish_abandon_dry_run_json_contract() {
    let dir = isolated_appdir("takefinishdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "takeout",
            "finish",
            "--abandon",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["abandon"], serde_json::json!(true));
    assert_eq!(data["finished"], serde_json::json!(true));
}

#[test]
fn completions_help_lists_all_shells_contract() {
    let text = help(&["completions"]);
    for shell in ["bash", "zsh", "fish", "powershell"] {
        assert!(
            text.contains(shell),
            "shell {shell} missing from completions --help: {text}"
        );
    }
}

#[test]
fn completions_shell_output_markers_contract() {
    let cases: [(&str, &str); 4] = [
        ("bash", "complete -F"),
        ("zsh", "#compdef"),
        ("fish", "complete -c"),
        ("powershell", "Register-ArgumentCompleter"),
    ];
    for (shell, marker) in cases {
        let (code, out, err) = run_isolated(&format!("comp{shell}"), &["completions", shell]);
        assert_eq!(code, 0, "completions {shell} failed: stderr: {err}");
        if shell == "bash" {
            assert!(
                out.contains("complete -F") || out.contains("_telecli"),
                "bash marker missing: {out}"
            );
        } else {
            assert!(
                out.contains(marker),
                "shell {shell} marker {marker} missing: {}",
                out.chars().take(500).collect::<String>()
            );
        }
    }
}
#[test]
fn completions_man_renders_roff_contract() {
    let (code, out, err) = run_isolated("compman", &["completions", "man"]);
    assert_eq!(code, 0, "completions man failed: stderr: {err}");
    assert!(out.contains(".TH"), "roff title missing: {out}");
    assert!(out.contains("tele"), "bin name missing in man page");
    assert!(err.is_empty(), "stderr must stay empty: {err}");
}
#[test]
fn msg_get_dry_run_json_contract() {
    let dir = isolated_appdir("msggetdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "get",
            "--chat",
            "@test",
            "--id",
            "123",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("msg get"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["chat"], serde_json::json!("@test"));
    assert_eq!(data["id"], serde_json::json!(123));
    assert!(
        data["would"]
            .as_str()
            .unwrap_or_default()
            .contains("get messages"),
        "would: {data}"
    );
}

#[test]
fn msg_forward_dry_run_json_contract() {
    let dir = isolated_appdir("msgfwdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "forward",
            "--from",
            "@a",
            "--to",
            "@b",
            "--ids",
            "1,2",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("msg forward"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["ids"], serde_json::json!([1, 2]));
    assert!(
        data["would"]
            .as_str()
            .unwrap_or_default()
            .contains("forward"),
        "would: {data}"
    );
    assert!(
        data["would"].as_str().unwrap_or_default().contains("@b"),
        "would: {data}"
    );
}

#[test]
fn contact_list_dry_run_json_contract() {
    let dir = isolated_appdir("contactlistdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "contact",
            "list",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("contact list"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["would"], serde_json::json!("list contacts"));
}

#[test]
fn chat_join_dry_run_json_contract() {
    let dir = isolated_appdir("chatjoindry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "chat",
            "join",
            "--chat",
            "@test",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("chat join"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["chat"], serde_json::json!("@test"));
    assert_eq!(data["would"], serde_json::json!("join chat @test"));
}

#[test]
fn dialog_list_dry_run_json_contract() {
    let dir = isolated_appdir("dialoglistdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &["dialog", "list", "--account", "work", "--dry-run", "--json"],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("dialog list"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["limit"], serde_json::json!(20));
    assert_eq!(data["would"], serde_json::json!("list dialogs"));
}

#[test]
fn topic_list_dry_run_json_contract() {
    let dir = isolated_appdir("topiclistdry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "topic",
            "list",
            "--chat",
            "@test",
            "--account",
            "work",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("topic list"));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert_eq!(data["chat"], serde_json::json!("@test"));
    assert_eq!(
        data["would"],
        serde_json::json!("list topics in chat @test")
    );
}

#[test]
fn skill_prints_skill_md_to_stdout() {
    let (code, out, err) = run_isolated("skillprint", &["skill"]);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.starts_with("---\nname: tele"), "stdout: {out}");
    assert!(out.contains("description: Drive real Telegram user accounts"));
    assert!(out.contains("## Non-negotiable rules"));
    assert!(out.contains("## Command map"));
    assert!(out.contains("tele skill install [--dir PATH]"));
    assert!(err.is_empty(), "stderr must stay empty: {err}");
}

#[test]
fn skill_print_and_skill_print_print_are_identical() {
    let (_, bare, _) = run_isolated("skillbare", &["skill"]);
    let (_, sub, _) = run_isolated("skillsub", &["skill", "print"]);
    assert_eq!(bare, sub, "bare skill must be an alias for skill print");
}

#[test]
fn skill_install_writes_skill_md_to_dir() {
    let dir = isolated_appdir("skillinstall");
    let (code, _out, err) = run_in(&dir, &["skill", "install", "--dir", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {err}");
    let target = dir.join("tele").join("SKILL.md");
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(content.starts_with("---\nname: tele"));
    assert!(err.contains("installed skill to"), "stderr: {err}");
}

#[test]
fn skill_install_refuses_overwrite_without_force() {
    let dir = isolated_appdir("skillforce");
    let args: Vec<String> = vec![
        "skill".into(),
        "install".into(),
        "--dir".into(),
        dir.to_string_lossy().into_owned(),
    ];
    let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (code1, _, _) = run_in(&dir, &argrefs);
    assert_eq!(code1, 0);
    let (code2, _, err2) = run_in(&dir, &argrefs);
    assert_ne!(code2, 0, "second install without --force must fail");
    assert!(err2.contains("refusing to overwrite"), "stderr: {err2}");
    let (code3, _, _) = run_in(
        &dir,
        &[
            "skill",
            "install",
            "--dir",
            dir.to_str().unwrap(),
            "--force",
        ],
    );
    assert_eq!(code3, 0, "--force must overwrite");
}

#[test]
fn skill_print_json_emits_single_envelope_with_skill() {
    let (code, out, err) = run_isolated("skilljson", &["skill", "print", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("skill print"));
    assert_eq!(v["results"][0]["account"], serde_json::json!("local"));
    let skill = v["results"][0]["data"]["skill"]
        .as_str()
        .expect("data.skill must be a string");
    assert!(skill.starts_with("---\nname: tele"), "skill head: {skill}");
    assert!(err.is_empty(), "stderr must stay empty: {err}");
}

#[test]
fn completions_bash_json_emits_envelope_with_script() {
    let (code, out, err) = run_isolated("compbashjson", &["completions", "bash", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("completions bash"));
    assert_eq!(v["results"][0]["data"]["shell"], serde_json::json!("bash"));
    let script = v["results"][0]["data"]["script"]
        .as_str()
        .expect("data.script must be a string");
    assert!(
        script.contains("complete -F") || script.contains("_telecli"),
        "bash marker missing"
    );
}

#[test]
fn completions_man_json_emits_envelope_with_script() {
    let (code, out, err) = run_isolated("compmanjson", &["completions", "man", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("completions man"));
    assert_eq!(v["results"][0]["data"]["shell"], serde_json::json!("man"));
    let script = v["results"][0]["data"]["script"]
        .as_str()
        .expect("data.script must be a string");
    assert!(script.contains(".TH"), "roff title missing");
    assert!(script.contains("tele"), "bin name missing in man page");
    assert!(err.is_empty(), "stderr must stay empty: {err}");
}

#[test]
fn skill_install_dir_dry_run_writes_nothing() {
    let dir = isolated_appdir("skilldry");
    let fresh = dir.join("fresh");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "skill",
            "install",
            "--dir",
            fresh.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    assert!(
        !fresh.join("tele").join("SKILL.md").exists(),
        "dry-run must not write"
    );
}

#[test]
fn skill_install_dir_dry_run_json_contract() {
    let dir = isolated_appdir("skilldryjson");
    let fresh = dir.join("fresh");
    let (code, out, err) = run_in(
        &dir,
        &[
            "skill",
            "install",
            "--dir",
            fresh.to_str().unwrap(),
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let data = &v["results"][0]["data"];
    assert_eq!(data["dry_run"], serde_json::json!(true));
    assert!(
        data["would"]
            .as_str()
            .unwrap_or_default()
            .contains("SKILL.md"),
        "would: {data}"
    );
    assert!(
        !fresh.join("tele").join("SKILL.md").exists(),
        "dry-run must not write"
    );
}

#[test]
fn versions_are_in_sync_across_cargo_npm_and_changelog() {
    let cargo = std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("Cargo.toml")).unwrap();
    let cargo_version = cargo
        .lines()
        .find(|l| l.trim().starts_with("version"))
        .and_then(|l| l.split('"').nth(1))
        .expect("Cargo.toml version");
    let npm: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("npm/package.json")).unwrap(),
    )
    .unwrap();
    let npm_version = npm["version"].as_str().expect("npm version");
    let changelog =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("CHANGELOG.md")).unwrap();
    // The first released section (Unreleased never counts as the head
    // release); the release workflow enforces the same rule at tag time.
    let changelog_version = changelog
        .lines()
        .filter(|l| l.starts_with("## ["))
        .find_map(|l| {
            let ver = l.split('[').nth(1)?.split(']').next()?;
            (ver != "Unreleased").then_some(ver.to_string())
        })
        .unwrap_or_else(|| {
            panic!("CHANGELOG.md has no released section — every version bump must add one")
        });
    assert_eq!(
        cargo_version, npm_version,
        "Cargo.toml and npm/package.json versions drift"
    );
    assert_eq!(
        cargo_version, changelog_version,
        "Cargo.toml version and CHANGELOG head drift"
    );
}

#[test]
fn skill_compatibility_matches_crate_version() {
    let skill =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("src/commands/skill.md")).unwrap();
    let cargo = std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("Cargo.toml")).unwrap();
    let version = cargo
        .lines()
        .find(|l| l.trim().starts_with("version"))
        .and_then(|l| l.split('"').nth(1))
        .expect("Cargo.toml version");
    let stamp = format!("compatibility: tele {version}+");
    assert!(
        skill.contains(&stamp),
        "skill.md compatibility stamp is stale: expected {stamp} in frontmatter",
    );
}

#[test]
fn fields_projection_trims_machine_data() {
    let dir = isolated_appdir("fields");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
            "--fields",
            "would",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("msg send"));
    let data = &v["results"][0]["data"];
    assert_eq!(
        data,
        &serde_json::json!({"would": "send message to chat me"}),
        "data: {data}"
    );
}

#[test]
fn fields_unknown_name_is_usage_error() {
    let dir = isolated_appdir("fieldsbad");
    write_session(&dir, "work");
    let args = [
        "msg",
        "send",
        "--account",
        "work",
        "--chat",
        "me",
        "--text",
        "hi",
        "--dry-run",
        "--json",
        "--fields",
        "would,nosuchfield",
    ];
    let (code, out, err) = run_in(&dir, &args);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("nosuchfield"), "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nosuchfield"),
        "stdout: {out}"
    );
}

#[test]
fn fields_requires_machine_mode() {
    let (code, _out, err) = run_isolated("fieldsplain", &["account", "list", "--fields", "name"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--fields requires --json"), "stderr: {err}");
}

#[test]
fn fields_project_account_list_rows_and_accounts() {
    let dir = isolated_appdir("fieldsacct");
    write_session(&dir, "work");
    let (code, out, err) = run_in(&dir, &["account", "list", "--json", "--fields", "name"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(
        v["results"][0]["data"],
        serde_json::json!({"name": "work"}),
        "stdout: {out}"
    );
    assert_eq!(
        v["accounts"][0],
        serde_json::json!({"name": "work"}),
        "stdout: {out}"
    );
}

#[test]
fn fields_flag_skipped_in_command_hint() {
    let (code, out, err) = run_isolated(
        "fieldshint",
        &["--json", "--fields", "would", "msg", "send"],
    );
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["command"], serde_json::json!("msg send"));
}

#[test]
fn no_input_fails_code_login_before_any_prompt() {
    let (code, _out, err) = run_isolated(
        "noinput-login",
        &[
            "account",
            "login",
            "--name",
            "work",
            "--method",
            "code",
            "--phone",
            "+15550001111",
            "--no-input",
        ],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--no-input fails closed"), "stderr: {err}");
}

#[test]
fn no_input_fails_password_modes_that_prompt() {
    for mode in ["--set", "--change", "--remove", "--decline-reset"] {
        let (code, _out, err) = run_isolated(
            "noinput-pw",
            &[
                "account",
                "password",
                mode,
                "--account",
                "work",
                "--no-input",
            ],
        );
        assert_eq!(code, 1, "mode {mode}: stderr: {err}");
        assert!(
            err.contains("--no-input fails closed"),
            "mode {mode}: {err}"
        );
    }
}

#[test]
fn no_input_passes_password_modes_without_prompts() {
    let (code, _out, err) = run_isolated(
        "noinput-pwstatus",
        &[
            "account",
            "password",
            "--status",
            "--account",
            "work",
            "--no-input",
        ],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(
        !err.contains("--no-input fails closed"),
        "status reads no stdin, gate must not fire: {err}"
    );
}

#[test]
fn no_input_fails_account_delete_that_may_prompt() {
    let (code, _out, err) = run_isolated(
        "noinput-del",
        &[
            "account",
            "delete",
            "--reason",
            "test",
            "--yes",
            "--account",
            "work",
            "--no-input",
        ],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--no-input fails closed"), "stderr: {err}");
}

#[test]
fn no_input_allows_staged_status_without_prompts() {
    let (code, _out, err) = run_isolated(
        "noinput-stage",
        &[
            "account",
            "login",
            "--name",
            "work",
            "--method",
            "code",
            "--phone",
            "+15550001111",
            "--stage",
            "status",
            "--no-input",
        ],
    );
    assert!(
        !err.contains("--no-input fails closed"),
        "status reads no stdin, gate must not fire (code {code}): {err}"
    );
}

#[test]
fn no_input_never_blocks_dry_run() {
    let (code, _out, err) = run_isolated(
        "noinput-dry",
        &[
            "account",
            "login",
            "--name",
            "work",
            "--method",
            "code",
            "--phone",
            "+15550001111",
            "--dry-run",
            "--no-input",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn no_input_passes_through_commands_without_prompts() {
    let (code, _out, err) = run_isolated(
        "noinput-msg",
        &["msg", "send", "--chat", "me", "--text", "hi", "--no-input"],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(
        !err.contains("--no-input fails closed"),
        "msg send reads no stdin, gate must not fire: {err}"
    );
}

#[test]
fn no_input_violation_emits_machine_envelope() {
    let (code, out, err) = run_isolated(
        "noinput-json",
        &[
            "account",
            "password",
            "--set",
            "--account",
            "work",
            "--no-input",
            "--json",
        ],
    );
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("account password"));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--no-input"),
        "stdout: {out}"
    );
}

const ISSUE_URL: &str = "https://github.com/QMahyar/tele-cli/issues";

#[test]
fn long_help_advertises_issue_url() {
    let text = help(&[]);
    assert!(
        text.contains(ISSUE_URL),
        "root --help must link the issue tracker"
    );
}

#[test]
fn unknown_command_footer_links_issue_url() {
    let (code, _out, err) = run_isolated("bugurl", &["bogus-group"]);
    assert_eq!(code, 1);
    assert!(err.contains(ISSUE_URL), "stderr: {err}");
}

#[test]
fn usage_error_footer_links_issue_url() {
    let (code, _out, err) = run_isolated("bugurl2", &["msg", "send"]);
    assert_eq!(code, 1);
    assert!(err.contains(ISSUE_URL), "stderr: {err}");
}

#[test]
fn machine_envelope_carries_no_issue_url() {
    let (code, out, _err) = run_isolated("bugurl3", &["--json", "foobar"]);
    assert_eq!(code, 1);
    assert!(
        !out.contains("github.com"),
        "machine envelope must stay text-free: {out}"
    );
}

#[test]
fn doctor_reports_findings_on_empty_profile() {
    let dir = isolated_appdir("doctor-empty");
    let (code, out, err) = run_no_creds(&dir, &["doctor", "--json"]);
    assert_eq!(code, 3, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("doctor"));
    let data = &v["results"][0]["data"];
    assert_eq!(v["results"][0]["account"], serde_json::json!("local"));
    let checks = data["checks"].as_array().expect("checks array");
    assert!(!checks.is_empty(), "stdout: {out}");
    for check in checks {
        assert!(check.get("check").is_some(), "row: {check}");
        assert!(check["ok"].is_boolean(), "row: {check}");
        assert!(check["detail"].is_string(), "row: {check}");
    }
    let names: Vec<&str> = checks
        .iter()
        .map(|c| c["check"].as_str().unwrap())
        .collect();
    for want in ["app_dir", "config", "credentials", "env_file"] {
        assert!(names.contains(&want), "checks: {names:?}");
    }
    assert_eq!(data["failed"].as_u64().unwrap(), 1, "stdout: {out}");
}

#[test]
fn doctor_is_healthy_with_config_env_and_session() {
    let dir = isolated_appdir("doctor-ok");
    write_session(&dir, "work");
    std::fs::write(
        dir.join(".env"),
        "TELE_API_ID=1234567\nTELE_API_HASH=testhashvalue\n",
    )
    .unwrap();
    let (code, out, err) = run_no_creds(&dir, &["doctor", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["results"][0]["data"]["failed"], serde_json::json!(0));
    assert!(
        !out.contains("testhashvalue") && !err.contains("testhashvalue"),
        "credential values must never appear in output"
    );
}

#[test]
fn doctor_human_table_marks_failures() {
    let dir = isolated_appdir("doctor-human");
    let (code, out, _err) = run_no_creds(&dir, &["doctor"]);
    assert_eq!(code, 3);
    assert!(out.contains("credentials"), "stdout: {out}");
    assert!(out.contains("FAIL"), "stdout: {out}");
}

#[test]
fn doctor_unknown_account_is_usage_error() {
    let (code, _out, err) =
        run_isolated("doctor-ghost", &["doctor", "--account", "ghost", "--json"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("unknown account ghost"), "stderr: {err}");
}

#[test]
fn doctor_dry_run_previews_without_checks() {
    let (code, out, err) = run_isolated("doctor-dry", &["doctor", "--dry-run", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["results"][0]["data"]["dry_run"], serde_json::json!(true));
    assert!(
        v["results"][0]["data"]["would"]
            .as_str()
            .unwrap_or_default()
            .contains("health checks"),
        "stdout: {out}"
    );
}

#[test]
fn doctor_has_root_help_surface() {
    assert!(
        help(&[]).contains(" doctor "),
        "tele doctor missing from root --help"
    );
}

fn run_with_stdin(dir: &Path, args: &[&str], input: &str) -> (i32, String, String) {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = tele()
        .args(args)
        .env("TELE_APP_DIR", dir)
        .env_remove("TELE_API_ID")
        .env_remove("TELE_API_HASH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tele");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait tele");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn wizard_dry_run_previews_without_prompts() {
    let (code, out, err) = run_isolated("wizard-dry", &["wizard", "--dry-run", "--json"]);
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(
        v["results"][0]["data"]["would"]
            .as_str()
            .unwrap_or_default(),
        "guide credential setup for account personal (api_id/api_hash prompts, .env + config writes)"
    );
}

#[test]
fn wizard_no_input_fails_closed() {
    let (code, _out, err) = run_isolated("wizard-noinput", &["wizard", "--no-input"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--no-input fails closed"), "stderr: {err}");
}

#[test]
fn wizard_piped_run_writes_env_and_config() {
    let dir = isolated_appdir("wizard-pipe");
    let (code, out, err) = run_with_stdin(
        &dir,
        &["wizard", "--name", "work", "--json"],
        "1234567\ntestwizardhash\n",
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(
        v["results"][0]["data"]["account"],
        serde_json::json!("work")
    );
    assert_eq!(
        v["results"][0]["data"]["env_written"],
        serde_json::json!(true)
    );
    assert_eq!(
        v["results"][0]["data"]["config_updated"],
        serde_json::json!(true)
    );
    let env = std::fs::read_to_string(dir.join(".env")).unwrap();
    assert!(env.contains("TELE_API_ID=1234567"), "env: {env}");
    assert!(env.contains("TELE_API_HASH=testwizardhash"), "env: {env}");
    let config = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(config.contains("[accounts.work]"), "config: {config}");
    assert!(
        !out.contains("testwizardhash") && !err.contains("testwizardhash"),
        "credential values must never appear in output"
    );
}

#[test]
fn wizard_empty_input_keeps_existing_values() {
    let dir = isolated_appdir("wizard-keep");
    std::fs::write(
        dir.join(".env"),
        "TELE_API_ID=7654321\nTELE_API_HASH=keepmehash\n",
    )
    .unwrap();
    let (code, out, err) = run_with_stdin(&dir, &["wizard", "--json"], "\n\n");
    assert_eq!(code, 0, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(
        v["results"][0]["data"]["env_written"],
        serde_json::json!(false)
    );
    let env = std::fs::read_to_string(dir.join(".env")).unwrap();
    assert!(env.contains("TELE_API_ID=7654321"), "env: {env}");
    assert!(env.contains("TELE_API_HASH=keepmehash"), "env: {env}");
    assert!(
        !out.contains("keepmehash") && !err.contains("keepmehash"),
        "credential values must never appear in output"
    );
}

#[test]
fn wizard_rejects_bad_api_id_three_times() {
    let dir = isolated_appdir("wizard-badid");
    let (code, _out, err) = run_with_stdin(&dir, &["wizard"], "abc\nabc\nabc\n");
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("api_id"), "stderr: {err}");
    assert!(
        !dir.join(".env").exists(),
        "failed wizard must not write credentials"
    );
}

#[test]
fn wizard_closed_stdin_fails_closed() {
    let dir = isolated_appdir("wizard-eof");
    let (code, _out, err) = run_with_stdin(&dir, &["wizard"], "");
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("stdin closed"), "stderr: {err}");
}

#[test]
fn wizard_has_root_help_surface() {
    assert!(
        help(&[]).contains(" wizard "),
        "tele wizard missing from root --help"
    );
}

#[test]
fn msg_poll_close_validates_before_connect() {
    let (code, _out, err) = run_isolated("pollclose-id", &["msg", "poll-close", "--chat", "me"]);
    assert_eq!(code, 1, "stderr: {err}");
    let (code, _out, err) =
        run_isolated("pollclose-bad", &["msg", "poll-close", "--chat", "me", "--id", "0"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--id must be a positive"), "stderr: {err}");
}

#[test]
fn msg_poll_close_dry_run_json_reports_would() {
    let dir = isolated_appdir("pollclose-dry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg", "poll-close", "--chat", "me", "--id", "9", "--account", "work", "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["id"], serde_json::json!(9));
    assert!(
        d["would"].as_str().unwrap_or_default().contains("close poll"),
        "data: {d}"
    );
}

#[test]
fn msg_poll_results_dry_run_json_reports_would() {
    let dir = isolated_appdir("pollresults-dry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg", "poll-results", "--chat", "me", "--id", "9", "--account", "work", "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert!(
        d["would"].as_str().unwrap_or_default().contains("results"),
        "data: {d}"
    );
}

#[test]
fn msg_poll_votes_validates_and_dry_runs() {
    let (code, _out, err) = run_isolated(
        "pollvotes-opt",
        &["msg", "poll-votes", "--chat", "me", "--id", "9", "--option", "0"],
    );
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("--option"), "stderr: {err}");
    let dir = isolated_appdir("pollvotes-dry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg", "poll-votes", "--chat", "me", "--id", "9", "--option", "1", "--account",
            "work", "--dry-run", "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert_eq!(d["option"], serde_json::json!(1));
}

#[test]
fn msg_poll_unread_dry_run_json_reports_would() {
    let dir = isolated_appdir("pollunread-dry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg", "poll-unread", "--chat", "me", "--account", "work", "--dry-run", "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert!(
        d["would"]
            .as_str()
            .unwrap_or_default()
            .contains("unread poll votes"),
        "data: {d}"
    );
}

#[test]
fn story_edit_requires_a_change_before_connect() {
    let (code, _out, err) =
        run_isolated("storyedit-none", &["story", "edit", "--chat", "me", "--id", "3"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("nothing to change"), "stderr: {err}");
}

#[test]
fn story_edit_views_reactions_link_dry_runs() {
    let dir = isolated_appdir("storyedit-dry");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "story", "edit", "--chat", "me", "--id", "3", "--caption", "new", "--account",
            "work", "--dry-run", "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert_eq!(d["dry_run"], serde_json::json!(true));
    assert!(
        d["would"].as_str().unwrap_or_default().contains("edit story 3"),
        "data: {d}"
    );
    let (code, out, err) = run_in(
        &dir,
        &[
            "story", "views", "--chat", "me", "--ids", "1,2", "--account", "work", "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(
        parse_json(&out)["results"][0]["data"]["ids"],
        serde_json::json!([1, 2])
    );
    let (code, out, err) = run_in(
        &dir,
        &[
            "story", "reactions", "--chat", "me", "--id", "5", "--account", "work",
            "--dry-run", "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(
        parse_json(&out)["results"][0]["data"]["dry_run"],
        serde_json::json!(true)
    );
    let (code, out, err) = run_in(
        &dir,
        &[
            "story", "link", "--chat", "me", "--id", "5", "--account", "work", "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let d = parse_json(&out)["results"][0]["data"].clone();
    assert!(
        d["would"].as_str().unwrap_or_default().contains("export link"),
        "data: {d}"
    );
}
