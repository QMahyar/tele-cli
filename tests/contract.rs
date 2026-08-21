use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn tele() -> Command {
    Command::new(env!("CARGO_BIN_EXE_telecli"))
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
        let status = cells.last().unwrap();
        if status == &"done" {
            rows.push((
                cells[0].to_string(),
                "done".to_string(),
                cells[cells.len() - 2].to_string(),
            ));
        }
    }
    rows
}

fn cell_token(cli: &str) -> &str {
    match cli.split_once('`') {
        Some((_, after)) => match after.split_once('`') {
            Some((token, _)) => token.trim(),
            None => after.trim(),
        },
        None => cli.trim(),
    }
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
        "account", "msg", "chat", "dialog", "topic", "contact", "profile", "privacy", "takeout",
        "listen", "raw",
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
    assert!(err.contains("no accounts selected"), "stderr: {err}");
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
fn parallel_out_of_range_is_clamped_not_usage_error() {
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
            code, 0,
            "--parallel {n} must clamp to 1-32, not a usage error: stderr: {err}"
        );
    }
}

#[test]
fn parallel_out_of_range_warns_on_stderr() {
    let (code, _out, err) = run_isolated("parwarn", &["--parallel", "99", "account", "list"]);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(err.contains("clamped"), "stderr: {err}");
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
            .contains("no accounts selected"),
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
    assert!(err.contains("requires --allow or --deny"), "stderr: {err}");
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
        let token = cell_token(&cli);
        if let Some(cmd) = token.strip_prefix("tele ") {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            let group = parts[0];
            assert!(
                root_help
                    .lines()
                    .any(|l| l.split_whitespace().next().unwrap_or("") == group),
                "row {id}: group {group} missing from root --help"
            );
            if parts.len() < 2 {
                continue;
            }
            let next = parts[1];
            if next == "*" || next.starts_with('(') {
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
            let lhelp = help(&["listen"]);
            for flag in token.split_whitespace().filter(|p| p.starts_with("--")) {
                assert!(
                    lhelp.contains(flag),
                    "row {id}: flag {flag} missing from `tele listen --help`"
                );
            }
            for word in token.split_whitespace().filter(|p| !p.starts_with("--")) {
                assert!(
                    listen_src.contains(&format!("\"{word}\"")),
                    "row {id}: event {word} missing from src/commands/listen.rs"
                );
            }
        } else if !registry.iter().any(|r| r == token) {
            panic!(
                "row {id}: CLI cell `{cli}` has no CLI surface (no tele command, src module, listen flag, or raw registry entry)"
            );
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
    assert!(names.len() >= 6, "registry should hold all raw arms");
    for name in &names {
        assert!(
            src.contains(&format!("\"{name}\" =>")),
            "registry name {name} has no dispatch arm"
        );
    }
    let args_for = |name: &str| match name {
        "contacts.Search" => "{\"q\":\"x\"}",
        "messages.ExportChatInvite" => "{\"chat\":\"me\"}",
        "stats.GetBroadcastStats" | "stats.GetMegagroupStats" => "{\"channel\":\"me\"}",
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
    let lock_path = PathBuf::from(MANIFEST_DIR).join("Cargo.lock");
    let lock = match std::fs::read_to_string(&lock_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!(
                "SKIP: cannot read Cargo.lock at {} — re-vendor tl/api.tl after a grammers bump",
                lock_path.display()
            );
            return;
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
            eprintln!(
                "SKIP: grammers-tl-types not found in Cargo.lock — re-vendor tl/api.tl after a grammers bump"
            );
            return;
        }
    };
    let cargo_home = std::env::var("CARGO_HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".cargo"));
    let cargo_home = match cargo_home {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: CARGO_HOME not set — re-vendor tl/api.tl after a grammers bump");
            return;
        }
    };
    let registry_src = cargo_home.join("registry").join("src");
    let upstream = match find_upstream_api_tl(&registry_src, &version) {
        Some(p) => p,
        None => {
            eprintln!(
                "SKIP: cannot locate upstream api.tl for grammers-tl-types {version} — re-vendor tl/api.tl after a grammers bump"
            );
            return;
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
