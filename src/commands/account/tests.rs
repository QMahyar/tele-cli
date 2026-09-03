use super::*;
use crate::commands::account::login::{
    argv_phone_warning, load_pending_document_under, pending_dir_under, qr_token_lines,
    remove_pending_document_under, resolve_phone, should_print_token, validate_login,
    DEFAULT_QR_TIMEOUT_SECS, QR_TOKEN_NON_TTY_WARNING, STAGE_HELP, TELE_PHONE_ENV,
};
use crate::commands::account::password::*;
use crate::commands::account::phone::*;
use crate::commands::account::staged_login::*;
use crate::commands::serve::{Lane, Plan};
use grammers_session::storages::SqliteSession;

fn login_args(method: &str, phone: Option<&str>) -> LoginArgs {
    LoginArgs {
        name: "x".to_string(),
        method: method.to_string(),
        phone: phone.map(str::to_string),
        show_token: false,
        qr_timeout_secs: DEFAULT_QR_TIMEOUT_SECS,
        stage: None,
    }
}

fn staged_args(method: &str, phone: Option<&str>, stage: Option<&str>) -> LoginArgs {
    LoginArgs {
        stage: stage.map(str::to_string),
        ..login_args(method, phone)
    }
}

fn pending_fixture(account: &str) -> PendingLogin {
    PendingLogin::new(account, "+15551234567", "hash-token-123".to_string())
}

fn staged_temp_dir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("telecli-staged-{}-{seq}", std::process::id()))
}

async fn with_pending_env<F, T>(f: impl FnOnce(std::path::PathBuf) -> F) -> T
where
    F: std::future::Future<Output = T>,
{
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    f(dir).await
}

async fn with_tele_app_env<F, T>(f: impl FnOnce(std::path::PathBuf) -> F) -> T
where
    F: std::future::Future<Output = T>,
{
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("TELE_APP_DIR", &dir);
    std::env::remove_var(TELE_PHONE_ENV);
    let result = f(dir.clone()).await;
    std::env::remove_var("TELE_APP_DIR");
    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn test_flags(command: &str, json: bool, dry_run: bool, config: &std::path::Path) -> GlobalFlags {
    GlobalFlags {
        account: vec![],
        tag: vec![],
        parallel: None,
        json,
        jsonl: false,
        dry_run,
        quiet: false,
        config_path: Some(config.to_path_buf()),
        command: command.to_string(),
    }
}

#[test]
fn login_code_requires_phone() {
    assert!(matches!(
        validate_login("code", None, DEFAULT_QR_TIMEOUT_SECS),
        Err(TeleError::Usage(_))
    ));
    assert!(validate_login("code", Some("+1"), DEFAULT_QR_TIMEOUT_SECS).is_ok());
}

#[test]
fn login_qr_needs_no_phone() {
    assert!(validate_login("qr", None, DEFAULT_QR_TIMEOUT_SECS).is_ok());
}

#[test]
fn login_unknown_method_rejected() {
    assert!(matches!(
        validate_login("sms", Some("+1"), DEFAULT_QR_TIMEOUT_SECS),
        Err(TeleError::Usage(_))
    ));
}

#[test]
fn login_qr_timeout_must_be_positive() {
    assert!(matches!(
        validate_login("qr", None, 0),
        Err(TeleError::Usage(_))
    ));
    assert!(validate_login("code", Some("+1"), 1).is_ok());
}

#[test]
pub(crate) fn resolve_phone_prefers_explicit_arg_over_env() {
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var(TELE_PHONE_ENV, "+15550001111");
    let resolved = resolve_phone(Some("+15557772222"));
    std::env::remove_var(TELE_PHONE_ENV);
    assert_eq!(resolved.as_deref(), Some("+15557772222"));
}

#[test]
pub(crate) fn resolve_phone_falls_back_to_nonempty_env() {
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var(TELE_PHONE_ENV, " +15550001111 ");
    let resolved = resolve_phone(None);
    std::env::remove_var(TELE_PHONE_ENV);
    assert_eq!(resolved.as_deref(), Some("+15550001111"));
}

#[test]
pub(crate) fn resolve_phone_ignores_empty_env_and_empty_arg() {
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::remove_var(TELE_PHONE_ENV);
    assert!(resolve_phone(None).is_none());
    std::env::set_var(TELE_PHONE_ENV, "+15550001111");
    let from_env = resolve_phone(Some("   "));
    std::env::remove_var(TELE_PHONE_ENV);
    assert_eq!(from_env.as_deref(), Some("+15550001111"));
}

#[test]
pub(crate) fn code_prompt_includes_phone_only_on_terminal_stderr() {
    assert_eq!(
        code_prompt(Some("+15551234567"), true),
        "Enter the code sent to +1***567: "
    );
    assert_eq!(code_prompt(Some("+15551234567"), false), "Enter the code: ");
    assert_eq!(code_prompt(None, true), "Enter the code: ");
    assert_eq!(code_prompt(None, false), "Enter the code: ");
}

#[test]
pub(crate) fn argv_phone_warning_fires_whenever_phone_is_passed() {
    assert!(argv_phone_warning(Some("+15551234567"), false).is_some());
    assert!(argv_phone_warning(Some("+15551234567"), true).is_some());
    assert!(argv_phone_warning(None, false).is_none());
    assert!(argv_phone_warning(None, true).is_none());
}

#[test]
pub(crate) fn should_print_token_gates_raw_uri() {
    assert!(should_print_token(true, false));
    assert!(should_print_token(false, true));
    assert!(!should_print_token(false, false));
}

#[test]
pub(crate) fn qr_token_lines_non_terminal_stderr_warns_and_keeps_token() {
    let (warning, line) = qr_token_lines("tg://login?token=abc123", false);
    assert_eq!(warning, Some(QR_TOKEN_NON_TTY_WARNING));
    assert_eq!(line, "URI: tg://login?token=abc123");
}

#[test]
pub(crate) fn qr_token_lines_terminal_stderr_emits_token_without_warning() {
    let (warning, line) = qr_token_lines("tg://login?token=abc123", true);
    assert_eq!(warning, None);
    assert_eq!(line, "URI: tg://login?token=abc123");
}

#[test]
pub(crate) fn prompt_line_writes_prompt_then_reads_line() {
    let mut out = Vec::new();
    let line = prompt_line(
        "Enter the code: ",
        &mut std::io::Cursor::new(b"12345\n"),
        &mut out,
    )
    .unwrap();
    assert_eq!(line.as_deref(), Some("12345\n"));
    assert_eq!(String::from_utf8(out).unwrap(), "Enter the code: ");
}

#[test]
pub(crate) fn prompt_line_eof_returns_none() {
    let mut out = Vec::new();
    let line = prompt_line("Enter password: ", &mut std::io::Cursor::new(b""), &mut out).unwrap();
    assert!(line.is_none());
    assert_eq!(String::from_utf8(out).unwrap(), "Enter password: ");
}

#[test]
pub(crate) fn strip_line_ending_preserves_password_spaces() {
    assert_eq!(strip_line_ending(" pass word \r\n"), " pass word ");
    assert_eq!(strip_line_ending(" pass word \n"), " pass word ");
}

#[test]
pub(crate) fn strip_line_ending_removes_only_line_terminator() {
    assert_eq!(strip_line_ending("password"), "password");
    assert_eq!(strip_line_ending("pass\n"), "pass");
}

#[test]
pub(crate) fn action_envelope_matches_contract_shape() {
    let envelope = action_envelope(
        "work",
        serde_json::json!({"authorized": true}),
        false,
        "account login",
    );
    let v = serde_json::to_value(&envelope).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("account login"));
    assert_eq!(v["dry_run"], serde_json::json!(false));
    let r = &v["results"][0];
    assert_eq!(r["account"], serde_json::json!("work"));
    assert_eq!(r["ok"], serde_json::json!(true));
    assert_eq!(r["data"]["authorized"], serde_json::json!(true));
    assert!(r["error"].is_null());
}

#[test]
pub(crate) fn dry_run_envelope_marks_dry_run_and_describes_action() {
    let envelope = dry_run_envelope(
        "work",
        "register account work in config.toml",
        "account add",
    );
    let v = serde_json::to_value(&envelope).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["dry_run"], serde_json::json!(true));
    assert_eq!(v["results"][0]["data"]["dry_run"], serde_json::json!(true));
    assert_eq!(
        v["results"][0]["data"]["would"],
        serde_json::json!("register account work in config.toml")
    );
}

#[test]
pub(crate) fn add_dry_run_data_carries_argument_keys() {
    let value = add_dry_run_data(
        "work",
        &Some(vec!["a".to_string(), "b".to_string()]),
        "register account work in config.toml",
    );
    assert_eq!(value["dry_run"], serde_json::json!(true));
    assert_eq!(value["name"], serde_json::json!("work"));
    assert_eq!(value["tags"], serde_json::json!(["a", "b"]));
    assert_eq!(
        value["would"],
        serde_json::json!("register account work in config.toml")
    );
    let no_tags = add_dry_run_data("home", &None, "register account home in config.toml");
    assert_eq!(no_tags["tags"], serde_json::Value::Null);
}

