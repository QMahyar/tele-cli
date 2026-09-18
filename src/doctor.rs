use crate::error::TeleResult;
use crate::executor::GlobalFlags;

struct Check {
    name: String,
    ok: bool,
    detail: String,
}

impl Check {
    fn row(&self) -> serde_json::Value {
        serde_json::json!({
            "check": self.name,
            "ok": self.ok,
            "detail": self.detail,
        })
    }

    fn table(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            if self.ok { "ok" } else { "FAIL" }.to_string(),
            self.detail.clone(),
        ]
    }
}

pub(crate) fn credential_presence(dir: &std::path::Path) -> (bool, bool) {
    let from_env = |key: &str| {
        std::env::var(key)
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
    };
    let map = crate::config::load_env(&dir.join(".env"));
    let from_file = |key: &str| map.get(key).is_some_and(|v| !v.trim().is_empty());
    (
        from_env("TELE_API_ID") || from_file("TELE_API_ID"),
        from_env("TELE_API_HASH") || from_file("TELE_API_HASH"),
    )
}

#[cfg(unix)]
fn is_private(path: &std::path::Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn is_private(_path: &std::path::Path) -> Option<bool> {
    None
}

fn file_check(name: &str, path: &std::path::Path, absent_detail: &str) -> Check {
    if !path.exists() {
        return Check {
            name: name.to_string(),
            ok: true,
            detail: absent_detail.to_string(),
        };
    }
    match is_private(path) {
        Some(true) => Check {
            name: name.to_string(),
            ok: true,
            detail: "present, private".to_string(),
        },
        Some(false) => Check {
            name: name.to_string(),
            ok: false,
            detail: "present, world-accessible: tighten permissions".to_string(),
        },
        None => Check {
            name: name.to_string(),
            ok: true,
            detail: "present (permission check unavailable on this platform)".to_string(),
        },
    }
}

fn session_check(dir: &std::path::Path, name: &str) -> Check {
    let path = dir.join("sessions").join(format!("{name}.session"));
    if !path.exists() {
        return Check {
            name: format!("session:{name}"),
            ok: false,
            detail: format!("missing: run tele account login {name}"),
        };
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Check {
            name: format!("session:{name}"),
            ok: false,
            detail: "empty file: run tele account login again".to_string(),
        };
    }
    let mut detail = format!("present ({size} bytes)");
    if dir
        .join("sessions")
        .join(format!("{name}.session.lock"))
        .exists()
    {
        detail.push_str("; locked (possibly in use by another process)");
    }
    match is_private(&path) {
        Some(true) | None => Check {
            name: format!("session:{name}"),
            ok: true,
            detail,
        },
        Some(false) => Check {
            name: format!("session:{name}"),
            ok: false,
            detail: "present, world-accessible: tighten permissions".to_string(),
        },
    }
}

fn summarize(checks: &[Check]) -> (usize, usize) {
    let failed = checks.iter().filter(|c| !c.ok).count();
    (checks.len() - failed, failed)
}

pub async fn run(flags: &GlobalFlags) -> TeleResult<i32> {
    if flags.dry_run {
        let data = serde_json::json!({
            "dry_run": true,
            "would": "run local health checks (config, credentials, sessions)",
        });
        let envelope = crate::output::Envelope::new(
            vec![crate::output::AccountOutcome {
                account: "local".to_string(),
                ok: true,
                error: None,
                data: Some(data),
                exit_code: None,
            }],
            true,
            &flags.command,
        );
        return crate::executor::finish(flags, &envelope);
    }
    let dir = crate::config::app_data_dir_checked()?;
    let names = crate::executor::select_accounts(flags)?;
    let mut checks = Vec::new();
    checks.push(if dir.exists() {
        Check {
            name: "app_dir".to_string(),
            ok: true,
            detail: dir.display().to_string(),
        }
    } else {
        Check {
            name: "app_dir".to_string(),
            ok: false,
            detail: format!(
                "missing: {} (set TELE_APP_DIR to choose a location)",
                dir.display()
            ),
        }
    });
    match crate::config::load_config(flags.config_path.as_deref()) {
        Ok(cfg) => checks.push(Check {
            name: "config".to_string(),
            ok: true,
            detail: format!("parsed ({} accounts)", cfg.accounts.len()),
        }),
        Err(e) => checks.push(Check {
            name: "config".to_string(),
            ok: false,
            detail: e.message(),
        }),
    }
    let (has_id, has_hash) = credential_presence(&dir);
    checks.push(Check {
        name: "credentials".to_string(),
        ok: has_id && has_hash,
        detail: format!(
            "TELE_API_ID {}, TELE_API_HASH {}",
            if has_id { "set" } else { "missing" },
            if has_hash { "set" } else { "missing" },
        ),
    });
    checks.push(file_check(
        "env_file",
        &dir.join(".env"),
        "absent (credentials may come from process environment)",
    ));
    let sessions_dir = dir.join("sessions");
    if !sessions_dir.exists() {
        checks.push(Check {
            name: "sessions_dir".to_string(),
            ok: true,
            detail: "absent (no sessions yet)".to_string(),
        });
    }
    for name in &names {
        checks.push(session_check(&dir, name));
    }
    let (passed, failed) = summarize(&checks);
    let data = serde_json::json!({
        "checks": checks.iter().map(Check::row).collect::<Vec<_>>(),
        "passed": passed,
        "failed": failed,
    });
    let outcome = crate::output::AccountOutcome {
        account: "local".to_string(),
        ok: failed == 0,
        error: None,
        data: Some(data),
        exit_code: None,
    };
    let envelope = crate::output::Envelope::new(vec![outcome], false, &flags.command);
    if !crate::output::machine_mode(flags.json, flags.jsonl) {
        let rows: Vec<Vec<String>> = checks.iter().map(Check::table).collect();
        crate::output::print_table(&["check", "status", "detail"], &rows)?;
    }
    crate::executor::finish(flags, &envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-doctor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn credential_presence_empty_dir_reports_both_missing() {
        let _guard = lock_env();
        std::env::remove_var("TELE_API_ID");
        std::env::remove_var("TELE_API_HASH");
        let dir = temp_dir("missing");
        assert_eq!(credential_presence(&dir), (false, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_presence_reads_env_file() {
        let _guard = lock_env();
        std::env::remove_var("TELE_API_ID");
        std::env::remove_var("TELE_API_HASH");
        let dir = temp_dir("file");
        std::fs::write(dir.join(".env"), "TELE_API_ID=42\nTELE_API_HASH=abc\n").unwrap();
        assert_eq!(credential_presence(&dir), (true, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_presence_prefers_process_env() {
        let _guard = lock_env();
        let dir = temp_dir("env");
        std::env::set_var("TELE_API_ID", "42");
        std::env::set_var("TELE_API_HASH", "abc");
        assert_eq!(credential_presence(&dir), (true, true));
        std::env::remove_var("TELE_API_ID");
        std::env::remove_var("TELE_API_HASH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_presence_ignores_blank_values() {
        let _guard = lock_env();
        std::env::remove_var("TELE_API_ID");
        std::env::remove_var("TELE_API_HASH");
        let dir = temp_dir("blank");
        std::fs::write(dir.join(".env"), "TELE_API_ID=   \nTELE_API_HASH=\n").unwrap();
        assert_eq!(credential_presence(&dir), (false, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_private_missing_path_is_unknown() {
        let dir = temp_dir("perm");
        assert_eq!(is_private(&dir.join("nope")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summarize_counts_failures() {
        let checks = vec![
            Check {
                name: "a".to_string(),
                ok: true,
                detail: String::new(),
            },
            Check {
                name: "b".to_string(),
                ok: false,
                detail: String::new(),
            },
        ];
        assert_eq!(summarize(&checks), (1, 1));
        assert_eq!(summarize(&[]), (0, 0));
    }

    #[test]
    fn check_row_and_table_shapes() {
        let check = Check {
            name: "config".to_string(),
            ok: true,
            detail: "parsed".to_string(),
        };
        assert_eq!(
            check.row(),
            serde_json::json!({"check": "config", "ok": true, "detail": "parsed"})
        );
        assert_eq!(
            check.table(),
            vec!["config".to_string(), "ok".to_string(), "parsed".to_string()]
        );
    }

    #[test]
    fn session_check_missing_names_login_command() {
        let dir = temp_dir("sess");
        let check = session_check(&dir, "work");
        assert!(!check.ok);
        assert!(check.detail.contains("tele account login work"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_check_empty_file_fails() {
        let dir = temp_dir("sessempty");
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions").join("work.session"), b"").unwrap();
        assert!(!session_check(&dir, "work").ok);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
