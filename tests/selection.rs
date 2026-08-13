use std::path::PathBuf;
use std::process::Command;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn tele() -> Command {
    Command::new(env!("CARGO_BIN_EXE_telecli"))
}

fn appdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("telecli-kernel-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
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

fn write_session(dir: &std::path::Path, name: &str) {
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(format!("{name}.session")), b"dummy").unwrap();
}

fn write_config(dir: &std::path::Path, toml: &str) {
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

fn list_sessions(dir: &std::path::Path) -> Vec<String> {
    let sessions = dir.join("sessions");
    let Ok(entries) = std::fs::read_dir(&sessions) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    out.sort();
    out
}

#[test]
fn unknown_account_is_usage_error_and_creates_no_session() {
    let dir = appdir("unknown");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
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
    assert_eq!(list_sessions(&dir), vec!["work.session"]);
}

#[test]
fn account_name_traversal_rejected() {
    let dir = appdir("traversal");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "..\\evil",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("unknown account"), "stderr: {err}");
    assert_eq!(list_sessions(&dir), vec!["work.session"]);
    assert!(!dir.join("evil.session").exists());
}

#[test]
fn account_all_expands_to_all_sessions() {
    let dir = appdir("all");
    write_session(&dir, "home");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "all",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("stdout must be JSON");
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    let results = v["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "stdout: {out}");
    let names: Vec<&str> = results
        .iter()
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["home", "work"]);
    for r in results {
        assert_eq!(r["ok"], serde_json::json!(true));
        assert!(r["error"].is_null(), "error must be null: {r}");
        assert_eq!(r["data"]["dry_run"], serde_json::json!(true));
    }
}

#[test]
fn account_all_with_no_sessions_is_usage_error() {
    let dir = appdir("allnone");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "all",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("no accounts selected"), "stderr: {err}");
}

#[test]
fn configured_account_without_session_is_accepted() {
    let dir = appdir("pending");
    write_config(&dir, "[accounts.pending]\ntags = [\"later\"]\n");
    let (code, out, _err) = run_in(
        &dir,
        &[
            "msg",
            "send",
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
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["results"][0]["account"], serde_json::json!("pending"));
    assert_eq!(list_sessions(&dir), Vec::<String>::new());
}

#[test]
fn repeated_account_flags_are_union() {
    let dir = appdir("repeat");
    write_session(&dir, "home");
    write_session(&dir, "work");
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--account",
            "home",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
}

#[test]
fn tag_union_with_account() {
    let dir = appdir("tagunion");
    write_session(&dir, "home");
    write_session(&dir, "work");
    write_config(
        &dir,
        "[accounts.home]\ntags = [\"iran\"]\n[accounts.work]\ntags = [\"iran\"]\n",
    );
    let (code, out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--account",
            "work",
            "--tag",
            "iran",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "stdout: {out}");
    let names: Vec<&str> = results
        .iter()
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["home", "work"]);
}

#[test]
fn tag_selects_only_accounts_with_sessions() {
    let dir = appdir("tagonly");
    write_session(&dir, "home");
    write_config(
        &dir,
        "[accounts.home]\ntags = [\"iran\"]\n[accounts.pending]\ntags = [\"iran\"]\n",
    );
    let (code, out, _err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--tag",
            "iran",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "stdout: {out}");
    assert_eq!(results[0]["account"], serde_json::json!("home"));
}

#[test]
fn unknown_tag_is_usage_error() {
    let dir = appdir("tagnone");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--tag",
            "nosuch",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("no accounts with tag nosuch"), "stderr: {err}");
}

#[test]
fn json_and_jsonl_are_mutually_exclusive() {
    let dir = appdir("bothjson");
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
            "--dry-run",
            "--json",
            "--jsonl",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("mutually exclusive"), "stderr: {err}");
}

#[test]
fn login_rejects_unsafe_name_without_writing_files() {
    let dir = appdir("loginbad");
    let out = tele()
        .args([
            "account",
            "login",
            "--name",
            "..\\evil",
            "--phone",
            "+10000000000",
        ])
        .env("TELE_APP_DIR", &dir)
        .env("TELE_API_ID", "12345")
        .env("TELE_API_HASH", "deadbeefdeadbeef")
        .output()
        .unwrap();
    let code = out.status.code().unwrap_or(-1);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(code, 3);
    assert!(err.contains("invalid account name"), "stderr: {err}");
    assert!(!dir.join("evil.session").exists());
    assert!(!dir.join("..").join("evil.session").exists());
}

#[test]
fn remove_rejects_unsafe_name() {
    let dir = appdir("removebad");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(&dir, &["account", "remove", "--name", "..\\evil"]);
    assert_eq!(code, 3);
    assert!(err.contains("invalid account name"), "stderr: {err}");
    assert_eq!(list_sessions(&dir), vec!["work.session"]);
}

#[test]
fn help_lists_quiet_and_verbose_globals() {
    let dir = appdir("helpflags");
    let out = tele()
        .env("TELE_APP_DIR", &dir)
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("-q, --quiet"), "help: {text}");
    assert!(text.contains("-v, --verbose"), "help: {text}");
}

#[test]
fn verbose_flag_is_accepted_on_any_group() {
    let dir = appdir("verbose");
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "-vv",
            "msg",
            "send",
            "--account",
            "work",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn dry_run_never_touches_network_or_sessions() {
    let dir = appdir("dryrun");
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
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert_eq!(list_sessions(&dir), vec!["work.session"]);
}

#[test]
fn malformed_config_is_surfaced() {
    let dir = appdir("badcfg");
    std::fs::write(dir.join("config.toml"), "not [valid toml").unwrap();
    write_session(&dir, "work");
    let (code, _out, err) = run_in(
        &dir,
        &[
            "msg",
            "send",
            "--tag",
            "x",
            "--chat",
            "me",
            "--text",
            "hi",
            "--dry-run",
        ],
    );
    assert_eq!(code, 3);
    assert!(err.contains("failed to parse"), "stderr: {err}");
}

#[test]
fn readme_envelope_has_no_command_field_until_plumbed() {
    let dir = appdir("envelope");
    write_session(&dir, "work");
    let (code, out, _err) = run_in(
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
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v.get("command").is_none(), "command not plumbed yet: {out}");
    assert!(v.get("results").is_some(), "results key required: {out}");
    assert!(v.get("accounts").is_none(), "accounts key removed: {out}");
}

#[test]
fn manifest_contract_row_exists() {
    let md =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("docs/cli-contract.md")).unwrap();
    assert!(md.contains("--account NAME     repeatable; NAME or all"));
    assert!(md.contains("--tag TAG          repeatable; union with --account"));
}