#[test]
pub(crate) fn list_envelope_keeps_accounts_and_adds_results() {
    let dir = std::env::temp_dir().join(format!("telecli-list-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account list", true, false, &dir.join("config.toml"));
    let rows = vec![serde_json::json!({
        "name": "work",
        "tags": "a,b",
        "session": "present",
    })];
    let v = list_envelope(&rows, &flags).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["command"], serde_json::json!("account list"));
    assert_eq!(v["dry_run"], serde_json::json!(false));
    assert_eq!(v["accounts"][0]["name"], serde_json::json!("work"));
    let r = &v["results"][0];
    assert_eq!(r["account"], serde_json::json!("work"));
    assert_eq!(r["ok"], serde_json::json!(true));
    assert_eq!(r["data"]["tags"], serde_json::json!("a,b"));
    assert!(r["error"].is_null());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_human_rows_report_authorization() {
    let mut failed = single_outcome("bad", serde_json::json!({}));
    failed.ok = false;
    let envelope = Envelope::new(
        vec![
            single_outcome("home", serde_json::json!({"authorized": true})),
            single_outcome("work", serde_json::json!({"authorized": false})),
            failed,
        ],
        false,
        "account status",
    );
    let rows = status_table_rows(&envelope);
    assert_eq!(
        rows,
        vec![
            vec!["home".to_string(), "yes".to_string(), "-".to_string()],
            vec!["work".to_string(), "no".to_string(), "-".to_string()],
        ]
    );
}

#[test]
fn status_device_object_omits_unset_fields() {
    let identity = config::DeviceIdentity::default();
    let data = status_device_data(&identity);
    assert_eq!(data, serde_json::json!({}));
}

#[test]
fn status_device_object_keeps_configured_fields() {
    let identity = config::DeviceIdentity {
        device_model: Some("Desktop".to_string()),
        system_version: Some("Windows".to_string()),
        app_version: Some("1.2.3".to_string()),
        lang_code: Some("en".to_string()),
    };
    assert_eq!(
        status_device_data(&identity),
        serde_json::json!({
            "device_model": "Desktop",
            "system_version": "Windows",
            "app_version": "1.2.3",
            "lang_code": "en"
        })
    );
}

#[test]
fn status_device_object_is_partial_when_only_some_fields_set() {
    let identity = config::DeviceIdentity {
        device_model: Some("Desktop".to_string()),
        ..Default::default()
    };
    assert_eq!(
        status_device_data(&identity),
        serde_json::json!({ "device_model": "Desktop" })
    );
}

#[test]
fn status_rows_show_device_summary_from_outcome_data() {
    let envelope = Envelope::new(
        vec![
            single_outcome(
                "full",
                serde_json::json!({
                    "authorized": true,
                    "device": {
                        "device_model": "Desktop",
                        "system_version": "Linux",
                        "app_version": "0.4.0",
                        "lang_code": "en"
                    }
                }),
            ),
            single_outcome(
                "partial",
                serde_json::json!({
                    "authorized": true,
                    "device": { "device_model": "Phone" }
                }),
            ),
            single_outcome("empty", serde_json::json!({"authorized": true})),
        ],
        false,
        "account status",
    );
    let rows = status_table_rows(&envelope);
    assert_eq!(rows[0][2], "Desktop/Linux/0.4.0/en");
    assert_eq!(rows[1][2], "Phone");
    assert_eq!(rows[2][2], "-");
}

#[tokio::test]
async fn login_dry_run_still_validates_method() {
    let dir = std::env::temp_dir().join(format!("telecli-login-drybad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account login", false, true, &dir.join("config.toml"));
    let err = login(&login_args("sms", Some("+15551234567")), &flags)
        .await
        .unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn login_dry_run_with_valid_method_exits_ok() {
    let dir = std::env::temp_dir().join(format!("telecli-login-dryok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account login", false, true, &dir.join("config.toml"));
    let code = login(&login_args("qr", None), &flags).await.unwrap();
    assert_eq!(code, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn add_rejects_invalid_names_before_any_write() {
    let dir = std::env::temp_dir().join(format!("telecli-add-badname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account add", false, true, &dir.join("config.toml"));
    for bad in ["all", ".", "..", "a/b", ""] {
        let err = add(
            &AddArgs {
                name: bad.to_string(),
                tags: None,
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
    }
    assert!(
        !dir.join("config.toml").exists(),
        "invalid names must not write config"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dry_run_add_writes_no_config() {
    let dir = std::env::temp_dir().join(format!("telecli-add-dryrun-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account add", false, true, &dir.join("config.toml"));
    let code = add(
        &AddArgs {
            name: "work".to_string(),
            tags: None,
        },
        &flags,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert!(
        !dir.join("config.toml").exists(),
        "dry-run must not write config"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn add_registers_account_and_writes_config() {
    let dir = std::env::temp_dir().join(format!("telecli-add-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account add", false, false, &dir.join("config.toml"));
    let code = add(
        &AddArgs {
            name: "work".to_string(),
            tags: Some(vec!["a".to_string()]),
        },
        &flags,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(text.contains("work"), "config: {text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn add_readd_without_tags_preserves_existing_tags() {
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = std::env::temp_dir().join(format!("telecli-add-tags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account add", false, false, &dir.join("config.toml"));
    let with_tags = AddArgs {
        name: "work".to_string(),
        tags: Some(vec!["ops".to_string()]),
    };
    add(&with_tags, &flags).await.unwrap();
    let without_tags = AddArgs {
        name: "work".to_string(),
        tags: None,
    };
    add(&without_tags, &flags).await.unwrap();
    let cfg = config::load_config(Some(&dir.join("config.toml"))).unwrap();
    assert_eq!(cfg.accounts["work"].tags, vec!["ops".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn remove_rejects_invalid_names_before_any_write() {
    let dir = std::env::temp_dir().join(format!("telecli-remove-badname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account remove", false, false, &dir.join("config.toml"));
    for bad in ["all", ".", "..", "a/b", ""] {
        let err = remove(
            &RemoveArgs {
                name: bad.to_string(),
            },
            &flags,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{bad:?}");
    }
    assert!(
        !dir.join("config.toml").exists(),
        "invalid names must not write config"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn remove_dry_run_rejects_reserved_name() {
    let dir = std::env::temp_dir().join(format!("telecli-remove-drybad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account remove", false, true, &dir.join("config.toml"));
    let err = remove(
        &RemoveArgs {
            name: "all".to_string(),
        },
        &flags,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn session_drop_then_remove_succeeds() {
    let dir = std::env::temp_dir().join(format!("telecli-session-close-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("closed.session");
    let session = SqliteSession::open(&path).await.unwrap();
    drop(session);
    std::fs::remove_file(&path).unwrap();
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)]
#[tokio::test]
async fn held_sqlite_session_blocks_removal_on_windows() {
    let dir = std::env::temp_dir().join(format!("telecli-session-held-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("held.session");
    let session = SqliteSession::open(&path).await.unwrap();
    let err = std::fs::remove_file(&path).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(32), "sharing violation: {err:?}");
    drop(session);
    std::fs::remove_file(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn ensure_account_config_entry_creates_missing_account() {
    let dir = std::env::temp_dir().join(format!("telecli-login-ensure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "parallel_max = 2\n").unwrap();
    ensure_account_config_entry("newuser", Some(&path)).unwrap();
    let cfg = config::load_config(Some(&path)).unwrap();
    assert!(cfg.accounts.contains_key("newuser"));
    assert_eq!(cfg.parallel_max, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn ensure_account_config_entry_noop_when_already_present() {
    let dir = std::env::temp_dir().join(format!("telecli-login-exists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "\n[accounts.work]\ntags = [\"a\"]\n").unwrap();
    ensure_account_config_entry("work", Some(&path)).unwrap();
    let cfg = config::load_config(Some(&path)).unwrap();
    assert_eq!(cfg.accounts["work"].tags, vec!["a".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

fn fixture_auth(hash: i64, current: bool) -> grammers_client::tl::types::Authorization {
    grammers_client::tl::types::Authorization {
        current,
        official_app: true,
        password_pending: false,
        encrypted_requests_disabled: false,
        call_requests_disabled: false,
        unconfirmed: false,
        hash,
        device_model: "Desktop".to_string(),
        platform: "Windows".to_string(),
        system_version: "10".to_string(),
        api_id: 6,
        app_name: "Telegram Desktop".to_string(),
        app_version: "5.2.3".to_string(),
        date_created: 1_700_000_000,
        date_active: 1_750_000_000,
        ip: "203.0.113.7".to_string(),
        country: "NL".to_string(),
        region: "".to_string(),
    }
}

#[test]
fn sessions_row_fixture_shapes_expected_keys() {
    let row = authorization_row(&fixture_auth(1_234_567_890_123, true));
    assert_eq!(row["hash"], serde_json::json!(1_234_567_890_123_i64));
    assert_eq!(row["device_model"], serde_json::json!("Desktop"));
    assert_eq!(row["platform"], serde_json::json!("Windows"));
    assert_eq!(row["system_version"], serde_json::json!("10"));
    assert_eq!(row["app"], serde_json::json!("Telegram Desktop 5.2.3"));
    assert_eq!(row["ip"], serde_json::json!("203.0.113.7"));
    assert_eq!(row["country"], serde_json::json!("NL"));
    let expected_date = chrono::DateTime::from_timestamp(1_700_000_000, 0)
        .unwrap()
        .to_rfc3339();
    assert_eq!(row["date_created"], serde_json::json!(expected_date));
    let expected_active = chrono::DateTime::from_timestamp(1_750_000_000, 0)
        .unwrap()
        .to_rfc3339();
    assert_eq!(row["date_active"], serde_json::json!(expected_active));
    assert_eq!(row["current"], serde_json::json!(true));
    assert_eq!(row["official_app"], serde_json::json!(true));
    assert_eq!(row["password_pending"], serde_json::json!(false));
}

#[test]
fn sessions_row_marks_non_current_without_current_key_truth() {
    let row = authorization_row(&fixture_auth(42, false));
    assert_eq!(row["current"], serde_json::json!(false));
    assert_ne!(row["hash"], serde_json::json!(42_i64 + 1));
}

#[test]
fn sessions_table_row_flattens_specified_columns() {
    let row = authorization_row(&fixture_auth(7, true));
    let cells = sessions_table_cells("work", &row);
    assert_eq!(
        cells,
        vec![
            "work".to_string(),
            "7".to_string(),
            "Desktop".to_string(),
            "Telegram Desktop 5.2.3".to_string(),
            "203.0.113.7".to_string(),
            "NL".to_string(),
            chrono::DateTime::from_timestamp(1_700_000_000, 0)
                .unwrap()
                .to_rfc3339(),
            "yes".to_string(),
        ]
    );
    let non_current_row = authorization_row(&fixture_auth(8, false));
    let cells = sessions_table_cells("home", &non_current_row);
    assert_eq!(cells.last().unwrap(), "");
}

#[test]
pub(crate) fn terminate_decision_refuses_current_session_hash() {
    let rows = vec![fixture_auth(111, true), fixture_auth(222, false)];
    let err = terminate_decision(111, &rows).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)));
    assert!(err.message().contains("current session"), "{err}");
    assert!(err.message().contains("111"), "{err}");
}

#[test]
pub(crate) fn terminate_decision_allows_other_session_hash() {
    let rows = vec![fixture_auth(111, true), fixture_auth(222, false)];
    terminate_decision(222, &rows).unwrap();
}

#[test]
pub(crate) fn terminate_decision_rejects_unknown_hash() {
    let rows = vec![fixture_auth(111, true)];
    let err = terminate_decision(999, &rows).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)));
    assert!(err.message().contains("999"), "{err}");
}

fn usage<T>(result: TeleResult<T>) -> bool {
    matches!(result, Err(TeleError::Usage(_)))
}

#[test]
fn password_mode_requires_exactly_one_flag() {
    assert!(usage(validate_password_modes(
        false, false, false, None, false, false, false, false, false, None, None
    )));
    assert!(usage(validate_password_modes(
        true, true, false, None, false, false, false, false, false, None, None
    )));
    assert!(usage(validate_password_modes(
        true, false, true, None, false, false, false, false, false, None, None
    )));
    assert!(usage(validate_password_modes(
        false, true, true, None, false, false, false, false, false, None, None
    )));
    assert!(usage(validate_password_modes(
        true, true, true, None, false, false, false, false, false, None, None
    )));
    assert!(matches!(
        validate_password_modes(
            true, false, false, None, false, false, false, false, false, None, None
        ),
        Ok(PasswordAction::Mode(PasswordMode::Set))
    ));
    assert!(matches!(
        validate_password_modes(
            false, true, false, None, false, false, false, false, false, None, None
        ),
        Ok(PasswordAction::Mode(PasswordMode::Change))
    ));
    assert!(matches!(
        validate_password_modes(
            false, false, true, None, false, false, false, false, false, None, None
        ),
        Ok(PasswordAction::Mode(PasswordMode::Remove))
    ));
}

#[test]
fn password_hint_and_email_conflict_rules() {
    assert!(usage(validate_password_modes(
        false,
        false,
        true,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        None
    )));
    assert!(usage(validate_password_modes(
        false,
        false,
        true,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        Some("me@example.com")
    )));
    assert!(usage(validate_password_modes(
        true,
        false,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("   "),
        None
    )));
    assert!(usage(validate_password_modes(
        false,
        true,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        Some("")
    )));
    assert!(validate_password_modes(
        true,
        false,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        Some("me@example.com")
    )
    .is_ok());
    assert!(validate_password_modes(
        false,
        true,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        None
    )
    .is_ok());
}

#[test]
fn password_state_matrix_reports_honest_blockers() {
    let err = plan_password_step(PasswordMode::Set, true).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)), "{err}");
    assert!(err.message().contains("--change"), "{err}");
    assert!(plan_password_step(PasswordMode::Set, false).is_ok());
    let err = plan_password_step(PasswordMode::Change, false).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)), "{err}");
    assert!(err.message().contains("no cloud password"), "{err}");
    assert!(plan_password_step(PasswordMode::Change, true).is_ok());
    let err = plan_password_step(PasswordMode::Remove, false).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)), "{err}");
    assert!(err.message().contains("no cloud password"), "{err}");
    assert!(plan_password_step(PasswordMode::Remove, true).is_ok());
}

const REF_SALT1_B64: &str = "X0g8OL0JhufNyVrhOO9PSblRwfgccT/s3vOvaSzsS0cWrJt3Chlevg==";
const REF_SALT2_B64: &str = "thb8a77fUREZxe00YpUn8Q==";
const REF_P_B64: &str = "xxyuucaxyQSObFIvcPE/c5gNQCOOPiHBSTTQN1Y9kw9IGYoKp8FAWCKUk9IlMPTb+jNvbgrJJROVQ67UTM58NyD9UfaUWHBaxozU/mtrE6vcl0ZRKWkyhFTxj6+MWV9kJHf+lrsqlB1bzR1KyMxJiAcI+ps3jjxPOpBgvuZ8+aSkppWBEFGQfhYnU7VrD2tBDbp02KhLKhSzFE4O8ShHVP0X7ZUNWWW0ud1GWC2xF40WnGvEZbDW/5yjko/vW5rk5Bj8Feg+vqD4f6n/Xu1wBQ3tKEn0e/lZ2VaFDOkphR8NgRX2NbEF7i5OFdBLJFS/b0+t8DSxBAMRnNjjuS/MWw==";
const REF_SRP_B_B64: &str = "k/cOvVD2QmrKJWh2lZn5HyTQEoTMUaRJ5i3MFSff5QEmsqpEI45PsjMUGe1K6/GgrhXgOr0Yv8UsprrsVkwTtaHS41d5mJd1e7c2/cLOsbVqrPGas1SNbZIqUi8LUfQBJMO8mTav8+Hc++o5rJrSrdxq8K0weDJ4u7hMqw7YRksP/rKwyTo5pdl9ugEFZyylR1Nz2NI+VKasm+2VGei+9PAHGfWtVhUb5VN2SN8vjj5yZctX+5SgVM4qgrjMZkrQYODWxt8YeTRUQOuXf6Dy028xolPYkXcy8TPUADOjS2FSlptgDVnNq/6iqyOTrWWeVtZuE2BbH2HkjjzWXA9YrA==";
const REF_RANDOM_A_B64: &str = "vzFXTzT84eU4kcWbf2JGigymgtqFhd+N4KGIczWXVfuxgYh4qe6Rm7HpTSDF8GB+AqMxdhmb8yICVsnqGmnzlaUV0gU52IzadQpSUvuGT1c/KwMvO0Z9CLNP2cidHF0GJ44RPlHU6JPBwCdFWvRlPwlmB6QVbZT7jh3HzuW/4yhQmC+UGuRBToO/It9WJwtDt8zETCbUCIZGTajjRKhUB5W49puPUIVSpyPNaTHh1pIE6OncBW8KKhCg15UeNV8+3vWl4YqQkSlaUeudsQuLDTBInI0pvAzYbpd4H14wxba/58r0qugbKC5lOsSKoaj954lyK8BPQyDNn4aEn+BcpA==";
const REF_M1_B64: &str = "TXr0EsWi57FUZzdr0Ri4U2BOaHsx9RxJgMTXwYdmE+M=";
const REF_G_A_B64: &str = "D6ErxlWxGHooKWpprlZdaCeC4M6wWgicDEHB3OmD3H9KU0NOp44GXZ4ctg5Ce0RopHgGCf66L1VlTuLv4K63Ltr94mUuJu1bTUuq2dKjgYBq9jQWv2Jj30WkPYW+VAG8Ij6/rAlCHK3dfiYL1rhlQhM8BI1s1Us42OLM32tVDodbE1OkrP4ykv+1ag9YsqOQJ8m/3ZH9TFMdI8d9bo99WD6u6jFt7d7WmTUpbOfqNOm+bvL72CnqxMm9ZG3BVj5H93uRQxygAs95/BSdloJyg1wVyhxrLF4DcSouG11S9eT6ocFssXf6/ZamqltLP0xkmRVkS2OFXPs4Vh/xf+37ig==";
const REF_PASSWORD: &str = "234567";

fn decode_b64(value: &str) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.decode(value).unwrap()
}

fn ref_srp_params() -> SrpParams {
    SrpParams {
        salt1: decode_b64(REF_SALT1_B64),
        salt2: decode_b64(REF_SALT2_B64),
        p: decode_b64(REF_P_B64),
        g: 3,
    }
}

#[test]
fn remove_srp_proof_matches_grammers_reference_vector() {
    let proof = input_check_password_srp(
        &ref_srp_params(),
        0x1234_5678_9abc_def0,
        &decode_b64(REF_SRP_B_B64),
        &decode_b64(REF_RANDOM_A_B64),
        REF_PASSWORD,
    )
    .unwrap();
    match proof {
        grammers_client::tl::enums::InputCheckPasswordSrp::Srp(srp) => {
            let grammers_client::tl::types::InputCheckPasswordSrp { srp_id, a, m1 } = srp;
            assert_eq!(srp_id, 0x1234_5678_9abc_def0);
            assert_eq!(decode_b64(REF_G_A_B64), a);
            assert_eq!(decode_b64(REF_M1_B64), m1);
        }
        other => panic!("expected Srp proof, got {other:?}"),
    }
}

#[test]
fn remove_srp_proof_rejects_unsupported_kdf_honestly() {
    use grammers_client::tl::enums;
    let err = password::extract_srp_params(None).unwrap_err();
    assert!(err.message().contains("unsupported"), "{err}");
    let err = password::extract_srp_params(Some(&enums::PasswordKdfAlgo::Unknown)).unwrap_err();
    assert!(err.message().contains("unsupported"), "{err}");
}

#[test]
fn remove_srp_proof_rejects_bad_prime_and_generator_without_panicking() {
    let mut params = ref_srp_params();
    params.p.truncate(128);
    let err = input_check_password_srp(
        &params,
        1,
        &decode_b64(REF_SRP_B_B64),
        &decode_b64(REF_RANDOM_A_B64),
        REF_PASSWORD,
    )
    .unwrap_err();
    assert!(err.message().contains("SRP"), "{err}");
    assert!(!err.message().contains(REF_PASSWORD), "{err}");

    let mut params = ref_srp_params();
    params.g = 99;
    let err = input_check_password_srp(
        &params,
        1,
        &decode_b64(REF_SRP_B_B64),
        &decode_b64(REF_RANDOM_A_B64),
        REF_PASSWORD,
    )
    .unwrap_err();
    assert!(err.message().contains("generator"), "{err}");
    assert!(!err.message().contains(REF_PASSWORD), "{err}");
}

#[test]
fn invalid_cloud_password_maps_to_auth_error_without_echoing_secret() {
    let e = grammers_client::InvocationError::Rpc(grammers_client::sender::RpcError {
        code: 400,
        name: "PASSWORD_HASH_INVALID".to_string(),
        value: None,
        caused_by: None,
    });
    let err = password::map_update_password_error(e);
    assert!(matches!(err, TeleError::Auth(_)), "{err}");
    assert_eq!(err.exit_code(), crate::error::EXIT_AUTH);
    assert!(err.message().contains("invalid cloud password"), "{err}");
    assert!(!err.message().contains(REF_PASSWORD), "{err}");
}

#[test]
fn other_update_password_errors_pass_through_taxonomy() {
    let e = grammers_client::InvocationError::Rpc(grammers_client::sender::RpcError {
        code: 420,
        name: "FLOOD_WAIT".to_string(),
        value: Some(9),
        caused_by: None,
    });
    let err = password::map_update_password_error(e);
    assert!(matches!(err, TeleError::Rpc(_, 420, _, Some(9))), "{err}");
}

fn export_args(name: &str, out: Option<&str>) -> ExportSessionArgs {
    ExportSessionArgs {
        name: name.to_string(),
        out: out.map(str::to_string),
    }
}

fn import_args(
    file: &str,
    as_name: Option<&str>,
    force: bool,
    from_telethon: bool,
) -> ImportSessionArgs {
    ImportSessionArgs {
        file: file.to_string(),
        as_name: as_name.map(str::to_string),
        force,
        from_telethon,
    }
}

fn seed_test_env(dir: &std::path::Path) {
    std::env::set_var("TELE_APP_DIR", dir);
    std::fs::write(
        dir.join("config.toml"),
        "[accounts.me]\n\
             [accounts.work]\n\
             [accounts.origin]\n\
             [accounts.restored]\n\
             [accounts.keyed]\n\
             [accounts.fromtg]\n\
             ",
    )
    .unwrap();
}

#[tokio::test]
async fn export_session_rejects_invalid_names_as_usage() {
    let dir = std::env::temp_dir().join(format!("telecli-exp-badname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags(
        "account export-session",
        false,
        false,
        &dir.join("config.toml"),
    );
    for bad in ["all", ".", "..", "../x", "a/b"] {
        let err = export_session(&export_args(bad, None), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn export_session_dry_run_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("telecli-exp-dry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out")).unwrap();
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    let flags = test_flags(
        "account export-session",
        false,
        true,
        &dir.join("config.toml"),
    );
    let code = export_session(&export_args("work", Some("out/w.session")), &flags)
        .await
        .unwrap();
    assert_eq!(code, 0);
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    assert!(
        !dir.join("sessions").exists(),
        "dry run must not touch sessions"
    );
    assert!(!dir.join("out").join("w.session").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn export_row_and_data_carry_contract_keys() {
    let exported = session::ExportedSession {
        account: "work".to_string(),
        path: std::path::PathBuf::from("/backup/work.session"),
        bytes: 4321,
        sha256: "ab".repeat(32),
    };
    let row = export_row(&exported);
    assert_eq!(row.len(), 4);
    assert_eq!(row[0], "work");
    assert_eq!(row[2], "4321");
    assert_eq!(row[3], format!("{}{}", "ab".repeat(31), "ab"));
    let data = export_data(&exported);
    assert_eq!(data["account"], serde_json::json!("work"));
    assert_eq!(data["bytes"], serde_json::json!(4321));
    assert_eq!(data["sha256"], serde_json::json!("ab".repeat(32)));
    assert!(data["path"].is_string());
}

#[test]
pub(crate) fn import_row_and_data_carry_resulting_account() {
    let imported = session::ImportedSession {
        account: "team_a".to_string(),
        path: std::path::PathBuf::from("sessions/team_a.session"),
        bytes: 8192,
    };
    let row = import_row(&imported);
    assert_eq!(
        row,
        vec![
            "team_a".to_string(),
            "sessions/team_a.session".to_string(),
            "8192".to_string(),
        ]
    );
    let data = import_data(&imported);
    assert_eq!(data["imported"], serde_json::json!(true));
    assert_eq!(data["account"], serde_json::json!("team_a"));
    assert_eq!(data["bytes"], serde_json::json!(8192));
}

#[tokio::test]
async fn import_session_rejects_invalid_as_name_before_disk() {
    let dir = std::env::temp_dir().join(format!("telecli-imp-badname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("src.session"), b"SQLite format 3\x00pad").unwrap();
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    let flags = test_flags(
        "account import-session",
        false,
        false,
        &dir.join("config.toml"),
    );
    for bad in ["all", ".", "..", "../escape", "has space"] {
        let err = import_session(&import_args("src.session", Some(bad), false, false), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
    }
    assert!(
        !dir.join("sessions").exists(),
        "invalid target names must create nothing"
    );
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn telethon_import_via_command_layer_converts_and_roundtrips() {
    use grammers_client::session::types::PeerId;
    use grammers_client::session::Session as _;

    let dir = std::env::temp_dir().join(format!("telecli-imp-tgok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("telethon.session");
    {
        let db = libsql::Builder::new_local(&src).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("CREATE TABLE version (number INTEGER PRIMARY KEY)", ())
            .await
            .unwrap();
        conn.execute("INSERT INTO version VALUES (6)", ())
            .await
            .unwrap();
        conn.execute(
                "CREATE TABLE sessions (dc_id INTEGER PRIMARY KEY, server_address TEXT, port INTEGER, auth_key BLOB, takeout_id BLOB, user_id INTEGER)",
                (),
            )
            .await
            .unwrap();
        conn.execute(
            &format!(
                "INSERT INTO sessions VALUES (2, '149.154.167.51', 443, X'{}', NULL, 8675309)",
                "5C".repeat(256)
            ),
            (),
        )
        .await
        .unwrap();
    }
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    let flags = test_flags(
        "account import-session",
        true,
        false,
        &dir.join("config.toml"),
    );
    let code = import_session(
        &import_args(src.to_str().unwrap(), None, false, true),
        &flags,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert!(session::session_path("telethon").exists());
    {
        let reopened = session::open_session("telethon").await.unwrap();
        assert_eq!(reopened.session.home_dc_id().unwrap(), 2);
        let option = reopened.session.dc_option(2).unwrap().expect("dc option");
        assert_eq!(option.auth_key, Some([0x5Cu8; 256]));
        let myself = reopened.session.peer(PeerId::self_user()).await.unwrap();
        match myself.expect("cached self user") {
            grammers_client::session::types::PeerInfo::User { id, .. } => {
                assert_eq!(id, 867_5309)
            }
            other => panic!("expected self user, got {other:?}"),
        }
    }
    let payload = import_data(&session::ImportedSession {
        account: "telethon".to_string(),
        path: session::session_path("telethon"),
        bytes: 4096,
    })
    .to_string();
    assert!(!payload.contains("auth_key"), "{payload}");
    assert!(!payload.contains("92,"), "{payload}");
    session::remove_session("telethon").await.unwrap();
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn telethon_import_rejects_foreign_schema_without_side_effects() {
    let dir = std::env::temp_dir().join(format!("telecli-imp-tgforeign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("telethon.session");
    let scaffold = grammers_client::session::storages::SqliteSession::open(&src)
        .await
        .unwrap();
    drop(scaffold);
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    let flags = test_flags(
        "account import-session",
        true,
        false,
        &dir.join("config.toml"),
    );
    let err = import_session(
        &import_args(src.to_str().unwrap(), Some("fromtg"), false, true),
        &flags,
    )
    .await
    .unwrap_err();
    let msg = err.message();
    assert!(msg.contains("sessions"), "{msg}");
    assert!(msg.to_lowercase().contains("not a telethon"), "{msg}");
    assert_eq!(err.exit_code(), crate::error::EXIT_ALL_FAILED);
    assert!(
        !dir.join("sessions").exists(),
        "rejected conversion must not scaffold anything"
    );
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn exported_payload_never_contains_auth_key_material() {
    use grammers_client::session::types::DcOption;
    use grammers_client::session::Session as _;
    use std::net::{SocketAddrV4, SocketAddrV6};

    let dir = std::env::temp_dir().join(format!("telecli-exp-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    {
        let locked = session::open_session("keyed").await.unwrap();
        locked
            .session
            .set_dc_option(&DcOption {
                id: 9,
                ipv4: SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 443),
                ipv6: SocketAddrV6::new(std::net::Ipv6Addr::LOCALHOST, 443, 0, 0),
                auth_key: Some([0xA7u8; 256]),
            })
            .await
            .unwrap();
        drop(locked);
    }
    let dest = dir.join("leak-probe.session");
    let exported = session::export_session("keyed", Some(&dest)).await.unwrap();
    let payload = export_data(&exported);
    assert_eq!(payload["sha256"].as_str().map(str::len), Some(64));
    let rendered = payload.to_string();
    assert!(
        !rendered.contains("a7a7a7a7"),
        "hex key material leaked into machine payload: {rendered}"
    );
    assert!(
        !rendered.contains("p6enp6en"),
        "base64 key material leaked into machine payload: {rendered}"
    );
    let _ = std::fs::remove_file(&dest);
    session::remove_session("keyed").await.unwrap();
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn import_then_export_via_commands_preserves_bytes() {
    let dir = std::env::temp_dir().join(format!("telecli-cmd-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inbox")).unwrap();
    let _guard = crate::config::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    seed_test_env(&dir);
    {
        let locked = session::open_session("origin").await.unwrap();
        drop(locked);
    }
    let backup = dir.join("inbox").join("origin.session");
    let json_flags = test_flags(
        "account export-session",
        true,
        false,
        &dir.join("config.toml"),
    );
    export_session(&export_args("origin", backup.to_str()), &json_flags)
        .await
        .unwrap();
    let import_flags = test_flags(
        "account import-session",
        true,
        false,
        &dir.join("config.toml"),
    );
    import_session(
        &import_args(backup.to_str().unwrap(), Some("restored"), false, false),
        &import_flags,
    )
    .await
    .unwrap();
    let reexport = dir.join("inbox").join("restored.session");
    export_session(&export_args("restored", reexport.to_str()), &json_flags)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        std::fs::read(&reexport).unwrap(),
        "command-level roundtrip must be byte identical"
    );
    session::remove_session("restored").await.unwrap();
    session::remove_session("origin").await.unwrap();
    std::env::remove_var("TELE_APP_DIR");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_login_stage_accepts_documented_modes_only() {
    assert_eq!(parse_login_stage("begin").unwrap(), LoginStage::Begin);
    assert_eq!(parse_login_stage("code").unwrap(), LoginStage::Code);
    assert_eq!(parse_login_stage("status").unwrap(), LoginStage::Status);
    assert_eq!(parse_login_stage("cancel").unwrap(), LoginStage::Cancel);
    for bad in ["", "restart", "BEGIN"] {
        assert!(
            matches!(parse_login_stage(bad), Err(TeleError::Usage(_))),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn staged_rejects_non_code_method_before_dry_run() {
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account login", false, true, &dir.join("config.toml"));
    let err = login(&staged_args("qr", None, Some("begin")), &flags)
        .await
        .unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn staged_unknown_value_is_usage() {
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account login", false, true, &dir.join("config.toml"));
    let err = login(
        &staged_args("code", Some("+15551234567"), Some("restart")),
        &flags,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)), "{err}");
    assert!(err.message().contains("--stage"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn staged_begin_requires_phone_and_writes_nothing() {
    with_tele_app_env(|dir| async move {
        let flags = test_flags("account login", false, false, &dir.join("config.toml"));
        let err = login(&staged_args("code", None, Some("begin")), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{err}");
        assert!(!dir.join("pending").exists());
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn staged_begin_dry_run_writes_no_state() {
    with_tele_app_env(|dir| async move {
        let flags = test_flags("account login", false, true, &dir.join("config.toml"));
        let code = login(
            &staged_args("code", Some("+15551234567"), Some("begin")),
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(!dir.join("pending").exists());
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn code_stage_requires_prior_begin_state() {
    with_pending_env(|dir| async move {
        let err = require_pending_under(&dir, "ghost").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{err}");
        assert!(err.message().contains("--stage begin"), "{err}");
    })
    .await;
}

#[tokio::test]
async fn status_reports_pending_with_redacted_phone() {
    with_pending_env(|dir| async move {
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        let loaded = load_pending_under(&dir, "work")
            .unwrap()
            .expect("pending must load");
        assert_eq!(loaded.phone_code_hash, "hash-token-123");
        let data = stage_status_data(Some(&loaded));
        assert_eq!(data["stage"], serde_json::json!("status"));
        assert_eq!(data["pending"], serde_json::json!(true));
        assert_eq!(data["phone"], serde_json::json!("+1***567"));
        assert!(!data["created_at"].as_str().unwrap_or_default().is_empty());
        remove_pending_under(&dir, "work").unwrap();
    })
    .await;
}

#[test]
fn status_without_pending_reports_absent() {
    let data = stage_status_data(None);
    assert_eq!(data["stage"], serde_json::json!("status"));
    assert_eq!(data["pending"], serde_json::json!(false));
}

#[tokio::test]
async fn cancel_discards_pending_then_reports_clean() {
    with_pending_env(|dir| async move {
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        assert!(pending_dir_under(&dir)
            .join(login_pending_file("work"))
            .exists());
        assert!(remove_pending_under(&dir, "work").unwrap());
        assert!(!pending_dir_under(&dir)
            .join(login_pending_file("work"))
            .exists());
        assert!(load_pending_under(&dir, "work").unwrap().is_none());
        assert!(!remove_pending_under(&dir, "work").unwrap());
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn pending_state_file_never_contains_secret_fields() {
    with_pending_env(|dir| async move {
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        let text =
            std::fs::read_to_string(pending_dir_under(&dir).join(login_pending_file("work")))
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<String> = value
            .as_object()
            .expect("state must be an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "account".to_string(),
                "created_at".to_string(),
                "phone".to_string(),
                "phone_code_hash".to_string(),
                "version".to_string(),
            ]
        );
        assert_eq!(value["version"], serde_json::json!(1));
        assert_eq!(
            value["phone_code_hash"],
            serde_json::json!("hash-token-123")
        );
        assert!(
            !text.contains("password"),
            "state leaked password key: {text}"
        );
        assert!(!text.contains("\"code\""), "state leaked code key: {text}");
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn pending_state_files_are_owner_restricted() {
    use std::os::unix::fs::PermissionsExt;
    with_pending_env(|dir| async move {
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        let file_mode = std::fs::metadata(pending_dir_under(&dir).join(login_pending_file("work")))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "pending state file must be owner-only");
        let dir_mode = std::fs::metadata(dir.join("pending"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "pending dir must be owner-only");
    })
    .await;
}

#[tokio::test]
async fn staged_status_via_login_needs_no_network() {
    with_tele_app_env(|dir| async move {
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        let flags = test_flags("account login", true, false, &dir.join("config.toml"));
        let code = login(&staged_args("code", None, Some("status")), &flags)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn ph2_deterministic_vector_matches_reference() {
    let pw = b"hello-ph2-deterministic";
    let s1 = b"fixed_salt1_32bytes_long_12345678";
    let s2 = b"fixed_salt2_16_!!";
    let out = ph2(pw, s1, s2);
    let expected = hex_to_bytes("4f42157ee0e0ddaeb1e9176c9337902b56241de35f2685fe605c9f3edae39228");
    assert_eq!(out.to_vec(), expected);
    assert_eq!(ph2(pw, s1, s2).to_vec(), expected);
    assert_ne!(ph2(b"other", s1, s2).to_vec(), expected);
}

#[test]
fn sh_is_salt_wrapped_sha256() {
    use sha2::{Digest, Sha256};
    let data = b"data";
    let salt = b"salt";
    let out = sh(data, salt);
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(data);
    hasher.update(salt);
    let expected: [u8; 32] = hasher.finalize().into();
    assert_eq!(out, expected);
}

#[test]
fn compute_new_password_hash_is_256_and_deterministic() {
    let salt1 = b"fixed_salt1_32bytes_long_12345678".to_vec();
    let extra = [0xABu8; 32];
    let extended = extend_salt1(&salt1, &extra);
    let salt2 = b"fixed_salt2_16_!!".to_vec();
    let p = decode_b64(REF_P_B64);
    let h1 =
        compute_new_password_hash("hello-ph2-deterministic", &extended, &salt2, 3, &p).unwrap();
    let h2 =
        compute_new_password_hash("hello-ph2-deterministic", &extended, &salt2, 3, &p).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 256);
    assert!(h1.iter().any(|&b| b != 0));
    let h3 = compute_new_password_hash("different", &extended, &salt2, 3, &p).unwrap();
    assert_ne!(h1, h3);
}

#[test]
fn new_password_algo_and_hash_extends_salt1() {
    let base = grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
            salt1: b"base_salt1_".to_vec(),
            salt2: b"salt2_fixed".to_vec(),
            g: 3,
            p: decode_b64(REF_P_B64),
        };
    let extra = [0x11u8; 32];
    let (algo, hash) = new_password_algo_and_hash("pwd", &base, Some(extra)).unwrap();
    match algo {
            grammers_client::tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(inner) => {
                assert_eq!(inner.salt1.len(), base.salt1.len() + 32);
                assert_eq!(&inner.salt1[base.salt1.len()..], &extra);
                assert_eq!(inner.salt2, base.salt2);
            }
            _ => panic!("wrong algo"),
        }
    assert_eq!(hash.len(), 256);
}

#[test]
fn password_hint_email_validation_matrix() {
    assert!(validate_password_modes(
        true,
        false,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        Some("a@b.com")
    )
    .is_ok());
    assert!(validate_password_modes(
        false,
        true,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        None
    )
    .is_ok());
    assert!(validate_password_modes(
        false,
        true,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        Some("a@b.com")
    )
    .is_ok());
    assert!(validate_password_modes(
        false,
        true,
        false,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        Some("a@b.com")
    )
    .is_ok());
    assert!(validate_password_modes(
        true, false, false, None, false, false, false, false, false, None, None
    )
    .is_ok());
    let err = validate_password_modes(
        false,
        false,
        true,
        None,
        false,
        false,
        false,
        false,
        false,
        Some("hint"),
        None,
    )
    .unwrap_err();
    assert!(err.message().contains("--hint"), "{err}");
    let err = validate_password_modes(
        false,
        false,
        true,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        Some("a@b.com"),
    )
    .unwrap_err();
    assert!(err.message().contains("--recovery-email"), "{err}");
}

#[test]
fn password_email_and_lifecycle_mode_matrix() {
    type FlagSet = (
        bool,
        bool,
        bool,
        Option<&'static str>,
        bool,
        bool,
        bool,
        bool,
        bool,
    );
    let one_each: [FlagSet; 6] = [
        (
            false,
            false,
            false,
            Some("123456"),
            false,
            false,
            false,
            false,
            false,
        ),
        (false, false, false, None, true, false, false, false, false),
        (false, false, false, None, false, true, false, false, false),
        (false, false, false, None, false, false, true, false, false),
        (false, false, false, None, false, false, false, true, false),
        (false, false, false, None, false, false, false, false, true),
    ];
    for (set, change, remove, confirm, resend, cancel, status, reset_start, decline) in one_each {
        assert!(
            validate_password_modes(
                set,
                change,
                remove,
                confirm,
                resend,
                cancel,
                status,
                reset_start,
                decline,
                None,
                None
            )
            .is_ok(),
            "single primary flag must pass"
        );
    }
    assert!(validate_password_modes(
        true, false, false, None, true, false, false, false, false, None, None
    )
    .is_err());
    assert!(validate_password_modes(
        false, false, false, None, false, false, false, false, false, None, None
    )
    .is_err());
    assert!(validate_password_modes(
        true,
        false,
        false,
        Some("   "),
        false,
        false,
        false,
        false,
        false,
        None,
        None
    )
    .is_err());
    let a = validate_password_modes(
        false,
        false,
        false,
        Some(" 123 "),
        false,
        false,
        false,
        false,
        false,
        None,
        None,
    )
    .unwrap();
    match a {
        PasswordAction::ConfirmEmail(code) => assert_eq!(code, "123"),
        other => panic!("expected confirm email action, got {other:?}"),
    }
}

#[test]
fn password_action_describe_covers_all_variants() {
    for (action, needle) in [
        (PasswordAction::Mode(PasswordMode::Set), "set"),
        (PasswordAction::Mode(PasswordMode::Change), "change"),
        (PasswordAction::Mode(PasswordMode::Remove), "remove"),
        (PasswordAction::ConfirmEmail("1".into()), "confirm"),
        (PasswordAction::ResendEmail, "resend"),
        (PasswordAction::CancelEmail, "cancel"),
        (PasswordAction::Status, "state"),
        (PasswordAction::ResetStart, "reset countdown"),
        (
            PasswordAction::DeclineReset,
            "cancel pending password reset",
        ),
    ] {
        assert!(
            action.describe().contains(needle),
            "{needle}: {}",
            action.describe()
        );
    }
}

#[test]
fn secret_never_in_ph2_error_output() {
    let pw = "super_secret_123";
    let salt1 = b"s1".to_vec();
    let salt2 = b"s2".to_vec();
    let p = decode_b64(REF_P_B64);
    let mut bad_p = p.clone();
    bad_p.truncate(128);
    let bad_params = SrpParams {
        salt1: salt1.clone(),
        salt2: salt2.clone(),
        p: bad_p,
        g: 3,
    };
    let err = input_check_password_srp(
        &bad_params,
        1,
        &decode_b64(REF_SRP_B_B64),
        &decode_b64(REF_RANDOM_A_B64),
        pw,
    )
    .unwrap_err();
    assert!(!err.message().contains(pw), "{err}");
    let bad_g = SrpParams {
        salt1,
        salt2,
        p,
        g: 99,
    };
    let err = input_check_password_srp(
        &bad_g,
        1,
        &decode_b64(REF_SRP_B_B64),
        &decode_b64(REF_RANDOM_A_B64),
        pw,
    )
    .unwrap_err();
    assert!(!err.message().contains(pw), "{err}");
}

#[tokio::test]
async fn password_dry_run_does_not_prompt_and_hides_secrets() {
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
    let mut flags = test_flags("account password", true, true, &dir.join("config.toml"));
    flags.account = vec!["work".to_string()];
    let args = PasswordArgs {
        set: true,
        change: false,
        remove: false,
        confirm_email: None,
        resend_email: false,
        cancel_email: false,
        status: false,
        reset_start: false,
        decline_reset: false,
        hint: Some("hintval".to_string()),
        recovery_email: Some("e@example.com".to_string()),
    };
    let code = password(&args, &flags).await.unwrap();
    assert_eq!(code, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn password_dry_run_change_honest_would() {
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
    let mut flags = test_flags("account password", true, true, &dir.join("config.toml"));
    flags.account = vec!["work".to_string()];
    let args = PasswordArgs {
        set: false,
        change: true,
        remove: false,
        confirm_email: None,
        resend_email: false,
        cancel_email: false,
        status: false,
        reset_start: false,
        decline_reset: false,
        hint: None,
        recovery_email: None,
    };
    let code = password(&args, &flags).await.unwrap();
    assert_eq!(code, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

fn ttl_args(get: bool, set: bool, days: Option<i64>) -> TtlArgs {
    TtlArgs { get, set, days }
}

#[test]
fn ttl_mode_matrix_rejects_conflicts_and_bounds() {
    assert!(usage(validate_ttl(false, false, None)));
    assert!(usage(validate_ttl(true, true, Some(30))));
    assert!(usage(validate_ttl(false, true, None)));
    for bad in [0, 366, -5] {
        let err = validate_ttl(false, true, Some(bad)).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad}: {err}");
    }
    assert!(usage(validate_ttl(true, false, Some(30))));
    assert_eq!(
        validate_ttl(false, true, Some(1)).unwrap(),
        TtlAction::Set(1)
    );
    assert_eq!(
        validate_ttl(false, true, Some(365)).unwrap(),
        TtlAction::Set(365)
    );
    assert_eq!(
        validate_ttl(false, true, Some(180)).unwrap(),
        TtlAction::Set(180)
    );
    assert_eq!(validate_ttl(true, false, None).unwrap(), TtlAction::Get);
}

#[test]
fn ttl_row_carries_days_key() {
    assert_eq!(ttl_data(42), serde_json::json!({"ttl_days": 42}));
    assert_eq!(
        ttl_set_data(180),
        serde_json::json!({"updated": true, "ttl_days": 180})
    );
}

#[tokio::test]
async fn ttl_dry_run_via_command_is_offline_noop() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let mut flags = test_flags("account ttl", true, true, &dir.join("config.toml"));
        flags.account = vec!["work".to_string()];
        let code = ttl(&ttl_args(false, true, Some(90)), &flags).await.unwrap();
        assert_eq!(code, 0);
        let code = ttl(&ttl_args(true, false, None), &flags).await.unwrap();
        assert_eq!(code, 0);
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

fn delete_args(reason: &str, yes: bool) -> DeleteArgs {
    DeleteArgs {
        reason: reason.to_string(),
        yes,
    }
}

#[test]
fn delete_reason_must_not_be_blank() {
    for bad in ["", "   "] {
        let err = validate_delete_reason(bad).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
    }
    validate_delete_reason("Delete my account").unwrap();
}

#[tokio::test]
async fn delete_without_yes_prints_would_and_exits_usage() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let mut flags = test_flags("account delete", true, false, &dir.join("config.toml"));
        flags.account = vec!["work".to_string()];
        let err = delete(&delete_args("Delete my account", false), &flags)
            .await
            .unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{err}");
        assert!(err.message().contains("--yes"), "{err}");
        assert!(err.message().contains("would"), "{err}");
        assert!(err.message().contains("work"), "{err}");
    })
    .await;
}

#[tokio::test]
async fn delete_requires_explicit_account_selection() {
    let dir = staged_temp_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = test_flags("account delete", false, false, &dir.join("config.toml"));
    let err = delete(&delete_args("bye", true), &flags).await.unwrap_err();
    assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{err}");
    assert!(err.message().contains("--account"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delete_dry_run_with_yes_is_offline_noop() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let mut flags = test_flags("account delete", true, true, &dir.join("config.toml"));
        flags.account = vec!["work".to_string()];
        let code = delete(&delete_args("Delete my account", true), &flags)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[test]
fn delete_dry_run_row_carries_would_and_reason() {
    let row = delete_dry_run_data("work", "spam");
    assert_eq!(row["dry_run"], serde_json::json!(true));
    assert_eq!(row["reason"], serde_json::json!("spam"));
    let would = row["would"].as_str().unwrap();
    assert!(would.contains("permanently delete"), "{would}");
    assert!(would.contains("work"), "{would}");
    assert!(!would.contains("password"), "{would}");
}

#[test]
fn sessions_parse_bool_accepts_true_false_only() {
    assert!(parse_bool_arg("true", "--disable-encrypted").unwrap());
    assert!(!parse_bool_arg("FALSE", "--disable-encrypted").unwrap());
    for bad in ["1", "", "yes", "maybe"] {
        assert!(
            matches!(
                parse_bool_arg(bad, "--disable-encrypted"),
                Err(TeleError::Usage(_))
            ),
            "{bad:?}"
        );
    }
}

fn base_sessions_args() -> SessionsArgs {
    SessionsArgs {
        terminate: None,
        web: false,
        terminate_web: None,
        terminate_all_web: false,
        change_flags: None,
        disable_encrypted: None,
        disable_call_requests: None,
    }
}

#[test]
fn sessions_mode_matrix_rejects_conflicting_primaries() {
    let ok_list = validate_sessions_modes(&base_sessions_args()).unwrap();
    assert!(matches!(ok_list, SessionsMode::List));
    let mut web_only = base_sessions_args();
    web_only.web = true;
    assert!(matches!(
        validate_sessions_modes(&web_only).unwrap(),
        SessionsMode::ListWeb
    ));
    let mut term_web = base_sessions_args();
    term_web.terminate_web = Some(7);
    assert!(matches!(
        validate_sessions_modes(&term_web).unwrap(),
        SessionsMode::TerminateWeb(7)
    ));
    let mut all_web = base_sessions_args();
    all_web.terminate_all_web = true;
    assert!(matches!(
        validate_sessions_modes(&all_web).unwrap(),
        SessionsMode::TerminateAllWeb
    ));
    let mut term = base_sessions_args();
    term.terminate = Some(9);
    assert!(matches!(
        validate_sessions_modes(&term).unwrap(),
        SessionsMode::Terminate(9)
    ));
    let mut web_plus_term = term.clone();
    web_plus_term.web = true;
    assert!(usage(validate_sessions_modes(&web_plus_term)));
    let mut double = base_sessions_args();
    double.terminate = Some(1);
    double.terminate_all_web = true;
    assert!(usage(validate_sessions_modes(&double)));
    let mut triple = double.clone();
    triple.terminate_web = Some(2);
    assert!(usage(validate_sessions_modes(&triple)));
}

#[test]
fn sessions_change_flags_needs_at_least_one_toggle() {
    let mut none = base_sessions_args();
    none.change_flags = Some(5);
    assert!(usage(validate_sessions_modes(&none)));
    let mut both = none.clone();
    both.disable_encrypted = Some("true".into());
    both.disable_call_requests = Some("false".into());
    match validate_sessions_modes(&both).unwrap() {
        SessionsMode::ChangeFlags {
            hash,
            encrypted_requests,
            call_requests,
        } => {
            assert_eq!(hash, 5);
            assert_eq!(encrypted_requests, Some(true));
            assert_eq!(call_requests, Some(false));
        }
        other => panic!("expected ChangeFlags, got {other:?}"),
    }
    let mut bad_value = none.clone();
    bad_value.disable_encrypted = Some("1".into());
    assert!(usage(validate_sessions_modes(&bad_value)));
    assert!(base_sessions_args().disable_encrypted.is_none());
}

#[test]
fn sessions_toggles_require_change_flags_hash() {
    let mut orphan = base_sessions_args();
    orphan.disable_call_requests = Some("true".into());
    assert!(usage(validate_sessions_modes(&orphan)));
    let mut orphan_pair = base_sessions_args();
    orphan_pair.disable_encrypted = Some("false".into());
    orphan_pair.disable_call_requests = Some("true".into());
    assert!(usage(validate_sessions_modes(&orphan_pair)));
}

fn web_fixture(hash: i64) -> grammers_client::tl::types::WebAuthorization {
    grammers_client::tl::types::WebAuthorization {
        hash,
        bot_id: 4242,
        domain: "web.example.org".to_string(),
        browser: "Chrome 126".to_string(),
        platform: "Linux".to_string(),
        date_created: 1_700_000_100,
        date_active: 1_750_000_200,
        ip: "198.51.100.9".to_string(),
        region: "".to_string(),
    }
}

#[test]
pub(crate) fn web_authorization_row_fixture_shapes_expected_keys() {
    let row = web_authorization_row(&web_fixture(1_234_567_890_124));
    assert_eq!(row["hash"], serde_json::json!(1_234_567_890_124_i64));
    assert_eq!(row["bot_id"], serde_json::json!(4242));
    assert_eq!(row["domain"], serde_json::json!("web.example.org"));
    assert_eq!(row["browser"], serde_json::json!("Chrome 126"));
    assert_eq!(row["platform"], serde_json::json!("Linux"));
    assert_eq!(row["ip"], serde_json::json!("198.51.100.9"));
    assert_eq!(row["region"], serde_json::json!(""));
    let created = chrono::DateTime::from_timestamp(1_700_000_100, 0)
        .unwrap()
        .to_rfc3339();
    let active = chrono::DateTime::from_timestamp(1_750_000_200, 0)
        .unwrap()
        .to_rfc3339();
    assert_eq!(row["date_created"], serde_json::json!(created));
    assert_eq!(row["date_active"], serde_json::json!(active));
    assert!(row.get("device_model").is_none());
}

#[test]
pub(crate) fn web_table_cells_flatten_specified_columns() {
    let row = web_authorization_row(&web_fixture(8));
    let cells = web_table_cells("work", &row);
    assert_eq!(cells.len(), 8);
    assert_eq!(cells[0], "work");
    assert_eq!(cells[1], "8");
    assert_eq!(cells[2], "web.example.org");
    assert!(cells.last().unwrap().len() > 10);
    let empty = web_table_cells("home", &serde_json::json!({}));
    assert_eq!(empty[0], "home");
    assert_eq!(empty[1], "null");
    assert!(empty.iter().skip(2).all(|c| c.is_empty()));
}

#[test]
fn web_decision_rejects_unknown_hash_only() {
    let rows = vec![web_fixture(111)];
    assert!(usage(web_hash_decision(999, &rows)));
    assert!(web_hash_decision(111, &rows).is_ok());
}

fn phone_args(
    change: Option<&str>,
    flashcall: bool,
    code: Option<&str>,
    hash: Option<&str>,
) -> PhoneArgs {
    PhoneArgs {
        change_phone: change.map(str::to_string),
        allow_flashcall: flashcall,
        confirm_code: code.map(str::to_string),
        phone_hash: hash.map(str::to_string),
    }
}

#[test]
fn phone_action_validation_matrix() {
    let none = validate_phone_modes(&phone_args(None, false, None, None)).unwrap_err();
    assert!(matches!(none, TeleError::Usage(_)));
    assert!(usage(validate_phone_modes(&phone_args(
        Some("+15550001111"),
        false,
        Some("12345"),
        Some("h")
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        None,
        false,
        Some("123"),
        None
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        Some(""),
        false,
        None,
        None
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        Some("   "),
        false,
        None,
        None
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        None,
        true,
        Some("123"),
        Some("h")
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        None,
        false,
        Some("123"),
        Some("")
    ))));
    assert!(usage(validate_phone_modes(&phone_args(
        None,
        false,
        Some("   "),
        Some("h")
    ))));
    match validate_phone_modes(&phone_args(Some(" +15550001111 "), true, None, None)).unwrap() {
        PhoneAction::Send { phone, flashcall } => {
            assert_eq!(phone, "+15550001111");
            assert!(flashcall);
        }
        other => panic!("expected Send, got {other:?}"),
    }
    match validate_phone_modes(&phone_args(None, false, Some(" 54321 "), Some(" hash "))).unwrap() {
        PhoneAction::Confirm { code, hash } => {
            assert_eq!(code, "54321");
            assert_eq!(hash, "hash");
        }
        other => panic!("expected Confirm, got {other:?}"),
    }
}

#[tokio::test]
async fn phone_pending_roundtrip_mismatch_and_missing_state() {
    with_pending_env(|dir| async move {
        let missing = require_pending_phone_under(&dir, "ghost").unwrap_err();
        assert!(matches!(missing, TeleError::Usage(_)), "{missing}");
        assert!(missing.message().contains("--change-phone"), "{missing}");
        let pending = PendingPhone::new("work", "+15551234567", "hash-a".to_string());
        save_pending_phone_under(&dir, &pending).unwrap();
        let loaded = require_pending_phone_under(&dir, "work").unwrap();
        assert_eq!(loaded.phone_code_hash, "hash-a");
        assert!(phone_hash_matches(&loaded, "hash-a"));
        assert!(!phone_hash_matches(&loaded, "hash-b"));
        assert!(remove_pending_phone_under(&dir, "work").unwrap());
        assert!(!remove_pending_phone_under(&dir, "work").unwrap());
    })
    .await;
}

#[test]
fn phone_dry_run_send_redacts_target_number() {
    let send = phone_dry_run_data(
        "work",
        &PhoneAction::Send {
            phone: "+15551234567".to_string(),
            flashcall: true,
        },
    );
    assert_eq!(send["dry_run"], serde_json::json!(true));
    let would = send["would"].as_str().unwrap();
    assert!(would.contains("+1***567"), "{would}");
    assert!(!would.contains("+15551234567"), "{would}");
    assert_eq!(send["flashcall"], serde_json::json!(true));
    let confirm = phone_dry_run_data(
        "work",
        &PhoneAction::Confirm {
            code: "54321".to_string(),
            hash: "hash-a".to_string(),
        },
    );
    let would = confirm["would"].as_str().unwrap();
    assert!(would.contains("confirm"), "{would}");
    assert!(would.contains("hash-a"), "{would}");
}

#[test]
fn parse_login_stage_supports_resend_and_cancel_code() {
    assert_eq!(parse_login_stage("resend").unwrap(), LoginStage::Resend);
    assert_eq!(
        parse_login_stage("cancel-code").unwrap(),
        LoginStage::CancelCode
    );
    assert_eq!(LoginStage::Resend.as_str(), "resend");
    assert_eq!(LoginStage::CancelCode.as_str(), "cancel-code");
    assert!(usage(parse_login_stage("resendnow")));
    assert!(usage(parse_login_stage("CANCEL-CODE")));
}

#[test]
fn stage_help_documents_local_vs_server_cancel() {
    assert!(STAGE_HELP.contains("resend"));
    assert!(STAGE_HELP.contains("cancel-code"));
    assert!(STAGE_HELP.contains("local"));
    assert!(STAGE_HELP.contains("server"));
}

#[tokio::test]
async fn resend_stage_without_pending_state_is_usage() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let flags = test_flags("account login", false, false, &dir.join("config.toml"));
        let err = login(&staged_args("code", None, Some("resend")), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{err}");
        assert!(err.message().contains("--stage begin"), "{err}");
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn cancel_code_stage_without_pending_state_is_usage() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let flags = test_flags("account login", false, false, &dir.join("config.toml"));
        let err = login(&staged_args("code", None, Some("cancel-code")), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{err}");
        assert!(err.message().contains("--stage begin"), "{err}");
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn staged_resend_dry_run_writes_no_state() {
    with_tele_app_env(|dir| async move {
        std::fs::write(dir.join("config.toml"), "[accounts.work]\n").unwrap();
        let flags = test_flags("account login", false, true, &dir.join("config.toml"));
        let code = login(
            &staged_args("code", Some("+15551234567"), Some("resend")),
            &flags,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(!dir.join("pending").exists());
        assert!(!dir.join("sessions").exists());
    })
    .await;
}

#[tokio::test]
async fn pending_login_document_io_roundtrip() {
    with_pending_env(|dir| async move {
        let file = login_pending_file("work");
        assert!(load_pending_document_under(&dir, &file).unwrap().is_none());
        save_pending_under(&dir, &pending_fixture("work")).unwrap();
        let text = load_pending_document_under(&dir, &file)
            .unwrap()
            .expect("document must exist");
        assert!(text.contains("hash-token-123"));
        assert!(remove_pending_document_under(&dir, &file).unwrap());
        assert!(load_pending_under(&dir, "work").unwrap().is_none());
    })
    .await;
}

fn plan_acct_op(op: &str, params: serde_json::Value) -> Result<Plan, serde_json::Value> {
    let route = account_serve_routes()
        .into_iter()
        .find(|r| r.op == op)
        .unwrap_or_else(|| panic!("route missing for {op}"));
    (route.planner)(op, params)
}

fn expect_acct_dry(plan: Plan, expected: serde_json::Value) {
    match plan {
        Plan::DryRun(data) => assert_eq!(data, expected),
        other => panic!("expected dry run plan, got {other:?}"),
    }
}

type AcctRouteLock = (&'static str, Lane, Option<u64>, bool, bool, bool);

#[test]
fn account_serve_lane_hints_table_is_locked() {
    let expected: &[AcctRouteLock] = &[
        (
            "account sessions list",
            Lane::Read,
            Some(120),
            true,
            false,
            true,
        ),
        (
            "account sessions web",
            Lane::Read,
            Some(120),
            true,
            false,
            true,
        ),
        ("account status", Lane::Read, Some(120), true, false, true),
        ("account ttl get", Lane::Read, Some(120), true, false, true),
        (
            "account ttl set",
            Lane::Mutate,
            Some(30),
            false,
            false,
            true,
        ),
    ];
    let routes = account_serve_routes();
    assert_eq!(routes.len(), expected.len());
    let mut names: Vec<&str> = routes.iter().map(|r| r.op).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "account sessions list",
            "account sessions web",
            "account status",
            "account ttl get",
            "account ttl set"
        ]
    );
    for (op, lane, secs, read_only, destructive, retry_safe) in expected {
        let route = routes.iter().find(|r| r.op == *op).unwrap();
        assert_eq!(route.lane, *lane, "lane for {op}");
        assert_eq!(
            route.timeout,
            secs.map(std::time::Duration::from_secs),
            "timeout for {op}"
        );
        assert_eq!(route.read_only, *read_only, "read_only for {op}");
        assert_eq!(route.destructive, *destructive, "destructive for {op}");
        assert_eq!(route.retry_safe, *retry_safe, "retry_safe for {op}");
        assert!(!route.summary.is_empty(), "summary for {op}");
        assert!(
            route.summary.chars().next().unwrap().is_ascii_lowercase(),
            "summary case for {op}"
        );
    }
}

#[test]
fn account_reads_accept_empty_params_and_pass_through() {
    for op in [
        "account status",
        "account ttl get",
        "account sessions list",
        "account sessions web",
    ] {
        let raw = serde_json::json!({});
        match plan_acct_op(op, raw.clone()).unwrap() {
            Plan::Execute(passed) => assert_eq!(passed, raw, "{op}"),
            other => panic!("{op}: expected execute plan, got {other:?}"),
        }
    }
    let raw = serde_json::json!({"days": 180});
    match plan_acct_op("account ttl set", raw.clone()).unwrap() {
        Plan::Execute(passed) => assert_eq!(passed, raw),
        other => panic!("expected execute plan, got {other:?}"),
    }
}

#[test]
fn account_ops_reject_unknown_fields_with_param_key() {
    for op in [
        "account status",
        "account ttl get",
        "account sessions list",
        "account sessions web",
    ] {
        let err = plan_acct_op(op, serde_json::json!({"dayz": 1})).unwrap_err();
        assert_eq!(err["type"], "ServeError", "{op}: {err}");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("unknown field"), "{op}: {msg}");
        assert!(msg.contains("dayz"), "{op}: {msg}");
        assert_eq!(err["param"], "dayz", "{op}");
    }
    let err = plan_acct_op(
        "account ttl set",
        serde_json::json!({"days": 30, "dayz": 31}),
    )
    .unwrap_err();
    assert_eq!(err["type"], "ServeError", "{err}");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.contains("unknown field"), "{msg}");
    assert!(msg.contains("dayz"), "{msg}");
    assert_eq!(err["param"], "dayz");
}

#[test]
fn account_ttl_set_param_errors_name_days_field() {
    let err = plan_acct_op("account ttl set", serde_json::json!({})).unwrap_err();
    assert_eq!(err["type"], "ServeError");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.contains("missing field"), "{msg}");
    assert!(msg.contains("days"), "{msg}");
    assert_eq!(err["param"], "days");

    let err = plan_acct_op("account ttl set", serde_json::json!({"days": "soon"})).unwrap_err();
    assert_eq!(err["type"], "ServeError");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.contains("invalid type"), "{msg}");
    assert!(msg.contains("i64"), "{msg}");
    assert_eq!(err["param"], "days");
}

#[test]
fn account_ttl_set_rejects_out_of_range_days_as_usage() {
    for bad in [0i64, 366, -5, 4_000_000_000] {
        let err = plan_acct_op("account ttl set", serde_json::json!({"days": bad})).unwrap_err();
        assert_eq!(err["type"], "UsageError", "{bad}: {err}");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("--days must be between 1 and 365"), "{msg}");
    }
    for good in [1i64, 90, 365] {
        plan_acct_op("account ttl set", serde_json::json!({"days": good})).unwrap();
    }
}

#[test]
fn account_dry_run_payloads_echo_shape_without_network() {
    let plan = plan_acct_op("account status", serde_json::json!({"dry_run": true})).unwrap();
    expect_acct_dry(
        plan,
        serde_json::json!({
            "dry_run": true,
            "would": "probe authorization status"
        }),
    );

    let plan = plan_acct_op("account ttl get", serde_json::json!({"dry_run": true})).unwrap();
    expect_acct_dry(
        plan,
        serde_json::json!({
            "dry_run": true,
            "would": "show the inactive-account TTL"
        }),
    );

    let plan = plan_acct_op(
        "account ttl set",
        serde_json::json!({"days": 90, "dry_run": true}),
    )
    .unwrap();
    expect_acct_dry(
        plan,
        serde_json::json!({
            "dry_run": true,
            "days": 90,
            "would": "set the inactive-account TTL to 90 days"
        }),
    );

    let plan = plan_acct_op(
        "account sessions list",
        serde_json::json!({"dry_run": true}),
    )
    .unwrap();
    expect_acct_dry(
        plan,
        serde_json::json!({
            "dry_run": true,
            "would": "list device sessions"
        }),
    );

    let plan = plan_acct_op("account sessions web", serde_json::json!({"dry_run": true})).unwrap();
    expect_acct_dry(
        plan,
        serde_json::json!({
            "dry_run": true,
            "would": "list web login sessions"
        }),
    );
}

#[test]
fn account_ttl_get_rejects_non_object_params_shape() {
    let err = plan_acct_op("account ttl get", serde_json::json!([1])).unwrap_err();
    assert_eq!(err["type"], "ServeError");
    assert!(err.get("param").is_none(), "{}", err);
}

#[test]
fn ttl_set_schema_declares_required_days_only() {
    let s = crate::commands::serve::params_schema::<TtlSetParams>();
    assert_eq!(s["type"], "object");
    assert_eq!(s["additionalProperties"], serde_json::Value::Bool(false));
    for prop in ["days", "dry_run"] {
        assert!(s["properties"][prop].is_object(), "{prop}");
    }
    let required: Vec<&str> = s["required"]
        .as_array()
        .expect("required array")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    assert_eq!(required, vec!["days"]);
}

#[test]
fn status_schema_is_empty_object_with_no_required_fields() {
    let s = crate::commands::serve::params_schema::<AccountStatusParams>();
    assert_eq!(s["type"], "object");
    assert_eq!(s["additionalProperties"], serde_json::Value::Bool(false));
    assert!(s["properties"]["dry_run"].is_object());
    assert!(s
        .get("required")
        .is_none_or(|r| r.as_array().unwrap().is_empty()));
}
