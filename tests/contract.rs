use std::path::PathBuf;
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

fn help(args: &[&str]) -> String {
    let out = tele()
        .args(args)
        .arg("--help")
        .env("TELE_APP_DIR", isolated_appdir("help"))
        .output()
        .expect("spawn telecli --help");
    String::from_utf8_lossy(&out.stdout).into_owned()
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
fn unknown_command_exits_2() {
    let (code, _out, err) = run_isolated("unknown", &["bogus-group"]);
    assert_eq!(code, 2);
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
fn raw_registered_name_reaches_fanout() {
    let (code, _out, err) = run_isolated("rawreg", &["raw", "messages.GetAllDrafts"]);
    assert_eq!(code, 1);
    assert!(err.contains("no accounts selected"), "stderr: {err}");
}

#[test]
fn listen_unknown_event_exits_1_before_connect() {
    let (code, _out, err) = run_isolated("lsev", &["listen", "--events", "Bogus"]);
    assert_eq!(code, 1);
    assert!(err.contains("unknown event name"), "stderr: {err}");
}

#[test]
fn listen_valid_events_reach_selection() {
    let (code, _out, err) =
        run_isolated("lsok", &["listen", "--events", "NewMessage,MessageDeleted"]);
    assert_eq!(code, 1);
    assert!(err.contains("no accounts selected"), "stderr: {err}");
}

#[test]
fn account_list_json_is_one_object() {
    let (code, out, _err) = run_isolated("acclist", &["account", "list", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value =
        serde_json::from_str(out.trim()).expect("stdout must be one JSON object");
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
    let (code, _out, _err) =
        run_isolated("dryrun", &["account", "login", "--name", "x", "--dry-run"]);
    assert_eq!(code, 0);
}

#[test]
fn want_rows_have_cli_surface() {
    let md =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("docs/capabilities.md")).unwrap();
    let root_help = help(&[]);
    for line in md.lines() {
        let line = line.trim();
        if !line.starts_with('|') || !line.ends_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 4 || cells.last() != Some(&"want") {
            continue;
        }
        let id = cells[0];
        let cli = cells[cells.len() - 2];
        if let Some(cmd) = cli.strip_prefix("tele ") {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            let group = parts[0];
            assert!(
                root_help.contains(&format!(" {group} ")),
                "row {id}: group {group} missing from root --help"
            );
            if parts.len() > 1 {
                let sub = parts[1];
                let ghelp = help(&[group]);
                assert!(
                    ghelp.lines().any(|l| l.trim_start().starts_with(sub)),
                    "row {id}: subcommand {sub} missing from `tele {group} --help`"
                );
                for flag in parts.iter().filter(|p| p.starts_with("--")) {
                    let shelp = help(&[group, sub]);
                    assert!(
                        shelp.contains(flag),
                        "row {id}: flag {flag} missing from `tele {group} {sub} --help`"
                    );
                }
            }
        } else if let Some(module) = cli.strip_prefix("src/") {
            assert!(
                PathBuf::from(MANIFEST_DIR)
                    .join("src")
                    .join(module.replace('`', ""))
                    .exists(),
                "row {id}: module {module} missing"
            );
        }
    }
}

#[test]
fn raw_registry_rows_have_arms() {
    let md =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("docs/capabilities.md")).unwrap();
    let raw_src =
        std::fs::read_to_string(PathBuf::from(MANIFEST_DIR).join("src/commands/raw.rs")).unwrap();
    for line in md.lines() {
        let line = line.trim();
        if !line.starts_with('|') || !line.ends_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 4 || cells.last() != Some(&"want") {
            continue;
        }
        let cli = cells[cells.len() - 2];
        if cli.starts_with("tele raw ") {
            let name = cli.trim_start_matches("tele raw ").trim_matches('`');
            if name.starts_with("registry") {
                continue;
            }
            assert!(
                raw_src.contains(&format!("\"{name}\" =>")),
                "row {}: raw registry missing arm for {name}",
                cells[0]
            );
        }
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
