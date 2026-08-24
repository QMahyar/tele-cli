use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds;
use crate::config;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, select_sessions, GlobalFlags};
use crate::output::{self, log_line, AccountOutcome, Envelope};
use crate::session;
use clap::{Args, Subcommand};
use hmac::Hmac;
use num_bigint::BigUint;
use sha2::{Digest, Sha256, Sha512};
use std::io::{IsTerminal, Write};
use std::sync::Arc;

pub(crate) fn redact_phone(phone: &str) -> String {
    let phone = phone.trim();
    if phone.len() <= 6 {
        return phone.to_string();
    }
    let prefix_len = phone.chars().take_while(|c| !c.is_ascii_digit()).count();
    let digits: Vec<char> = phone.chars().skip(prefix_len).collect();
    if digits.len() <= 6 {
        return phone.to_string();
    }
    let prefix: String = phone.chars().take(prefix_len + 1).collect();
    let suffix: String = digits.iter().rev().take(3).rev().collect();
    format!("{}***{}", prefix, suffix)
}

#[derive(Subcommand)]
pub enum AccountCmd {
    List,
    Status,
    Add(AddArgs),
    Login(LoginArgs),
    Logout(LogoutArgs),
    Remove(RemoveArgs),
    Sessions(SessionsArgs),
    Password(PasswordArgs),
    ExportSession(ExportSessionArgs),
    ImportSession(ImportSessionArgs),
    Ttl(TtlArgs),
    Delete(DeleteArgs),
    Phone(PhoneArgs),
}
#[derive(Args)]
pub struct AddArgs {
    #[arg(long)]
    name: String,
    #[arg(long, value_delimiter = ',')]
    tags: Option<Vec<String>>,
}
#[derive(Args)]
pub struct LoginArgs {
    #[arg(long)]
    name: String,
    #[arg(long, default_value = "code")]
    method: String,
    #[arg(long)]
    phone: Option<String>,
    #[arg(long, help = "print the QR login URI to stderr if QR rendering fails")]
    show_token: bool,
    #[arg(long, default_value_t = DEFAULT_QR_TIMEOUT_SECS, help = "overall QR login deadline in seconds")]
    qr_timeout_secs: u64,
    #[arg(long, help = STAGE_HELP)]
    stage: Option<String>,
}

const STAGE_HELP: &str = "run code login stepwise across invocations: begin | code | status | cancel | resend | cancel-code (cancel discards the local pending state only; cancel-code also asks the server to invalidate the sent code)";

const DEFAULT_QR_TIMEOUT_SECS: u64 = 300;
const TELE_PHONE_ENV: &str = "TELE_PHONE";
const MAX_CODE_ATTEMPTS: usize = 3;
const MAX_PASSWORD_ATTEMPTS: usize = 3;
#[derive(Args, Clone)]
pub struct SessionsArgs {
    #[arg(long, help = "terminate the device session with this hash")]
    terminate: Option<i64>,
    #[arg(long, help = "list web login sessions instead of device sessions")]
    web: bool,
    #[arg(
        long,
        value_name = "HASH",
        help = "terminate the web login session with this hash"
    )]
    terminate_web: Option<i64>,
    #[arg(long, help = "terminate every web login session on this account")]
    terminate_all_web: bool,
    #[arg(
        long,
        value_name = "HASH",
        help = "update per-web-session settings; combine with --disable-encrypted or --disable-call-requests"
    )]
    change_flags: Option<i64>,
    #[arg(
        long,
        value_name = "TRUE|FALSE",
        help = "with --change-flags: disable encrypted requests for that web session"
    )]
    disable_encrypted: Option<String>,
    #[arg(
        long,
        value_name = "TRUE|FALSE",
        help = "with --change-flags: disable call requests for that web session"
    )]
    disable_call_requests: Option<String>,
}
#[derive(Args)]
pub struct PasswordArgs {
    #[arg(long, help = "set a new cloud password")]
    set: bool,
    #[arg(long, help = "change the existing cloud password")]
    change: bool,
    #[arg(long, help = "remove the cloud password")]
    remove: bool,
    #[arg(
        long,
        value_name = "CODE",
        help = "confirm the pending recovery-email code"
    )]
    confirm_email: Option<String>,
    #[arg(long, help = "resend the pending recovery-email code")]
    resend_email: bool,
    #[arg(long, help = "cancel pending recovery-email setup")]
    cancel_email: bool,
    #[arg(
        long,
        help = "show cloud password state (recovery email, pending reset)"
    )]
    status: bool,
    #[arg(long, help = "start the 7-day password reset countdown")]
    reset_start: bool,
    #[arg(
        long,
        help = "cancel a pending 7-day reset (prompts for the current password)"
    )]
    decline_reset: bool,
    #[arg(long, help = "password hint (only with --set or --change)")]
    hint: Option<String>,
    #[arg(long, help = "recovery email (only with --set or --change)")]
    recovery_email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordMode {
    Set,
    Change,
    Remove,
}

impl PasswordMode {
    fn as_str(self) -> &'static str {
        match self {
            PasswordMode::Set => "set",
            PasswordMode::Change => "change",
            PasswordMode::Remove => "remove",
        }
    }
}

#[derive(Debug, Clone)]
enum PasswordAction {
    Mode(PasswordMode),
    ConfirmEmail(String),
    ResendEmail,
    CancelEmail,
    Status,
    ResetStart,
    DeclineReset,
}

impl PasswordAction {
    fn describe(&self) -> String {
        match self {
            PasswordAction::Mode(m) => format!("{} cloud password", m.as_str()),
            PasswordAction::ConfirmEmail(_) => "confirm recovery-email code".to_string(),
            PasswordAction::ResendEmail => "resend recovery-email code".to_string(),
            PasswordAction::CancelEmail => "cancel pending recovery-email setup".to_string(),
            PasswordAction::Status => "show cloud password state".to_string(),
            PasswordAction::ResetStart => "start 7-day password reset countdown".to_string(),
            PasswordAction::DeclineReset => "cancel pending password reset".to_string(),
        }
    }
}

#[derive(Args)]
pub struct LogoutArgs {
    #[arg(long)]
    name: String,
}
#[derive(Args)]
pub struct RemoveArgs {
    #[arg(long)]
    name: String,
}

#[derive(Args)]
pub struct ExportSessionArgs {
    #[arg(long)]
    name: String,
    #[arg(long, help = "destination path; defaults to ./{name}.session")]
    out: Option<String>,
}

#[derive(Args)]
pub struct ImportSessionArgs {
    #[arg(long)]
    file: String,
    #[arg(long = "as", help = "target account name; defaults to the file stem")]
    as_name: Option<String>,
    #[arg(long, help = "overwrite an existing account session")]
    force: bool,
    #[arg(long, help = "source is a Telethon SQLite session to convert")]
    from_telethon: bool,
}

#[derive(Args)]
pub struct TtlArgs {
    #[arg(long, help = "show the current inactive-account self-destruct TTL")]
    get: bool,
    #[arg(long, help = "set the inactive-account self-destruct TTL")]
    set: bool,
    #[arg(
        long,
        value_name = "DAYS",
        help = "days of inactivity before the account self-destructs, 1..=365 (required with --set)"
    )]
    days: Option<i64>,
}

#[derive(Args)]
pub struct DeleteArgs {
    #[arg(long, value_name = "REASON", help = "why the account is being deleted")]
    reason: String,
    #[arg(long, help = "confirm permanent deletion of this Telegram account")]
    yes: bool,
}

#[derive(Args)]
pub struct PhoneArgs {
    #[arg(
        long,
        value_name = "+PHONE",
        help = "send the change-phone code to this new number"
    )]
    change_phone: Option<String>,
    #[arg(
        long,
        help = "allow flash-call verification when sending the change-phone code"
    )]
    allow_flashcall: bool,
    #[arg(
        long,
        value_name = "CODE",
        help = "confirm the pending phone change with this code"
    )]
    confirm_code: Option<String>,
    #[arg(
        long,
        value_name = "HASH",
        help = "phone_code_hash printed by --change-phone"
    )]
    phone_hash: Option<String>,
}
pub async fn run(cmd: AccountCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        AccountCmd::List => list(flags).await,
        AccountCmd::Status => status(flags).await,
        AccountCmd::Add(args) => add(&args, flags).await,
        AccountCmd::Login(args) => login(&args, flags).await,
        AccountCmd::Logout(args) => logout(&args, flags).await,
        AccountCmd::Remove(args) => remove(&args, flags).await,
        AccountCmd::Sessions(args) => sessions(&args, flags).await,
        AccountCmd::Password(args) => password(&args, flags).await,
        AccountCmd::ExportSession(args) => export_session(&args, flags).await,
        AccountCmd::ImportSession(args) => import_session(&args, flags).await,
        AccountCmd::Ttl(args) => ttl(&args, flags).await,
        AccountCmd::Delete(args) => delete(&args, flags).await,
        AccountCmd::Phone(args) => phone(&args, flags).await,
    }
}
async fn list(flags: &GlobalFlags) -> TeleResult<i32> {
    let cfg = config::load_config(flags.config_path.as_deref())?;
    let names = select_sessions(
        &cfg,
        &session::list_session_names(),
        &flags.account,
        &flags.tag,
    )?;
    let mut rows = Vec::new();
    for name in names {
        let tags = cfg
            .accounts
            .get(&name)
            .map(|a| a.tags.join(","))
            .unwrap_or_default();
        rows.push(serde_json::json!({
            "name": name,
            "tags": tags,
            "session": "present",
        }));
    }
    if output::machine_mode(flags.json, flags.jsonl) {
        output::print_json(&list_envelope(&rows, flags)?)?;
        return Ok(crate::error::EXIT_OK);
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r["name"].as_str().unwrap_or_default().to_string(),
                r["tags"].as_str().unwrap_or_default().to_string(),
                r["session"].as_str().unwrap_or_default().to_string(),
            ]
        })
        .collect();
    if table_rows.is_empty() {
        output::print_line("no sessions yet: tele account add + tele account login")?;
    } else {
        output::print_table(&["name", "tags", "session"], &table_rows)?;
    }
    Ok(crate::error::EXIT_OK)
}
async fn status(flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let credentials = creds()?;
    let cfg = config::load_config(flags.config_path.as_deref())?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let credentials = credentials.clone();
        let cfg = cfg.clone();
        Box::pin(async move {
            let guard =
                ClientGuard::connect(&name, credentials.api_id, config_path.as_deref()).await?;
            guard.rate_limiter.acquire().await;
            let authorized = guard.client.is_authorized().await.map_err(|e| {
                if crate::error::invocation_is_unauthorized(&e) {
                    TeleError::Auth("not logged in".to_string())
                } else {
                    TeleError::Invocation(
                        crate::error::invocation_message(&e),
                        crate::error::invocation_wait_seconds(&e),
                    )
                }
            })?;
            let device = status_device_data(&config::account_identity(&cfg, &name));
            Ok(serde_json::json!({ "authorized": authorized, "device": device }))
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        let rows = status_table_rows(&envelope);
        if !rows.is_empty() {
            output::print_table(&["account", "authorized", "device"], &rows)?;
        }
    }
    crate::executor::finish(flags, &envelope)
}

fn status_device_data(identity: &config::DeviceIdentity) -> serde_json::Value {
    let mut device = serde_json::Map::new();
    for (key, value) in [
        ("device_model", &identity.device_model),
        ("system_version", &identity.system_version),
        ("app_version", &identity.app_version),
        ("lang_code", &identity.lang_code),
    ] {
        if let Some(v) = value {
            device.insert(key.to_string(), serde_json::json!(v));
        }
    }
    serde_json::Value::Object(device)
}

fn status_device_summary(data: Option<&serde_json::Value>) -> String {
    let Some(serde_json::Value::Object(map)) = data else {
        return "-".to_string();
    };
    let parts: Vec<String> = ["device_model", "system_version", "app_version", "lang_code"]
        .iter()
        .filter_map(|k| map.get(*k).and_then(|v| v.as_str()).map(String::from))
        .collect();
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("/")
    }
}
async fn add(args: &AddArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    session::validate_name(&args.name).map_err(TeleError::Usage)?;
    let path = flags
        .config_path
        .clone()
        .unwrap_or_else(|| config::app_data_dir().join("config.toml"));
    if flags.dry_run {
        log_line(
            "info",
            &format!(
                "[dry-run] would register account {} in {}",
                args.name,
                path.display()
            ),
        );
        let would = format!(
            "register account {} with tags {:?} in {}",
            args.name,
            args.tags.clone().unwrap_or_default(),
            path.display()
        );
        return crate::executor::finish(
            flags,
            &action_envelope(
                &args.name,
                add_dry_run_data(&args.name, &args.tags, &would),
                true,
                &flags.command,
            ),
        );
    }
    let mut cfg = config::load_config(flags.config_path.as_deref())?;
    let entry = cfg
        .accounts
        .entry(args.name.clone())
        .or_insert_with(config::AccountConfig::default);
    if let Some(tags) = &args.tags {
        entry.tags = tags.clone();
    }
    config::write_config(&path, &cfg)?;
    log_line(
        "info",
        &format!("account {} registered in {}", args.name, path.display()),
    );
    let data = serde_json::json!({
        "registered": true,
        "config": path.display().to_string(),
    });
    crate::executor::finish(
        flags,
        &action_envelope(&args.name, data, flags.dry_run, &flags.command),
    )
}
fn validate_login(method: &str, phone: Option<&str>, qr_timeout_secs: u64) -> TeleResult<()> {
    match method {
        "qr" => {}
        "code" => {
            if phone.is_none() {
                return Err(TeleError::Usage(
                    "--phone required for code login (or set TELE_PHONE)".to_string(),
                ));
            }
        }
        other => {
            return Err(TeleError::Usage(format!(
                "unknown login method {other} (use code or qr)"
            )));
        }
    }
    if qr_timeout_secs == 0 {
        return Err(TeleError::Usage(
            "--qr-timeout-secs must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn resolve_phone(explicit: Option<&str>) -> Option<String> {
    if let Some(phone) = explicit {
        let trimmed = phone.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    std::env::var(TELE_PHONE_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

async fn login(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let phone = resolve_phone(args.phone.as_deref());
    session::validate_name(&args.name).map_err(TeleError::Usage)?;
    let stage = match args.stage.as_deref() {
        Some(raw) => Some(validate_staged(&args.method, raw, phone.as_deref())?),
        None => None,
    };
    if let Some(stage) = stage {
        if let Some(message) =
            argv_phone_warning(args.phone.as_deref(), std::io::stderr().is_terminal())
        {
            log_line("warn", message);
        }
        if flags.dry_run {
            log_line("info", "[dry-run] would run staged login");
            let would = format!(
                "run the {} stage of code login for account {}",
                stage.as_str(),
                args.name
            );
            return crate::executor::finish(
                flags,
                &dry_run_envelope(&args.name, &would, &flags.command),
            );
        }
        return staged_login(args, flags, stage, phone).await;
    }
    validate_login(&args.method, phone.as_deref(), args.qr_timeout_secs)?;
    if let Some(message) =
        argv_phone_warning(args.phone.as_deref(), std::io::stderr().is_terminal())
    {
        log_line("warn", message);
    }
    if flags.dry_run {
        log_line("info", "[dry-run] would log in account");
        let would = format!("log in account {} via {}", args.name, args.method);
        return crate::executor::finish(
            flags,
            &dry_run_envelope(&args.name, &would, &flags.command),
        );
    }
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .unwrap_or(false);
    let mut guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name);
                }
                return Err(e.into());
            }
        };
    let was_authorized = guard
        .client
        .is_authorized()
        .await
        .map_err(tele_invocation)?;
    if was_authorized {
        log_line("info", "account already authorized");
        let data = serde_json::json!({
            "authorized": true,
            "method": args.method.clone(),
        });
        return crate::executor::finish(
            flags,
            &action_envelope(&args.name, data, flags.dry_run, &flags.command),
        );
    }
    let result = login_flow(args, flags, &credentials, &mut guard, phone.as_deref()).await;
    if result.is_err() && !session_existed_before {
        drop(guard);
        cleanup_partial_session(&args.name);
    }
    result
}

fn cleanup_partial_session(name: &str) {
    match session::remove_session(name) {
        Ok(()) => log_line("warn", "login failed; removed partial session files"),
        Err(e) => log_line(
            "warn",
            &format!("login failed; could not remove partial session files: {e:#}"),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginStage {
    Begin,
    Code,
    Status,
    Cancel,
    Resend,
    CancelCode,
}

impl LoginStage {
    fn as_str(self) -> &'static str {
        match self {
            LoginStage::Begin => "begin",
            LoginStage::Code => "code",
            LoginStage::Status => "status",
            LoginStage::Cancel => "cancel",
            LoginStage::Resend => "resend",
            LoginStage::CancelCode => "cancel-code",
        }
    }
}

fn parse_login_stage(raw: &str) -> TeleResult<LoginStage> {
    match raw.trim() {
        "begin" => Ok(LoginStage::Begin),
        "code" => Ok(LoginStage::Code),
        "status" => Ok(LoginStage::Status),
        "cancel" => Ok(LoginStage::Cancel),
        "resend" => Ok(LoginStage::Resend),
        "cancel-code" => Ok(LoginStage::CancelCode),
        other => Err(TeleError::Usage(format!(
            "unknown --stage {other} (use begin, code, status, cancel, resend or cancel-code)"
        ))),
    }
}

fn validate_staged(method: &str, raw_stage: &str, phone: Option<&str>) -> TeleResult<LoginStage> {
    let stage = parse_login_stage(raw_stage)?;
    if method != "code" {
        return Err(TeleError::Usage(
            "--stage supports code login only; drop --method or set it to code".to_string(),
        ));
    }
    if stage == LoginStage::Begin && phone.is_none() {
        return Err(TeleError::Usage(
            "--phone required for --stage begin (or set TELE_PHONE)".to_string(),
        ));
    }
    Ok(stage)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingLogin {
    version: u32,
    account: String,
    phone: String,
    phone_code_hash: String,
    created_at: String,
}

const PENDING_LOGIN_VERSION: u32 = 1;

impl PendingLogin {
    fn new(account: &str, phone: &str, phone_code_hash: String) -> Self {
        Self {
            version: PENDING_LOGIN_VERSION,
            account: account.to_string(),
            phone: phone.to_string(),
            phone_code_hash,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn pending_dir_under(base: &std::path::Path) -> std::path::PathBuf {
    base.join("pending")
}

fn login_pending_file(name: &str) -> String {
    format!("{name}.login.json")
}

fn phone_pending_file(name: &str) -> String {
    format!("{name}.phone.json")
}

fn save_pending_document_under(base: &std::path::Path, file: &str, text: &str) -> TeleResult<()> {
    let dir = pending_dir_under(base);
    crate::fs_util::create_dir_private(&dir)
        .map_err(|e| TeleError::Other(format!("failed to create {}: {e}", dir.display())))?;
    let path = pending_dir_under(base).join(file);
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = std::path::PathBuf::from(tmp_name);
    let result = std::fs::write(&tmp_path, "")
        .and_then(|()| crate::fs_util::restrict_file_private(&tmp_path))
        .and_then(|()| std::fs::write(&tmp_path, text))
        .and_then(|()| {
            #[cfg(windows)]
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            std::fs::rename(&tmp_path, &path)
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result.map_err(|e| TeleError::Other(format!("failed to write {}: {e}", path.display())))
}

fn load_pending_document_under(base: &std::path::Path, file: &str) -> TeleResult<Option<String>> {
    match std::fs::read_to_string(pending_dir_under(base).join(file)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(TeleError::Other(format!(
            "failed to read pending state {}: {e}",
            pending_dir_under(base).join(file).display()
        ))),
    }
}

fn remove_pending_document_under(base: &std::path::Path, file: &str) -> TeleResult<bool> {
    match std::fs::remove_file(pending_dir_under(base).join(file)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(TeleError::Other(format!(
            "failed to remove pending state {file}: {e}"
        ))),
    }
}

fn save_pending(pending: &PendingLogin) -> TeleResult<()> {
    save_pending_under(&config::app_data_dir(), pending)
}

fn save_pending_under(base: &std::path::Path, pending: &PendingLogin) -> TeleResult<()> {
    let text = serde_json::to_string_pretty(pending)?;
    save_pending_document_under(base, &login_pending_file(&pending.account), &text)
}

fn load_pending(name: &str) -> TeleResult<Option<PendingLogin>> {
    load_pending_under(&config::app_data_dir(), name)
}

fn load_pending_under(base: &std::path::Path, name: &str) -> TeleResult<Option<PendingLogin>> {
    match load_pending_document_under(base, &login_pending_file(name))? {
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| TeleError::Other(format!(
                "pending login state for {name} is corrupt ({e}); run tele account login --stage begin again"
            ))),
        None => Ok(None),
    }
}

fn require_pending(name: &str) -> TeleResult<PendingLogin> {
    require_pending_under(&config::app_data_dir(), name)
}

fn require_pending_under(base: &std::path::Path, name: &str) -> TeleResult<PendingLogin> {
    load_pending_under(base, name)?.ok_or_else(|| {
        TeleError::Usage(format!(
            "no pending login for account {name}; run tele account login --name {name} --stage begin first"
        ))
    })
}

fn remove_pending(name: &str) -> TeleResult<bool> {
    remove_pending_under(&config::app_data_dir(), name)
}

fn remove_pending_under(base: &std::path::Path, name: &str) -> TeleResult<bool> {
    remove_pending_document_under(base, &login_pending_file(name))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingPhone {
    version: u32,
    account: String,
    phone: String,
    phone_code_hash: String,
    created_at: String,
}

const PENDING_PHONE_VERSION: u32 = 1;

impl PendingPhone {
    fn new(account: &str, phone: &str, phone_code_hash: String) -> Self {
        Self {
            version: PENDING_PHONE_VERSION,
            account: account.to_string(),
            phone: phone.to_string(),
            phone_code_hash,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn save_pending_phone(pending: &PendingPhone) -> TeleResult<()> {
    save_pending_phone_under(&config::app_data_dir(), pending)
}

fn save_pending_phone_under(base: &std::path::Path, pending: &PendingPhone) -> TeleResult<()> {
    let text = serde_json::to_string_pretty(pending)?;
    save_pending_document_under(base, &phone_pending_file(&pending.account), &text)
}

fn load_pending_phone_under(
    base: &std::path::Path,
    name: &str,
) -> TeleResult<Option<PendingPhone>> {
    match load_pending_document_under(base, &phone_pending_file(name))? {
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| TeleError::Other(format!(
                "pending phone-change state for {name} is corrupt ({e}); run tele account phone --change-phone again"
            ))),
        None => Ok(None),
    }
}

fn require_pending_phone(name: &str) -> TeleResult<PendingPhone> {
    require_pending_phone_under(&config::app_data_dir(), name)
}

fn require_pending_phone_under(base: &std::path::Path, name: &str) -> TeleResult<PendingPhone> {
    load_pending_phone_under(base, name)?.ok_or_else(|| {
        TeleError::Usage(format!(
            "no pending phone change for account {name}; run tele account phone --change-phone first"
        ))
    })
}

fn remove_pending_phone(name: &str) -> TeleResult<bool> {
    remove_pending_phone_under(&config::app_data_dir(), name)
}

fn remove_pending_phone_under(base: &std::path::Path, name: &str) -> TeleResult<bool> {
    remove_pending_document_under(base, &phone_pending_file(name))
}

fn phone_hash_matches(pending: &PendingPhone, hash: &str) -> bool {
    pending.phone_code_hash == hash.trim()
}

fn stage_status_data(pending: Option<&PendingLogin>) -> serde_json::Value {
    match pending {
        Some(p) => serde_json::json!({
            "stage": "status",
            "pending": true,
            "account": p.account,
            "phone": redact_phone(&p.phone),
            "created_at": p.created_at,
        }),
        None => serde_json::json!({"stage": "status", "pending": false}),
    }
}

fn stage_cancel_data(cancelled: bool) -> serde_json::Value {
    serde_json::json!({"stage": "cancel", "cancelled": cancelled})
}

fn stage_status_line(pending: Option<&PendingLogin>, name: &str) -> String {
    match pending {
        Some(p) => format!(
            "pending login for account {name}: run tele account login --name {name} --stage code (requested {})",
            p.created_at
        ),
        None => format!("no pending login for account {name}"),
    }
}

async fn staged_login(
    args: &LoginArgs,
    flags: &GlobalFlags,
    stage: LoginStage,
    phone: Option<String>,
) -> TeleResult<i32> {
    match stage {
        LoginStage::Status => {
            let pending = load_pending(&args.name)?;
            if !output::machine_mode(flags.json, flags.jsonl) {
                output::print_line(&stage_status_line(pending.as_ref(), &args.name))?;
            }
            crate::executor::finish(
                flags,
                &action_envelope(
                    &args.name,
                    stage_status_data(pending.as_ref()),
                    flags.dry_run,
                    &flags.command,
                ),
            )
        }
        LoginStage::Cancel => {
            let cancelled = remove_pending(&args.name)?;
            if !output::machine_mode(flags.json, flags.jsonl) {
                if cancelled {
                    output::print_line(&format!(
                        "discarded pending login for account {}",
                        args.name
                    ))?;
                } else {
                    output::print_line(&format!("no pending login for account {}", args.name))?;
                }
            }
            crate::executor::finish(
                flags,
                &action_envelope(
                    &args.name,
                    stage_cancel_data(cancelled),
                    flags.dry_run,
                    &flags.command,
                ),
            )
        }
        LoginStage::Begin => staged_begin(args, flags, phone.as_deref()).await,
        LoginStage::Code => staged_code(args, flags).await,
        LoginStage::Resend => staged_resend(args, flags).await,
        LoginStage::CancelCode => staged_cancel_code(args, flags).await,
    }
}

async fn staged_begin(
    args: &LoginArgs,
    flags: &GlobalFlags,
    phone: Option<&str>,
) -> TeleResult<i32> {
    let phone = phone.ok_or_else(|| {
        TeleError::Usage("--phone required for --stage begin (or set TELE_PHONE)".to_string())
    })?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .unwrap_or(false);
    let guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name);
                }
                return Err(e.into());
            }
        };
    let result = staged_begin_flow(&guard, &credentials, &args.name, phone, flags).await;
    drop(guard);
    if result.is_err() && !session_existed_before {
        cleanup_partial_session(&args.name);
    }
    result
}

async fn staged_begin_flow(
    guard: &ClientGuard,
    credentials: &crate::config::Credentials,
    name: &str,
    phone: &str,
    flags: &GlobalFlags,
) -> TeleResult<i32> {
    guard.rate_limiter.acquire().await;
    let authorized = guard
        .client
        .is_authorized()
        .await
        .map_err(tele_invocation)?;
    if authorized {
        let _ = remove_pending(name);
        log_line("info", "account already authorized");
        let data = serde_json::json!({"authorized": true, "method": "code"});
        return crate::executor::finish(flags, &action_envelope(name, data, false, &flags.command));
    }
    let sent = send_login_code(
        &guard.client,
        &guard.session,
        phone,
        credentials.api_id,
        &credentials.api_hash,
    )
    .await?;
    save_pending(&PendingLogin::new(name, phone, sent.phone_code_hash))?;
    log_line(
        "info",
        &format!(
            "login code sent to {}; finish with tele account login --name {name} --stage code",
            redact_phone(phone)
        ),
    );
    crate::executor::finish(
        flags,
        &action_envelope(
            name,
            serde_json::json!({"stage": "begin", "pending": true}),
            false,
            &flags.command,
        ),
    )
}

async fn send_login_code(
    client: &grammers_client::Client,
    storage: &Arc<grammers_client::session::storages::SqliteSession>,
    phone: &str,
    api_id: i32,
    api_hash: &str,
) -> TeleResult<grammers_client::tl::types::auth::SentCode> {
    use grammers_client::{session::Session as _, tl};
    let request = tl::functions::auth::SendCode {
        phone_number: phone.to_string(),
        api_id,
        api_hash: api_hash.to_string(),
        settings: tl::types::CodeSettings {
            allow_flashcall: false,
            current_number: false,
            allow_app_hash: false,
            allow_missed_call: false,
            allow_firebase: false,
            logout_tokens: None,
            token: None,
            app_sandbox: None,
            unknown_number: false,
        }
        .into(),
    };
    match client.invoke(&request).await {
        Ok(tl::enums::auth::SentCode::Code(code)) => Ok(code),
        Ok(tl::enums::auth::SentCode::Success(_)) => Err(TeleError::Auth(
            "server reports the account is already signed in".to_string(),
        )),
        Ok(tl::enums::auth::SentCode::PaymentRequired(x)) => Err(TeleError::Other(format!(
            "login requires paid verification (product {})",
            x.store_product
        ))),
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.code == 303 => {
            let dc_id = rpc
                .value
                .map(|v| i32::try_from(v).unwrap_or_default())
                .ok_or_else(|| {
                    TeleError::Other("DC migration hint arrived without a target DC".to_string())
                })?;
            storage.set_home_dc_id(dc_id).await.map_err(|e| {
                TeleError::Other(format!("failed to switch home DC to {dc_id}: {e}"))
            })?;
            match client.invoke(&request).await {
                Ok(tl::enums::auth::SentCode::Code(code)) => Ok(code),
                Ok(_) => Err(TeleError::Other(
                    "unexpected response after DC migration".to_string(),
                )),
                Err(e) => Err(tele_invocation(e)),
            }
        }
        Err(e) => Err(tele_invocation(e)),
    }
}

async fn staged_code(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .unwrap_or(false);
    let guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name);
                }
                return Err(e.into());
            }
        };
    let result = staged_code_flow(&guard, &pending, flags).await;
    drop(guard);
    if result.is_err() && !session_existed_before {
        cleanup_partial_session(&args.name);
    }
    result
}

async fn staged_code_flow(
    guard: &ClientGuard,
    pending: &PendingLogin,
    flags: &GlobalFlags,
) -> TeleResult<i32> {
    guard.rate_limiter.acquire().await;
    let already = guard
        .client
        .is_authorized()
        .await
        .map_err(tele_invocation)?;
    if already {
        let _ = remove_pending(&pending.account);
        log_line("info", "account already authorized");
        return code_envelope(flags, pending, true);
    }
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let prompt = code_prompt(Some(&pending.phone), stderr.is_terminal());
    for attempt in 1..=MAX_CODE_ATTEMPTS {
        let Some(code_line) = prompt_line(&prompt, &mut stdin, &mut stderr)? else {
            return Err(TeleError::Usage(
                "no code entered (stdin closed)".to_string(),
            ));
        };
        let code = code_line.trim().to_string();
        match raw_sign_in(&guard.client, pending, &code).await {
            StagedSignIn::SignedIn(auth) => {
                complete_staged_login(guard, *auth).await?;
                let _ = remove_pending(&pending.account);
                log_line(
                    "info",
                    &format!(
                        "account {} logged in ({})",
                        pending.account,
                        redact_phone(&pending.phone)
                    ),
                );
                return code_envelope(flags, pending, true);
            }
            StagedSignIn::PasswordNeeded => {
                if !std::io::stdin().is_terminal() {
                    return Err(TeleError::Auth(
                        "2FA password required; re-run this command in an interactive terminal"
                            .to_string(),
                    ));
                }
                let pw_token = refresh_password_token(&guard.client).await?;
                password_flow(&guard.client, pw_token, &mut stdin, &mut stderr).await?;
                let _ = remove_pending(&pending.account);
                log_line(
                    "info",
                    &format!(
                        "account {} logged in ({})",
                        pending.account,
                        redact_phone(&pending.phone)
                    ),
                );
                return code_envelope(flags, pending, true);
            }
            StagedSignIn::InvalidCode => {
                if attempt >= MAX_CODE_ATTEMPTS {
                    return Err(TeleError::Usage(
                        "invalid code: attempts exhausted; re-run tele account login --stage code (or --stage begin if the code expired)"
                            .to_string(),
                    ));
                }
                log_line("warn", "invalid code; try again");
            }
            StagedSignIn::CodeExpired => {
                let _ = remove_pending(&pending.account);
                return Err(TeleError::Auth(
                    "login code expired; discarded pending state; re-run tele account login --stage begin"
                        .to_string(),
                ));
            }
            StagedSignIn::SignUpRequired => {
                return Err(TeleError::Usage(
                    "sign up with an official client first".to_string(),
                ));
            }
            StagedSignIn::Failed(e) => return Err(e),
        }
    }
    Err(TeleError::Usage(
        "invalid code: attempts exhausted".to_string(),
    ))
}

fn code_envelope(flags: &GlobalFlags, pending: &PendingLogin, authorized: bool) -> TeleResult<i32> {
    let data = serde_json::json!({
        "stage": "code",
        "authorized": authorized,
        "method": "code",
    });
    crate::executor::finish(
        flags,
        &action_envelope(&pending.account, data, false, &flags.command),
    )
}

async fn staged_resend(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .unwrap_or(false);
    let guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name);
                }
                return Err(e.into());
            }
        };
    let result = staged_resend_flow(&guard, &pending, flags).await;
    drop(guard);
    if result.is_err() && !session_existed_before {
        cleanup_partial_session(&args.name);
    }
    result
}

async fn staged_resend_flow(
    guard: &ClientGuard,
    pending: &PendingLogin,
    flags: &GlobalFlags,
) -> TeleResult<i32> {
    guard.rate_limiter.acquire().await;
    let request = grammers_client::tl::functions::auth::ResendCode {
        phone_number: pending.phone.clone(),
        phone_code_hash: pending.phone_code_hash.clone(),
        reason: None,
    };
    match guard.client.invoke(&request).await {
        Ok(grammers_client::tl::enums::auth::SentCode::Code(code)) => {
            let updated = PendingLogin {
                phone_code_hash: code.phone_code_hash,
                ..pending.clone()
            };
            save_pending(&updated)?;
            log_line(
                "info",
                &format!(
                    "login code resent to {}; finish with tele account login --name {} --stage code",
                    redact_phone(&pending.phone),
                    pending.account
                ),
            );
            let data = serde_json::json!({"stage": "resend", "resent": true});
            crate::executor::finish(
                flags,
                &action_envelope(&pending.account, data, flags.dry_run, &flags.command),
            )
        }
        Ok(grammers_client::tl::enums::auth::SentCode::Success(_)) => {
            let _ = remove_pending(&pending.account);
            log_line("info", "server reports the account is already signed in");
            let data = serde_json::json!({"stage": "resend", "authorized": true});
            crate::executor::finish(
                flags,
                &action_envelope(&pending.account, data, flags.dry_run, &flags.command),
            )
        }
        Ok(grammers_client::tl::enums::auth::SentCode::PaymentRequired(x)) => {
            Err(TeleError::Other(format!(
                "login requires paid verification (product {})",
                x.store_product
            )))
        }
        Err(e) => Err(tele_invocation(e)),
    }
}

async fn staged_cancel_code(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .unwrap_or(false);
    let guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name);
                }
                return Err(e.into());
            }
        };
    let result = staged_cancel_code_flow(&guard, &pending).await;
    drop(guard);
    if result.is_err() && !session_existed_before {
        cleanup_partial_session(&args.name);
    }
    match result {
        Ok(data) => crate::executor::finish(
            flags,
            &action_envelope(&args.name, data, flags.dry_run, &flags.command),
        ),
        Err(e) => Err(e),
    }
}

async fn staged_cancel_code_flow(
    guard: &ClientGuard,
    pending: &PendingLogin,
) -> TeleResult<serde_json::Value> {
    guard.rate_limiter.acquire().await;
    let request = grammers_client::tl::functions::auth::CancelCode {
        phone_number: pending.phone.clone(),
        phone_code_hash: pending.phone_code_hash.clone(),
    };
    match guard.client.invoke(&request).await {
        Ok(true) => {
            remove_pending(&pending.account)?;
            Ok(serde_json::json!({
                "stage": "cancel-code",
                "cancelled": true,
                "server_notified": true,
            }))
        }
        Ok(false) => Err(TeleError::Other(
            "server refused to cancel the sent login code; local pending state kept".to_string(),
        )),
        Err(e) => Err(tele_invocation(e)),
    }
}

async fn login_flow(
    args: &LoginArgs,
    flags: &GlobalFlags,
    credentials: &crate::config::Credentials,
    guard: &mut ClientGuard,
    phone: Option<&str>,
) -> TeleResult<i32> {
    match args.method.as_str() {
        "qr" => {
            let show_token = args.show_token;
            client::qr_login(
                &guard.client,
                &mut guard.updates,
                credentials,
                args.qr_timeout_secs,
                |uri| {
                    render_qr(uri, show_token);
                },
            )
            .await?;
        }
        "code" => {
            let phone = phone
                .ok_or_else(|| TeleError::Usage("--phone required for code login".to_string()))?;
            let token = guard
                .client
                .request_login_code(phone, &credentials.api_hash)
                .await
                .map_err(tele_invocation)?;
            let mut stdin = std::io::stdin().lock();
            let mut stderr = std::io::stderr();
            let prompt = code_prompt(Some(phone), stderr.is_terminal());
            sign_in_with_retries(
                &guard.client,
                &token,
                &prompt,
                &mut stdin,
                &mut stderr,
                &args.name,
                phone,
            )
            .await?;
        }
        other => {
            return Err(TeleError::Usage(format!(
                "unknown login method {other} (use code or qr)"
            )));
        }
    }
    let data = serde_json::json!({
        "authorized": true,
        "method": args.method.clone(),
    });
    crate::executor::finish(
        flags,
        &action_envelope(&args.name, data, flags.dry_run, &flags.command),
    )
}

async fn sign_in_with_retries(
    client: &grammers_client::Client,
    token: &grammers_client::client::LoginToken,
    prompt: &str,
    stdin: &mut impl std::io::BufRead,
    stderr: &mut impl std::io::Write,
    account_name: &str,
    phone: &str,
) -> TeleResult<()> {
    for attempt in 1..=MAX_CODE_ATTEMPTS {
        let Some(code_line) = prompt_line(prompt, stdin, stderr)? else {
            return Err(TeleError::Usage(
                "no code entered (stdin closed)".to_string(),
            ));
        };
        let code = code_line.trim().to_string();
        match client.sign_in(token, &code).await {
            Ok(_user) => {
                log_line(
                    "info",
                    &format!(
                        "account {} logged in ({})",
                        account_name,
                        redact_phone(phone)
                    ),
                );
                return Ok(());
            }
            Err(grammers_client::SignInError::PasswordRequired(pw_token)) => {
                return password_flow(client, pw_token, stdin, stderr).await;
            }
            Err(grammers_client::SignInError::InvalidCode) => {
                if attempt >= MAX_CODE_ATTEMPTS {
                    return Err(TeleError::Usage(
                        "invalid code: attempts exhausted; run tele account login again to request a new code"
                            .to_string(),
                    ));
                }
                log_line("warn", "invalid code; try again");
            }
            Err(grammers_client::SignInError::InvalidPassword(_)) => {
                return Err(TeleError::Usage("invalid code".to_string()));
            }
            Err(grammers_client::SignInError::SignUpRequired) => {
                return Err(TeleError::Usage(
                    "sign up with an official client first".to_string(),
                ));
            }
            Err(grammers_client::SignInError::Other(e)) => return Err(tele_invocation(e)),
        }
    }
    Ok(())
}

async fn password_flow(
    client: &grammers_client::Client,
    pw_token: grammers_client::client::PasswordToken,
    stdin: &mut impl std::io::BufRead,
    stderr: &mut impl std::io::Write,
) -> TeleResult<()> {
    let mut pw_token = Some(pw_token);
    for attempt in 1..=MAX_PASSWORD_ATTEMPTS {
        let Some(token) = pw_token.take() else {
            return Err(TeleError::Auth(
                "2FA password token unavailable".to_string(),
            ));
        };
        let echo_disabled = disable_stdin_echo();
        if !echo_disabled && attempt == 1 {
            log_line(
                "warn",
                "secure password input unavailable; input will be echoed to the terminal",
            );
        }
        let read = prompt_line("Enter the 2FA password: ", stdin, stderr);
        restore_stdin_echo(echo_disabled);
        let Some(password_line) = read? else {
            return Err(TeleError::Auth(
                "2FA password required; stdin closed".to_string(),
            ));
        };
        let password = strip_line_ending(&password_line).to_string();
        match client.check_password(token, &password).await {
            Ok(_) => {
                log_line("info", "2FA passed");
                return Ok(());
            }
            Err(grammers_client::SignInError::InvalidPassword(_)) => {
                if attempt >= MAX_PASSWORD_ATTEMPTS {
                    return Err(TeleError::Auth(
                        "invalid 2FA password: attempts exhausted".to_string(),
                    ));
                }
                log_line("warn", "invalid 2FA password; try again");
                pw_token = Some(refresh_password_token(client).await?);
            }
            Err(grammers_client::SignInError::Other(e)) => return Err(tele_invocation(e)),
            Err(_) => return Err(TeleError::Auth("2FA check failed".to_string())),
        }
    }
    Ok(())
}

async fn refresh_password_token(
    client: &grammers_client::Client,
) -> TeleResult<grammers_client::client::PasswordToken> {
    use grammers_client::tl::{self, enums};
    let response = client
        .invoke(&tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    let enums::account::Password::Password(password) = response;
    Ok(grammers_client::client::PasswordToken::new(password))
}

enum StagedSignIn {
    SignedIn(Box<grammers_client::tl::types::auth::Authorization>),
    PasswordNeeded,
    InvalidCode,
    CodeExpired,
    SignUpRequired,
    Failed(TeleError),
}

async fn raw_sign_in(
    client: &grammers_client::Client,
    pending: &PendingLogin,
    code: &str,
) -> StagedSignIn {
    use grammers_client::tl;
    let request = tl::functions::auth::SignIn {
        phone_number: pending.phone.clone(),
        phone_code_hash: pending.phone_code_hash.clone(),
        phone_code: Some(code.to_string()),
        email_verification: None,
    };
    match client.invoke(&request).await {
        Ok(tl::enums::auth::Authorization::Authorization(x)) => StagedSignIn::SignedIn(Box::new(x)),
        Ok(tl::enums::auth::Authorization::SignUpRequired(_)) => StagedSignIn::SignUpRequired,
        Err(grammers_client::InvocationError::Rpc(rpc))
            if rpc.name == "SESSION_PASSWORD_NEEDED" =>
        {
            StagedSignIn::PasswordNeeded
        }
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.name == "PHONE_CODE_EXPIRED" => {
            StagedSignIn::CodeExpired
        }
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.name.starts_with("PHONE_CODE_") => {
            StagedSignIn::InvalidCode
        }
        Err(e) => StagedSignIn::Failed(tele_invocation(e)),
    }
}

async fn complete_staged_login(
    guard: &ClientGuard,
    auth: grammers_client::tl::types::auth::Authorization,
) -> TeleResult<()> {
    use grammers_client::{
        session::{
            types::{PeerAuth, PeerInfo, UpdateState, UpdatesState},
            Session as _,
        },
        tl,
    };
    let user = match auth.user {
        tl::enums::User::User(user) => user,
        tl::enums::User::Empty(_) => {
            return Err(TeleError::Other(
                "server returned an empty user after sign in".to_string(),
            ));
        }
    };
    guard
        .session
        .cache_peer(&PeerInfo::User {
            id: user.id,
            auth: user
                .access_hash
                .filter(|_| !user.min)
                .map(PeerAuth::from_hash),
            bot: Some(user.bot),
            is_self: Some(true),
        })
        .await
        .map_err(|e| TeleError::Other(format!("failed to cache signed-in user: {e}")))?;
    if let Ok(tl::enums::updates::State::State(state)) = guard
        .client
        .invoke(&tl::functions::updates::GetState {})
        .await
    {
        guard
            .session
            .set_update_state(UpdateState::All(UpdatesState {
                pts: state.pts,
                qts: state.qts,
                date: state.date,
                seq: state.seq,
                channels: Vec::new(),
            }))
            .await
            .map_err(|e| TeleError::Other(format!("failed to store update state: {e}")))?;
    }
    Ok(())
}
async fn logout(args: &LogoutArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if flags.dry_run {
        log_line("info", "[dry-run] would log out account");
        let would = format!("log out account {}", args.name);
        return crate::executor::finish(
            flags,
            &dry_run_envelope(&args.name, &would, &flags.command),
        );
    }
    let credentials = creds()?;
    let guard =
        ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref()).await?;
    if let Err(e) = guard.client.sign_out().await {
        if crate::error::invocation_is_unauthorized(&e) {
            log_line("info", "account was not authorized; removing session");
        } else {
            return Err(tele_invocation(e));
        }
    }
    guard.close().await;
    remove_session_file_retry(&session::session_path(&args.name)).await?;
    remove_session_file_retry(&session::lock_path(&args.name)).await?;
    log_line("info", &format!("account {} logged out", args.name));
    let data = serde_json::json!({"signed_out": true});
    crate::executor::finish(
        flags,
        &action_envelope(&args.name, data, flags.dry_run, &flags.command),
    )
}
async fn remove(args: &RemoveArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    session::validate_name(&args.name).map_err(TeleError::Usage)?;
    if flags.dry_run {
        log_line("info", "[dry-run] would remove account");
        let would = format!("remove account {} and its session", args.name);
        return crate::executor::finish(
            flags,
            &dry_run_envelope(&args.name, &would, &flags.command),
        );
    }
    session::remove_session(&args.name)?;
    let mut cfg = config::load_config(flags.config_path.as_deref())?;
    cfg.accounts.remove(&args.name);
    let path = flags
        .config_path
        .clone()
        .unwrap_or_else(|| config::app_data_dir().join("config.toml"));
    config::write_config(&path, &cfg)?;
    log_line("info", &format!("account {} removed", args.name));
    let data = serde_json::json!({
        "removed": true,
        "config": path.display().to_string(),
    });
    crate::executor::finish(
        flags,
        &action_envelope(&args.name, data, flags.dry_run, &flags.command),
    )
}

fn export_row(exported: &session::ExportedSession) -> Vec<String> {
    vec![
        exported.account.clone(),
        exported.path.display().to_string(),
        exported.bytes.to_string(),
        exported.sha256.clone(),
    ]
}

fn export_data(exported: &session::ExportedSession) -> serde_json::Value {
    serde_json::json!({
        "account": exported.account,
        "path": exported.path.display().to_string(),
        "bytes": exported.bytes,
        "sha256": exported.sha256,
    })
}

fn import_data(imported: &session::ImportedSession) -> serde_json::Value {
    serde_json::json!({
        "imported": true,
        "account": imported.account,
        "path": imported.path.display().to_string(),
        "bytes": imported.bytes,
    })
}

async fn export_session(args: &ExportSessionArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    session::validate_name(&args.name).map_err(TeleError::Usage)?;
    if flags.dry_run {
        let dest = args.out.as_deref().unwrap_or("./<name>.session");
        let would = format!("export session {dest} from account {}", args.name);
        return crate::executor::finish(
            flags,
            &dry_run_envelope(&args.name, &would, &flags.command),
        );
    }
    let exported =
        session::export_session(&args.name, args.out.as_deref().map(std::path::Path::new))?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        log_line("warn", session::SESSION_FILE_WARNING);
    }
    let data = export_data(&exported);
    if !output::machine_mode(flags.json, flags.jsonl) {
        output::print_table(
            &["account", "path", "bytes", "sha256"],
            &[export_row(&exported)],
        )?;
    }
    crate::executor::finish(
        flags,
        &action_envelope(&args.name, data, false, &flags.command),
    )
}

fn import_row(imported: &session::ImportedSession) -> Vec<String> {
    vec![
        imported.account.clone(),
        imported.path.display().to_string(),
        imported.bytes.to_string(),
    ]
}

async fn import_session(args: &ImportSessionArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if let Some(name) = args.as_name.as_deref() {
        session::validate_name(name).map_err(TeleError::Usage)?;
    }
    let file = std::path::PathBuf::from(args.file.trim());
    if args.from_telethon {
        if flags.dry_run {
            let name = session::resolve_import_name(args.as_name.as_deref(), &file)
                .map_err(|e| TeleError::Usage(format!("{e:#}")))?;
            let would = format!(
                "convert Telethon session {} as account {name}",
                file.display()
            );
            return crate::executor::finish(
                flags,
                &dry_run_envelope(&name, &would, &flags.command),
            );
        }
        let imported =
            session::convert_telethon_session(&file, args.as_name.as_deref(), args.force).await?;
        return finish_import(flags, &imported).await;
    }
    if flags.dry_run {
        let name = session::resolve_import_name(args.as_name.as_deref(), &file)
            .map_err(|e| TeleError::Usage(format!("{e:#}")))?;
        let would = format!("import session {} as account {name}", file.display());
        return crate::executor::finish(flags, &dry_run_envelope(&name, &would, &flags.command));
    }
    let imported = session::import_session(&file, args.as_name.as_deref(), args.force).await?;
    finish_import(flags, &imported).await
}

async fn finish_import(
    flags: &GlobalFlags,
    imported: &session::ImportedSession,
) -> TeleResult<i32> {
    if !output::machine_mode(flags.json, flags.jsonl) {
        log_line("warn", session::SESSION_FILE_WARNING);
    }
    let data = import_data(imported);
    if !output::machine_mode(flags.json, flags.jsonl) {
        output::print_table(&["account", "path", "bytes"], &[import_row(imported)])?;
    }
    crate::executor::finish(
        flags,
        &action_envelope(&imported.account, data, false, &flags.command),
    )
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TtlAction {
    Get,
    Set(i32),
}

fn validate_ttl(get: bool, set: bool, days: Option<i64>) -> TeleResult<TtlAction> {
    match (get, set) {
        (false, false) => Err(TeleError::Usage("choose one of --get or --set".to_string())),
        (true, true) => Err(TeleError::Usage(
            "--get and --set are mutually exclusive".to_string(),
        )),
        (true, false) => {
            if days.is_some() {
                return Err(TeleError::Usage("--days only applies to --set".to_string()));
            }
            Ok(TtlAction::Get)
        }
        (false, true) => {
            let raw =
                days.ok_or_else(|| TeleError::Usage("--days required with --set".to_string()))?;
            let parsed = i32::try_from(raw)
                .map_err(|_| TeleError::Usage("--days must be between 1 and 365".to_string()))?;
            if !(1..=365).contains(&parsed) {
                return Err(TeleError::Usage(format!(
                    "--days must be between 1 and 365, got {parsed}"
                )));
            }
            Ok(TtlAction::Set(parsed))
        }
    }
}

fn ttl_data(days: i32) -> serde_json::Value {
    serde_json::json!({"ttl_days": days})
}

fn ttl_set_data(days: i32) -> serde_json::Value {
    serde_json::json!({"updated": true, "ttl_days": days})
}

fn ttl_dry_run_data(name: &str, action: &TtlAction) -> serde_json::Value {
    let would = match action {
        TtlAction::Get => format!("show the inactive-account TTL for {name}"),
        TtlAction::Set(days) => format!("set the inactive-account TTL for {name} to {days} days"),
    };
    serde_json::json!({"dry_run": true, "would": would})
}

async fn execute_ttl_action(
    guard: &ClientGuard,
    action: TtlAction,
) -> TeleResult<serde_json::Value> {
    use grammers_client::tl::{self, enums};
    guard.rate_limiter.acquire().await;
    match action {
        TtlAction::Get => {
            let response = guard
                .client
                .invoke(&tl::functions::account::GetAccountTtl {})
                .await
                .map_err(tele_invocation)?;
            let enums::AccountDaysTtl::Ttl(ttl) = response;
            Ok(ttl_data(ttl.days))
        }
        TtlAction::Set(days) => {
            let request = tl::functions::account::SetAccountTtl {
                ttl: enums::AccountDaysTtl::Ttl(tl::types::AccountDaysTtl { days }),
            };
            let result = guard.client.invoke(&request).await;
            match result {
                Ok(true) => Ok(ttl_set_data(days)),
                Ok(false) => Err(TeleError::Other(
                    "server refused to update the account TTL".to_string(),
                )),
                Err(e) => Err(tele_invocation(e)),
            }
        }
    }
}

async fn ttl(args: &TtlArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let action = validate_ttl(args.get, args.set, args.days)?;
    require_explicit_selection("account ttl", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(ttl_dry_run_data(&name, &action));
            }
            let credentials = creds()?;
            let guard =
                ClientGuard::connect(&name, credentials.api_id, config_path.as_deref()).await?;
            execute_ttl_action(&guard, action).await
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        for outcome in envelope.accounts.iter().filter(|o| o.ok) {
            match action {
                TtlAction::Get => {
                    let days = outcome
                        .data
                        .as_ref()
                        .and_then(|d| d["ttl_days"].as_i64())
                        .unwrap_or_default();
                    output::print_line(&format!(
                        "account {}: session TTL {} days",
                        outcome.account, days
                    ))?;
                }
                TtlAction::Set(days) => {
                    output::print_line(&format!(
                        "account {}: session TTL set to {} days",
                        outcome.account, days
                    ))?;
                }
            }
        }
    }
    crate::executor::finish(flags, &envelope)
}

fn validate_delete_reason(reason: &str) -> TeleResult<&str> {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return Err(TeleError::Usage("--reason must not be empty".to_string()));
    }
    Ok(trimmed)
}

fn delete_would(target: &str, reason: &str) -> String {
    format!(
        "permanently delete account {target}: profile, chats and messages will be destroyed (reason: {reason})"
    )
}

fn delete_dry_run_data(name: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "action": "delete account",
        "reason": reason,
        "would": delete_would(name, reason),
    })
}

async fn delete_proof(
    guard: &ClientGuard,
) -> TeleResult<grammers_client::tl::enums::InputCheckPasswordSrp> {
    let password = fetch_password(guard).await?;
    if !password.has_password {
        return Ok(grammers_client::tl::enums::InputCheckPasswordSrp::InputCheckPasswordEmpty);
    }
    let params = extract_srp_params(password.current_algo.as_ref())?;
    let srp_b = password
        .srp_b
        .clone()
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    let srp_id = password
        .srp_id
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    prompt_current_password_proof(
        &params,
        srp_id,
        &srp_b,
        &password.secure_random,
        "Enter the current cloud password to delete the account: ",
    )
}

async fn delete(args: &DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let reason = validate_delete_reason(&args.reason)?;
    require_explicit_selection("account delete", flags)?;
    if !args.yes {
        let targets = crate::executor::select_accounts(flags)?;
        let list = targets.join(", ");
        let would = delete_would(&list, reason);
        log_line("info", &format!("[no-op] would {would}"));
        return Err(TeleError::Usage(format!("--yes required: would {would}")));
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let reason = reason.to_string();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let reason = reason.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(delete_dry_run_data(&name, &reason));
            }
            let credentials = creds()?;
            let guard =
                ClientGuard::connect(&name, credentials.api_id, config_path.as_deref()).await?;
            let proof = delete_proof(&guard).await?;
            guard.rate_limiter.acquire().await;
            let request = grammers_client::tl::functions::account::DeleteAccount {
                reason: reason.clone(),
                password: Some(proof),
            };
            let result = guard.client.invoke(&request).await;
            drop(guard);
            match result {
                Ok(true) => Ok(serde_json::json!({"deleted": true})),
                Ok(false) => Err(TeleError::Other(
                    "server refused to delete the account".to_string(),
                )),
                Err(e) => Err(map_update_password_error(e)),
            }
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        for outcome in envelope.accounts.iter().filter(|o| o.ok) {
            output::log_line("info", &format!("account {} deleted", outcome.account));
        }
    }
    crate::executor::finish(flags, &envelope)
}

#[derive(Debug, Clone)]
enum PhoneAction {
    Send { phone: String, flashcall: bool },
    Confirm { code: String, hash: String },
}

impl PhoneAction {
    fn describe(&self, name: &str) -> String {
        match self {
            PhoneAction::Send { phone, .. } => format!(
                "send the change-phone code for account {} to {}",
                name,
                redact_phone(phone)
            ),
            PhoneAction::Confirm { code, hash } => format!(
                "confirm the phone change for account {name} with code {code} using phone_code_hash {hash}"
            ),
        }
    }
}

fn validate_phone_modes(args: &PhoneArgs) -> TeleResult<PhoneAction> {
    let primaries =
        usize::from(args.change_phone.is_some()) + usize::from(args.confirm_code.is_some());
    if primaries == 0 {
        return Err(TeleError::Usage(
            "choose one of --change-phone or --confirm-code".to_string(),
        ));
    }
    if primaries > 1 {
        return Err(TeleError::Usage(
            "--change-phone and --confirm-code are mutually exclusive".to_string(),
        ));
    }
    if let Some(phone) = args.change_phone.as_deref() {
        let phone = phone.trim();
        if phone.is_empty() {
            return Err(TeleError::Usage(
                "--change-phone must not be empty".to_string(),
            ));
        }
        if args.phone_hash.is_some() {
            return Err(TeleError::Usage(
                "--phone-hash only applies to --confirm-code".to_string(),
            ));
        }
        return Ok(PhoneAction::Send {
            phone: phone.to_string(),
            flashcall: args.allow_flashcall,
        });
    }
    if args.allow_flashcall {
        return Err(TeleError::Usage(
            "--allow-flashcall only applies to --change-phone".to_string(),
        ));
    }
    let code = args
        .confirm_code
        .as_deref()
        .ok_or_else(|| TeleError::Usage("--confirm-code required".to_string()))?
        .trim();
    if code.is_empty() {
        return Err(TeleError::Usage(
            "--confirm-code must not be empty".to_string(),
        ));
    }
    let hash = args
        .phone_hash
        .as_deref()
        .ok_or_else(|| TeleError::Usage("--phone-hash required with --confirm-code".to_string()))?
        .trim();
    if hash.is_empty() {
        return Err(TeleError::Usage(
            "--phone-hash must not be empty".to_string(),
        ));
    }
    Ok(PhoneAction::Confirm {
        code: code.to_string(),
        hash: hash.to_string(),
    })
}

fn phone_dry_run_data(name: &str, action: &PhoneAction) -> serde_json::Value {
    match action {
        PhoneAction::Send { phone, flashcall } => serde_json::json!({
            "dry_run": true,
            "flashcall": flashcall,
            "would": PhoneAction::Send {
                phone: phone.clone(),
                flashcall: *flashcall,
            }
            .describe(name),
        }),
        other => serde_json::json!({
            "dry_run": true,
            "would": other.describe(name),
        }),
    }
}

async fn send_change_phone_code(
    client: &grammers_client::Client,
    phone: &str,
    flashcall: bool,
) -> TeleResult<String> {
    use grammers_client::tl::{self};
    let request = tl::functions::account::SendChangePhoneCode {
        phone_number: phone.to_string(),
        settings: tl::types::CodeSettings {
            allow_flashcall: flashcall,
            current_number: false,
            allow_app_hash: false,
            allow_missed_call: false,
            allow_firebase: false,
            logout_tokens: None,
            token: None,
            app_sandbox: None,
            unknown_number: false,
        }
        .into(),
    };
    match client.invoke(&request).await {
        Ok(tl::enums::auth::SentCode::Code(code)) => Ok(code.phone_code_hash),
        Ok(tl::enums::auth::SentCode::Success(_)) => Err(TeleError::Auth(
            "server reports the number is already active".to_string(),
        )),
        Ok(tl::enums::auth::SentCode::PaymentRequired(x)) => Err(TeleError::Other(format!(
            "verification requires a paid product ({})",
            x.store_product
        ))),
        Err(e) => Err(tele_invocation(e)),
    }
}

async fn confirm_change_phone(
    guard: &ClientGuard,
    name: &str,
    pending: &PendingPhone,
    action: &PhoneAction,
) -> TeleResult<serde_json::Value> {
    let PhoneAction::Confirm { code, hash } = action else {
        return Err(TeleError::Other(
            "internal error: confirm called without a confirmation action".to_string(),
        ));
    };
    if !phone_hash_matches(pending, hash) {
        remove_pending_phone_under(&config::app_data_dir(), name).ok();
        return Err(TeleError::Usage(
            "--phone-hash does not match the pending change-phone request; run tele account phone --change-phone again"
                .to_string(),
        ));
    }
    guard.rate_limiter.acquire().await;
    let request = grammers_client::tl::functions::account::ChangePhone {
        phone_number: pending.phone.clone(),
        phone_code_hash: pending.phone_code_hash.clone(),
        phone_code: code.to_string(),
    };
    let response = guard.client.invoke(&request).await;
    match response {
        Ok(grammers_client::tl::enums::User::User(user)) => {
            remove_pending_phone(name)?;
            Ok(serde_json::json!({
                "changed": true,
                "user_id": user.id,
                "username": user.username,
            }))
        }
        Ok(grammers_client::tl::enums::User::Empty(_)) => Err(TeleError::Other(
            "server returned an empty user after changing the phone".to_string(),
        )),
        Err(e) => Err(tele_invocation(e)),
    }
}

async fn execute_phone_action(
    guard: &ClientGuard,
    name: &str,
    action: &PhoneAction,
) -> TeleResult<serde_json::Value> {
    match action {
        PhoneAction::Send { phone, flashcall } => {
            guard.rate_limiter.acquire().await;
            let phone_code_hash = send_change_phone_code(&guard.client, phone, *flashcall).await?;
            save_pending_phone(&PendingPhone::new(name, phone, phone_code_hash.clone()))?;
            log_line(
                "info",
                &format!(
                    "change-phone code sent to {}; finish with tele account phone --confirm-code <CODE> --phone-hash {phone_code_hash}",
                    redact_phone(phone)
                ),
            );
            Ok(serde_json::json!({
                "sent": true,
                "to": redact_phone(phone),
                "phone_code_hash": phone_code_hash,
            }))
        }
        confirm @ PhoneAction::Confirm { .. } => {
            let pending = require_pending_phone(name)?;
            let value = confirm_change_phone(guard, name, &pending, confirm).await?;
            log_line("info", &format!("phone changed for account {name}"));
            Ok(value)
        }
    }
}

async fn phone(args: &PhoneArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let action = validate_phone_modes(args)?;
    require_explicit_selection("account phone", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let action = action.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(phone_dry_run_data(&name, &action));
            }
            let credentials = creds()?;
            let guard =
                ClientGuard::connect(&name, credentials.api_id, config_path.as_deref()).await?;
            execute_phone_action(&guard, &name, &action).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

const SESSION_REMOVE_RETRIES: usize = 20;
const SESSION_REMOVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

async fn fetch_authorizations(
    client: &grammers_client::Client,
) -> TeleResult<Vec<grammers_client::tl::types::Authorization>> {
    use grammers_client::tl::{self, enums};
    let response = client
        .invoke(&tl::functions::account::GetAuthorizations {})
        .await
        .map_err(tele_invocation)?;
    let enums::account::Authorizations::Authorizations(listed) = response;
    Ok(listed
        .authorizations
        .into_iter()
        .map(|auth| match auth {
            enums::Authorization::Authorization(auth) => auth,
        })
        .collect())
}

async fn sessions(args: &SessionsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let mode = validate_sessions_modes(args)?;
    if mode.is_mutator() {
        require_explicit_selection("account sessions", flags)?;
    }
    let credentials = creds()?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let credentials = credentials.clone();
        Box::pin(async move {
            if dry_run {
                if let Some(would) = mode.dry_run_description() {
                    return Ok(serde_json::json!({
                        "dry_run": true,
                        "would": would,
                    }));
                }
            }
            match mode {
                SessionsMode::List => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let authorizations = fetch_authorizations(&guard.client).await?;
                    drop(guard);
                    let rows: Vec<serde_json::Value> =
                        authorizations.iter().map(authorization_row).collect();
                    Ok(serde_json::json!({"count": rows.len(), "authorizations": rows}))
                }
                SessionsMode::ListWeb => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let webs = fetch_web_authorizations(&guard.client).await?;
                    drop(guard);
                    let rows: Vec<serde_json::Value> =
                        webs.iter().map(web_authorization_row).collect();
                    Ok(serde_json::json!({
                        "count": rows.len(),
                        "web": true,
                        "authorizations": rows
                    }))
                }
                SessionsMode::Terminate(hash) => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let authorizations = fetch_authorizations(&guard.client).await?;
                    terminate_decision(hash, &authorizations)?;
                    let result = guard
                        .client
                        .invoke(
                            &grammers_client::tl::functions::account::ResetAuthorization { hash },
                        )
                        .await;
                    drop(guard);
                    match result {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(TeleError::Other(format!(
                                "server refused to terminate authorization {hash}"
                            )));
                        }
                        Err(e) => return Err(tele_invocation(e)),
                    }
                    Ok(serde_json::json!({ "terminated": true, "hash": hash }))
                }
                SessionsMode::TerminateWeb(hash) => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let webs = fetch_web_authorizations(&guard.client).await?;
                    web_hash_decision(hash, &webs)?;
                    let result = guard
                        .client
                        .invoke(
                            &grammers_client::tl::functions::account::ResetWebAuthorization {
                                hash,
                            },
                        )
                        .await;
                    drop(guard);
                    match result {
                        Ok(true) => Ok(serde_json::json!({
                            "terminated": true,
                            "web": true,
                            "hash": hash
                        })),
                        Ok(false) => Err(TeleError::Other(format!(
                            "server refused to terminate web authorization {hash}"
                        ))),
                        Err(e) => Err(tele_invocation(e)),
                    }
                }
                SessionsMode::TerminateAllWeb => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let result = guard
                        .client
                        .invoke(&grammers_client::tl::functions::account::ResetWebAuthorizations {})
                        .await;
                    drop(guard);
                    match result {
                        Ok(true) => Ok(serde_json::json!({ "terminated_all": true, "web": true })),
                        Ok(false) => Err(TeleError::Other(
                            "server refused to terminate web authorizations".to_string(),
                        )),
                        Err(e) => Err(tele_invocation(e)),
                    }
                }
                SessionsMode::ChangeFlags {
                    hash,
                    encrypted_requests,
                    call_requests,
                } => {
                    let guard =
                        ClientGuard::connect(&name, credentials.api_id, config_path.as_deref())
                            .await?;
                    guard.rate_limiter.acquire().await;
                    let webs = fetch_web_authorizations(&guard.client).await?;
                    web_hash_decision(hash, &webs)?;
                    let request =
                        grammers_client::tl::functions::account::ChangeAuthorizationSettings {
                            confirmed: false,
                            hash,
                            encrypted_requests_disabled: encrypted_requests,
                            call_requests_disabled: call_requests,
                        };
                    let result = guard.client.invoke(&request).await;
                    drop(guard);
                    match result {
                        Ok(true) => Ok(serde_json::json!({
                            "updated": true,
                            "web": true,
                            "hash": hash,
                            "encrypted_requests_disabled": encrypted_requests,
                            "call_requests_disabled": call_requests,
                        })),
                        Ok(false) => Err(TeleError::Other(format!(
                            "server refused to update web authorization {hash}"
                        ))),
                        Err(e) => Err(tele_invocation(e)),
                    }
                }
            }
        })
    })
    .await?;
    let human = !output::machine_mode(flags.json, flags.jsonl);
    if mode == SessionsMode::List && human {
        let table_rows: Vec<Vec<String>> = envelope
            .accounts
            .iter()
            .filter(|o| o.ok)
            .flat_map(|o| {
                o.data
                    .as_ref()
                    .and_then(|d| d["authorizations"].as_array())
                    .map(|rows| {
                        rows.iter()
                            .map(|row| sessions_table_cells(&o.account, row))
                            .collect::<Vec<Vec<String>>>()
                    })
                    .unwrap_or_default()
            })
            .collect();
        if table_rows.is_empty() {
            output::print_line("no active authorizations")?;
        } else {
            output::print_table(
                &[
                    "account", "hash", "device", "app", "ip", "country", "date", "current",
                ],
                &table_rows,
            )?;
        }
    }
    if mode == SessionsMode::ListWeb && human {
        let table_rows: Vec<Vec<String>> = envelope
            .accounts
            .iter()
            .filter(|o| o.ok)
            .flat_map(|o| {
                o.data
                    .as_ref()
                    .and_then(|d| d["authorizations"].as_array())
                    .map(|rows| {
                        rows.iter()
                            .map(|row| web_table_cells(&o.account, row))
                            .collect::<Vec<Vec<String>>>()
                    })
                    .unwrap_or_default()
            })
            .collect();
        if table_rows.is_empty() {
            output::print_line("no active web sessions")?;
        } else {
            output::print_table(
                &[
                    "account", "hash", "domain", "browser", "platform", "region", "created",
                    "active",
                ],
                &table_rows,
            )?;
        }
    }
    crate::executor::finish(flags, &envelope)
}

async fn password(args: &PasswordArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let action = validate_password_modes(
        args.set,
        args.change,
        args.remove,
        args.confirm_email.as_deref(),
        args.resend_email,
        args.cancel_email,
        args.status,
        args.reset_start,
        args.decline_reset,
        args.hint.as_deref(),
        args.recovery_email.as_deref(),
    )?;
    require_explicit_selection("account password", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let hint_present = args.hint.is_some();
    let email_present = args.recovery_email.is_some();
    let hint = args.hint.clone();
    let email = args.recovery_email.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let hint = hint.clone();
        let email = email.clone();
        let action = action.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "action": action.describe(),
                    "would": action.describe(),
                    "hint": hint_present,
                    "recovery_email": email_present,
                }));
            }
            let credentials = creds()?;
            let guard =
                ClientGuard::connect(&name, credentials.api_id, config_path.as_deref()).await?;
            execute_password_action(&guard, &action, hint.as_deref(), email.as_deref()).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn execute_password_action(
    guard: &ClientGuard,
    action: &PasswordAction,
    hint: Option<&str>,
    email: Option<&str>,
) -> TeleResult<serde_json::Value> {
    match action {
        PasswordAction::Mode(mode) => {
            match mode {
                PasswordMode::Remove => remove_cloud_password(guard).await?,
                PasswordMode::Set => set_cloud_password(guard, hint, email).await?,
                PasswordMode::Change => change_cloud_password(guard, hint, email).await?,
            }
            Ok(serde_json::json!({"updated": true, "mode": mode.as_str()}))
        }
        PasswordAction::ConfirmEmail(code) => {
            confirm_password_email(guard, code).await?;
            Ok(serde_json::json!({"confirmed": true}))
        }
        PasswordAction::ResendEmail => {
            resend_password_email(guard).await?;
            Ok(serde_json::json!({"resent": true}))
        }
        PasswordAction::CancelEmail => {
            cancel_password_email(guard).await?;
            Ok(serde_json::json!({"cancelled": true}))
        }
        PasswordAction::Status => password_status(guard).await,
        PasswordAction::ResetStart => start_password_reset(guard).await,
        PasswordAction::DeclineReset => {
            decline_password_reset(guard).await?;
            Ok(serde_json::json!({"declined": true}))
        }
    }
}

#[allow(dead_code)]
async fn fetch_has_password(client: &grammers_client::Client) -> TeleResult<bool> {
    use grammers_client::tl::{self, enums};
    let response = client
        .invoke(&tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    let enums::account::Password::Password(password) = response;
    Ok(password.has_password)
}

async fn remove_session_file_retry(path: &std::path::Path) -> TeleResult<()> {
    let mut last_error: Option<std::io::Error> = None;
    for _ in 0..SESSION_REMOVE_RETRIES {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                tokio::time::sleep(SESSION_REMOVE_RETRY_DELAY).await;
            }
        }
    }
    Err(TeleError::Other(format!(
        "could not remove session file {}: {}",
        path.display(),
        last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )))
}

fn single_outcome(account: &str, data: serde_json::Value) -> AccountOutcome {
    AccountOutcome {
        account: account.to_string(),
        ok: true,
        error: None,
        data: Some(data),
        exit_code: None,
    }
}

fn action_envelope(
    account: &str,
    data: serde_json::Value,
    dry_run: bool,
    command: &str,
) -> Envelope {
    Envelope::new(vec![single_outcome(account, data)], dry_run, command)
}

fn add_dry_run_data(name: &str, tags: &Option<Vec<String>>, would: &str) -> serde_json::Value {
    serde_json::json!({
        "would": would,
        "dry_run": true,
        "name": name,
        "tags": tags,
    })
}

fn dry_run_envelope(account: &str, would: &str, command: &str) -> Envelope {
    action_envelope(
        account,
        serde_json::json!({"would": would, "dry_run": true}),
        true,
        command,
    )
}

fn list_envelope(rows: &[serde_json::Value], flags: &GlobalFlags) -> TeleResult<serde_json::Value> {
    let outcomes: Vec<AccountOutcome> = rows
        .iter()
        .map(|r| single_outcome(r["name"].as_str().unwrap_or_default(), r.clone()))
        .collect();
    let mut value = serde_json::to_value(Envelope::new(outcomes, flags.dry_run, &flags.command))?;
    value["accounts"] = serde_json::Value::Array(rows.to_vec());
    Ok(value)
}

fn status_table_rows(envelope: &Envelope) -> Vec<Vec<String>> {
    envelope
        .accounts
        .iter()
        .filter(|o| o.ok)
        .map(|o| {
            let authorized = o
                .data
                .as_ref()
                .and_then(|d| d["authorized"].as_bool())
                .unwrap_or(false);
            vec![
                o.account.clone(),
                if authorized { "yes" } else { "no" }.to_string(),
                status_device_summary(o.data.as_ref().and_then(|d| d.get("device"))),
            ]
        })
        .collect()
}

fn code_prompt(phone: Option<&str>, stderr_is_terminal: bool) -> String {
    match phone {
        Some(phone) if stderr_is_terminal => format!("Enter the code sent to {phone}: "),
        _ => "Enter the code: ".to_string(),
    }
}

fn argv_phone_warning(phone: Option<&str>, _stderr_is_terminal: bool) -> Option<&'static str> {
    phone.map(|_| {
        "--phone is visible in process listings and shell history; prefer TELE_PHONE or an interactive prompt"
    })
}

fn prompt_line(
    prompt: &str,
    reader: &mut impl std::io::BufRead,
    writer: &mut impl std::io::Write,
) -> TeleResult<Option<String>> {
    writer.write_all(prompt.as_bytes())?;
    writer.flush()?;
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

fn strip_line_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn authorization_row(auth: &grammers_client::tl::types::Authorization) -> serde_json::Value {
    let date = |ts: i32| {
        chrono::DateTime::from_timestamp(ts as i64, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default()
    };
    let app = if auth.app_name.is_empty() {
        format!("api_id {}", auth.api_id)
    } else if auth.app_version.is_empty() {
        auth.app_name.clone()
    } else {
        format!("{} {}", auth.app_name, auth.app_version)
    };
    serde_json::json!({
        "hash": auth.hash,
        "device_model": auth.device_model,
        "platform": auth.platform,
        "system_version": auth.system_version,
        "app": app,
        "ip": auth.ip,
        "country": auth.country,
        "region": auth.region,
        "date_created": date(auth.date_created),
        "date_active": date(auth.date_active),
        "current": auth.current,
        "official_app": auth.official_app,
        "password_pending": auth.password_pending,
        "unconfirmed": auth.unconfirmed,
    })
}

fn sessions_table_cells(account: &str, row: &serde_json::Value) -> Vec<String> {
    vec![
        account.to_string(),
        row["hash"].to_string(),
        row["device_model"].as_str().unwrap_or_default().to_string(),
        row["app"].as_str().unwrap_or_default().to_string(),
        row["ip"].as_str().unwrap_or_default().to_string(),
        row["country"].as_str().unwrap_or_default().to_string(),
        row["date_created"].as_str().unwrap_or_default().to_string(),
        if row["current"].as_bool() == Some(true) {
            "yes"
        } else {
            ""
        }
        .to_string(),
    ]
}

fn terminate_decision(
    hash: i64,
    authorizations: &[grammers_client::tl::types::Authorization],
) -> TeleResult<()> {
    match authorizations.iter().find(|a| a.hash == hash) {
        Some(auth) if auth.current => Err(TeleError::Usage(format!(
            "refusing to terminate the current session (hash {hash}); pick the hash of another device session from tele account sessions"
        ))),
        Some(_) => Ok(()),
        None => Err(TeleError::Usage(format!(
            "no active authorization with hash {hash}; run tele account sessions to list valid hashes"
        ))),
    }
}

async fn fetch_web_authorizations(
    client: &grammers_client::Client,
) -> TeleResult<Vec<grammers_client::tl::types::WebAuthorization>> {
    use grammers_client::tl::{self, enums};
    let response = client
        .invoke(&tl::functions::account::GetWebAuthorizations {})
        .await
        .map_err(tele_invocation)?;
    let enums::account::WebAuthorizations::Authorizations(listed) = response;
    Ok(listed
        .authorizations
        .into_iter()
        .map(|auth| match auth {
            enums::WebAuthorization::Authorization(inner) => inner,
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionsMode {
    List,
    ListWeb,
    Terminate(i64),
    TerminateWeb(i64),
    TerminateAllWeb,
    ChangeFlags {
        hash: i64,
        encrypted_requests: Option<bool>,
        call_requests: Option<bool>,
    },
}

impl SessionsMode {
    fn is_mutator(self) -> bool {
        !matches!(self, SessionsMode::List | SessionsMode::ListWeb)
    }

    fn dry_run_description(self) -> Option<String> {
        match self {
            SessionsMode::List | SessionsMode::ListWeb => None,
            SessionsMode::Terminate(hash) => Some(format!("terminate authorization {hash}")),
            SessionsMode::TerminateWeb(hash) => Some(format!("terminate web authorization {hash}")),
            SessionsMode::TerminateAllWeb => Some("terminate all web authorizations".to_string()),
            SessionsMode::ChangeFlags {
                hash,
                encrypted_requests,
                call_requests,
            } => {
                let mut parts = Vec::new();
                if let Some(v) = encrypted_requests {
                    parts.push(format!("disable encrypted requests={v}"));
                }
                if let Some(v) = call_requests {
                    parts.push(format!("disable call requests={v}"));
                }
                Some(format!(
                    "update web authorization {hash} settings: {}",
                    parts.join("; ")
                ))
            }
        }
    }
}

fn parse_bool_arg(value: &str, flag: &str) -> TeleResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(TeleError::Usage(format!(
            "invalid {flag} value \"{other}\"; use true or false"
        ))),
    }
}

fn validate_sessions_modes(args: &SessionsArgs) -> TeleResult<SessionsMode> {
    let primaries = usize::from(args.terminate.is_some())
        + usize::from(args.terminate_web.is_some())
        + usize::from(args.terminate_all_web)
        + usize::from(args.change_flags.is_some());
    if primaries > 1 || (primaries >= 1 && args.web) {
        return Err(TeleError::Usage(
            "--web, --terminate, --terminate-web, --terminate-all-web and --change-flags are mutually exclusive"
                .to_string(),
        ));
    }
    if let Some(hash) = args.change_flags {
        let encrypted_requests = args
            .disable_encrypted
            .as_deref()
            .map(|v| parse_bool_arg(v, "--disable-encrypted"))
            .transpose()?;
        let call_requests = args
            .disable_call_requests
            .as_deref()
            .map(|v| parse_bool_arg(v, "--disable-call-requests"))
            .transpose()?;
        if encrypted_requests.is_none() && call_requests.is_none() {
            return Err(TeleError::Usage(
                "--change-flags needs --disable-encrypted and/or --disable-call-requests"
                    .to_string(),
            ));
        }
        return Ok(SessionsMode::ChangeFlags {
            hash,
            encrypted_requests,
            call_requests,
        });
    }
    if args.disable_encrypted.is_some() || args.disable_call_requests.is_some() {
        return Err(TeleError::Usage(
            "--disable-encrypted/--disable-call-requests only apply with --change-flags HASH"
                .to_string(),
        ));
    }
    if let Some(hash) = args.terminate_web {
        return Ok(SessionsMode::TerminateWeb(hash));
    }
    if args.terminate_all_web {
        return Ok(SessionsMode::TerminateAllWeb);
    }
    if let Some(hash) = args.terminate {
        return Ok(SessionsMode::Terminate(hash));
    }
    if args.web {
        return Ok(SessionsMode::ListWeb);
    }
    Ok(SessionsMode::List)
}

fn web_authorization_row(web: &grammers_client::tl::types::WebAuthorization) -> serde_json::Value {
    let date = |ts: i32| {
        chrono::DateTime::from_timestamp(ts as i64, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default()
    };
    serde_json::json!({
        "hash": web.hash,
        "bot_id": web.bot_id,
        "domain": web.domain,
        "browser": web.browser,
        "platform": web.platform,
        "ip": web.ip,
        "region": web.region,
        "date_created": date(web.date_created),
        "date_active": date(web.date_active),
    })
}

fn web_table_cells(account: &str, row: &serde_json::Value) -> Vec<String> {
    vec![
        account.to_string(),
        row["hash"].to_string(),
        row["domain"].as_str().unwrap_or_default().to_string(),
        row["browser"].as_str().unwrap_or_default().to_string(),
        row["platform"].as_str().unwrap_or_default().to_string(),
        row["region"].as_str().unwrap_or_default().to_string(),
        row["date_created"].as_str().unwrap_or_default().to_string(),
        row["date_active"].as_str().unwrap_or_default().to_string(),
    ]
}

fn web_hash_decision(
    hash: i64,
    webs: &[grammers_client::tl::types::WebAuthorization],
) -> TeleResult<()> {
    if webs.iter().any(|w| w.hash == hash) {
        Ok(())
    } else {
        Err(TeleError::Usage(format!(
            "no active web session with hash {hash}; run tele account sessions --web to list valid hashes"
        )))
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_password_modes(
    set: bool,
    change: bool,
    remove: bool,
    confirm_email: Option<&str>,
    resend_email: bool,
    cancel_email: bool,
    status: bool,
    reset_start: bool,
    decline_reset: bool,
    hint: Option<&str>,
    recovery_email: Option<&str>,
) -> TeleResult<PasswordAction> {
    let primary = usize::from(set)
        + usize::from(change)
        + usize::from(remove)
        + usize::from(confirm_email.is_some())
        + usize::from(resend_email)
        + usize::from(cancel_email)
        + usize::from(status)
        + usize::from(reset_start)
        + usize::from(decline_reset);
    if primary == 0 {
        return Err(TeleError::Usage(
            "choose exactly one of --set, --change, --remove, --confirm-email, --resend-email, --cancel-email, --status, --reset-start or --decline-reset".to_string(),
        ));
    }
    if primary > 1 {
        return Err(TeleError::Usage(
            "password mode flags are mutually exclusive".to_string(),
        ));
    }
    if hint.is_some_and(|h| h.trim().is_empty()) {
        return Err(TeleError::Usage("--hint must not be empty".to_string()));
    }
    if recovery_email.is_some_and(|e| e.trim().is_empty()) {
        return Err(TeleError::Usage(
            "--recovery-email must not be empty".to_string(),
        ));
    }
    let set_or_change = set || change;
    if !set_or_change {
        if hint.is_some() {
            return Err(TeleError::Usage(
                "--hint only applies to --set or --change".to_string(),
            ));
        }
        if recovery_email.is_some() {
            return Err(TeleError::Usage(
                "--recovery-email only applies to --set or --change".to_string(),
            ));
        }
    }
    if let Some(code) = confirm_email {
        if code.trim().is_empty() {
            return Err(TeleError::Usage(
                "--confirm-email must not be empty".to_string(),
            ));
        }
        return Ok(PasswordAction::ConfirmEmail(code.trim().to_string()));
    }
    if resend_email {
        return Ok(PasswordAction::ResendEmail);
    }
    if cancel_email {
        return Ok(PasswordAction::CancelEmail);
    }
    if status {
        return Ok(PasswordAction::Status);
    }
    if reset_start {
        return Ok(PasswordAction::ResetStart);
    }
    if decline_reset {
        return Ok(PasswordAction::DeclineReset);
    }
    if set {
        return Ok(PasswordAction::Mode(PasswordMode::Set));
    }
    if change {
        return Ok(PasswordAction::Mode(PasswordMode::Change));
    }
    Ok(PasswordAction::Mode(PasswordMode::Remove))
}

const NO_SRP_CHALLENGE_MSG: &str =
    "GetPassword response is missing SRP challenge parameters; retry the command";

fn plan_password_step(mode: PasswordMode, has_password: bool) -> TeleResult<()> {
    match (mode, has_password) {
        (PasswordMode::Set, true) => Err(TeleError::Usage(
            "a cloud password is already set on this account; use --change".to_string(),
        )),
        (PasswordMode::Set, false) => Ok(()),
        (PasswordMode::Change, false) | (PasswordMode::Remove, false) => Err(TeleError::Usage(
            "no cloud password is set on this account; use --set".to_string(),
        )),
        (PasswordMode::Change, true) => Ok(()),
        (PasswordMode::Remove, true) => Ok(()),
    }
}

fn sh(data: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(data);
    hasher.update(salt);
    hasher.finalize().into()
}

fn ph1(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    sh(&sh(password, salt1), salt2)
}

fn ph2(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    let hash1 = ph1(password, salt1, salt2);
    let mut dk = [0u8; 64];
    pbkdf2::pbkdf2::<Hmac<Sha512>>(&hash1, salt1, 100000, &mut dk).unwrap();
    sh(&dk, salt2)
}

fn compute_new_password_hash(
    password: &str,
    salt1: &[u8],
    salt2: &[u8],
    g: i32,
    p: &[u8],
) -> Vec<u8> {
    let x = ph2(password.as_bytes(), salt1, salt2);
    let big_x = BigUint::from_bytes_be(&x);
    let big_p = BigUint::from_bytes_be(p);
    let big_g = BigUint::from(g as u32);
    let big_v = big_g.modpow(&big_x, &big_p);
    let mut v = big_v.to_bytes_be();
    if v.len() > 256 {
        v = v[v.len() - 256..].to_vec();
    }
    let mut out = vec![0u8; 256 - v.len()];
    out.extend_from_slice(&v);
    out
}

fn extend_salt1(base_salt1: &[u8], extra: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(base_salt1.len() + 32);
    out.extend_from_slice(base_salt1);
    out.extend_from_slice(extra);
    out
}

fn generate_secure_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("secure random failed");
    buf
}

fn new_password_algo_and_hash(
    password: &str,
    base: &grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow,
    extra: Option<[u8; 32]>,
) -> (grammers_client::tl::enums::PasswordKdfAlgo, Vec<u8>) {
    let extra = extra.unwrap_or_else(generate_secure_32);
    let new_salt1 = extend_salt1(&base.salt1, &extra);
    let hash = compute_new_password_hash(password, &new_salt1, &base.salt2, base.g, &base.p);
    let algo = grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
        salt1: new_salt1,
        salt2: base.salt2.clone(),
        g: base.g,
        p: base.p.clone(),
    };
    let algo_enum = grammers_client::tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(algo);
    (algo_enum, hash)
}

fn extract_new_algo(
    algo: &grammers_client::tl::enums::PasswordKdfAlgo,
) -> TeleResult<
    grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow,
> {
    match algo {
        grammers_client::tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(inner) => Ok(grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
            salt1: inner.salt1.clone(),
            salt2: inner.salt2.clone(),
            g: inner.g,
            p: inner.p.clone(),
        }),
        grammers_client::tl::enums::PasswordKdfAlgo::Unknown => Err(TeleError::Other(
            "server sent an unsupported cloud-password KDF algorithm; cannot build new password hash".to_string(),
        )),
    }
}

fn prompt_password_with_echo(prompt: &str) -> TeleResult<String> {
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let echo_disabled = disable_stdin_echo();
    if !echo_disabled {
        log_line(
            "warn",
            "secure password input unavailable; input will be echoed to the terminal",
        );
    }
    let read = prompt_line(prompt, &mut stdin, &mut stderr);
    restore_stdin_echo(echo_disabled);
    let _ = writeln!(stderr);
    let Some(line) = read? else {
        return Err(TeleError::Auth(
            "password required; stdin closed".to_string(),
        ));
    };
    Ok(strip_line_ending(&line).to_string())
}

fn prompt_new_password_pair() -> TeleResult<String> {
    let first = prompt_password_with_echo("Enter new cloud password: ")?;
    if first.is_empty() {
        return Err(TeleError::Usage("password must not be empty".to_string()));
    }
    let second = prompt_password_with_echo("Confirm new cloud password: ")?;
    if first != second {
        return Err(TeleError::Usage("passwords do not match".to_string()));
    }
    Ok(first)
}

async fn set_cloud_password(
    guard: &ClientGuard,
    hint: Option<&str>,
    email: Option<&str>,
) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    let response = guard
        .client
        .invoke(&grammers_client::tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    let grammers_client::tl::enums::account::Password::Password(pw) = response;
    plan_password_step(PasswordMode::Set, pw.has_password)?;
    let base = extract_new_algo(&pw.new_algo)?;
    let new_password = prompt_new_password_pair()?;
    let (algo, hash) = new_password_algo_and_hash(&new_password, &base, None);
    let new_settings = grammers_client::tl::enums::account::PasswordInputSettings::Settings(
        grammers_client::tl::types::account::PasswordInputSettings {
            new_algo: Some(algo),
            new_password_hash: Some(hash),
            hint: hint.map(|s| s.to_string()),
            email: email.map(|s| s.to_string()),
            new_secure_settings: None,
        },
    );
    let empty = grammers_client::tl::enums::InputCheckPasswordSrp::InputCheckPasswordEmpty;
    update_settings_with_email_loop(guard, empty, new_settings).await
}

async fn update_settings_with_email_loop(
    guard: &ClientGuard,
    proof: grammers_client::tl::enums::InputCheckPasswordSrp,
    new_settings: grammers_client::tl::enums::account::PasswordInputSettings,
) -> TeleResult<()> {
    for attempt in 1..=MAX_CODE_ATTEMPTS {
        guard.rate_limiter.acquire().await;
        let req = grammers_client::tl::functions::account::UpdatePasswordSettings {
            password: proof.clone(),
            new_settings: new_settings.clone(),
        };
        match guard.client.invoke(&req).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                if !is_email_unconfirmed(&e) || attempt == MAX_CODE_ATTEMPTS {
                    return Err(map_update_password_error(e));
                }
                confirm_pending_password_email(guard).await?;
            }
        }
    }
    Err(TeleError::Other(
        "recovery-email confirmation did not complete".to_string(),
    ))
}

fn is_email_unconfirmed(e: &grammers_client::InvocationError) -> bool {
    e.to_string().contains("EMAIL_UNCONFIRMED")
}

async fn fetch_password(
    guard: &ClientGuard,
) -> TeleResult<grammers_client::tl::types::account::Password> {
    guard.rate_limiter.acquire().await;
    let response = guard
        .client
        .invoke(&grammers_client::tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    match response {
        grammers_client::tl::enums::account::Password::Password(pw) => Ok(pw),
    }
}

async fn confirm_pending_password_email(guard: &ClientGuard) -> TeleResult<()> {
    let pw = fetch_password(guard).await?;
    if let Some(pattern) = &pw.email_unconfirmed_pattern {
        output::log_line(
            "info",
            &format!("confirmation code sent to email matching {pattern}"),
        );
    }
    let code = prompt_plain_line("Enter the code sent to that email: ")?;
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::ConfirmPasswordEmail { code })
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

fn prompt_plain_line(prompt: &str) -> TeleResult<String> {
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let read = prompt_line(prompt, &mut stdin, &mut stderr);
    let _ = writeln!(stderr);
    let Some(line) = read? else {
        return Err(TeleError::Usage("input required; stdin closed".to_string()));
    };
    Ok(strip_line_ending(&line).to_string())
}

async fn confirm_password_email(guard: &ClientGuard, code: &str) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(
            &grammers_client::tl::functions::account::ConfirmPasswordEmail {
                code: code.to_string(),
            },
        )
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

async fn resend_password_email(guard: &ClientGuard) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::ResendPasswordEmail {})
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

async fn cancel_password_email(guard: &ClientGuard) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::CancelPasswordEmail {})
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

async fn password_status(guard: &ClientGuard) -> TeleResult<serde_json::Value> {
    let pw = fetch_password(guard).await?;
    Ok(serde_json::json!({
        "has_password": pw.has_password,
        "has_recovery": pw.has_recovery,
        "hint": pw.hint,
        "email_unconfirmed_pattern": pw.email_unconfirmed_pattern,
        "pending_reset_date": pw.pending_reset_date.map(|d| {
            chrono::DateTime::from_timestamp(d as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        }),
    }))
}

async fn start_password_reset(guard: &ClientGuard) -> TeleResult<serde_json::Value> {
    guard.rate_limiter.acquire().await;
    let result = guard
        .client
        .invoke(&grammers_client::tl::functions::account::ResetPassword {})
        .await
        .map_err(tele_invocation)?;
    let value = match result {
        grammers_client::tl::enums::account::ResetPasswordResult::ResetPasswordOk => {
            serde_json::json!({"result": "reset"})
        }
        grammers_client::tl::enums::account::ResetPasswordResult::ResetPasswordRequestedWait(w) => {
            serde_json::json!({
                "result": "wait",
                "until_date": chrono::DateTime::from_timestamp(w.until_date as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })
        }
        grammers_client::tl::enums::account::ResetPasswordResult::ResetPasswordFailedWait(w) => {
            serde_json::json!({
                "result": "failed_wait",
                "retry_date": chrono::DateTime::from_timestamp(w.retry_date as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })
        }
    };
    Ok(value)
}

async fn decline_password_reset(guard: &ClientGuard) -> TeleResult<()> {
    let pw = fetch_password(guard).await?;
    if !pw.has_password {
        return Err(TeleError::Usage(
            "no cloud password is set on this account".to_string(),
        ));
    }
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::DeclinePasswordReset {})
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

async fn change_cloud_password(
    guard: &ClientGuard,
    hint: Option<&str>,
    email: Option<&str>,
) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    let response = guard
        .client
        .invoke(&grammers_client::tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    let grammers_client::tl::enums::account::Password::Password(pw) = response;
    plan_password_step(PasswordMode::Change, pw.has_password)?;
    let params = extract_srp_params(pw.current_algo.as_ref())?;
    let srp_b = pw
        .srp_b
        .clone()
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    let srp_id = pw
        .srp_id
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    let random_a = pw.secure_random.clone();
    let base = extract_new_algo(&pw.new_algo)?;
    let current = prompt_password_with_echo("Enter current cloud password: ")?;
    if current.is_empty() {
        return Err(TeleError::Usage("password must not be empty".to_string()));
    }
    let new_password = prompt_new_password_pair()?;
    let proof = input_check_password_srp(&params, srp_id, &srp_b, &random_a, &current)?;
    let (algo, hash) = new_password_algo_and_hash(&new_password, &base, None);
    let new_settings = grammers_client::tl::enums::account::PasswordInputSettings::Settings(
        grammers_client::tl::types::account::PasswordInputSettings {
            new_algo: Some(algo),
            new_password_hash: Some(hash),
            hint: hint.map(|s| s.to_string()),
            email: email.map(|s| s.to_string()),
            new_secure_settings: None,
        },
    );
    guard.rate_limiter.acquire().await;
    update_settings_with_email_loop(guard, proof, new_settings).await
}

#[derive(Debug)]
struct SrpParams {
    salt1: Vec<u8>,
    salt2: Vec<u8>,
    p: Vec<u8>,
    g: i32,
}

fn extract_srp_params(
    algo: Option<&grammers_client::tl::enums::PasswordKdfAlgo>,
) -> TeleResult<SrpParams> {
    use grammers_client::tl::{self, enums};
    match algo {
        Some(enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(
            tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
                salt1,
                salt2,
                p,
                g,
            },
        )) => Ok(SrpParams {
            salt1: salt1.clone(),
            salt2: salt2.clone(),
            p: p.clone(),
            g: *g,
        }),
        Some(enums::PasswordKdfAlgo::Unknown) | None => Err(TeleError::Other(
            "server sent an unsupported cloud-password KDF algorithm; cannot build SRP proof"
                .to_string(),
        )),
    }
}

fn input_check_password_srp(
    params: &SrpParams,
    srp_id: i64,
    srp_b: &[u8],
    random_a: &[u8],
    password: &str,
) -> TeleResult<grammers_client::tl::enums::InputCheckPasswordSrp> {
    use grammers_client::tl::{self, enums};
    use grammers_crypto::two_factor_auth::{calculate_2fa, check_p_and_g};
    if !(2..=7).contains(&params.g) {
        return Err(TeleError::Other(format!(
            "server sent unsupported SRP generator g={}; cannot build proof",
            params.g
        )));
    }
    if !check_p_and_g(&params.p, &params.g) {
        return Err(TeleError::Other(
            "server sent invalid SRP prime parameters; cannot build proof".to_string(),
        ));
    }
    let (m1, g_a) = calculate_2fa(
        &params.salt1,
        &params.salt2,
        &params.p,
        &params.g,
        srp_b.to_vec(),
        random_a.to_vec(),
        password,
    );
    Ok(enums::InputCheckPasswordSrp::Srp(
        tl::types::InputCheckPasswordSrp {
            srp_id,
            a: g_a.to_vec(),
            m1: m1.to_vec(),
        },
    ))
}

fn map_update_password_error(e: grammers_client::InvocationError) -> TeleError {
    if let grammers_client::InvocationError::Rpc(rpc) = &e {
        if rpc.name == "PASSWORD_HASH_INVALID" {
            return TeleError::Auth("invalid cloud password; nothing was changed".to_string());
        }
        if matches!(
            rpc.name.as_str(),
            "NEW_SETTINGS_EMPTY" | "INPUT_FETCH_ERROR" | "INPUT_CONSTRUCTOR_INVALID"
        ) {
            return TeleError::Other(format!(
                "{e} — Telegram rejected this payload (known grammers TL limitation for \
UpdatePasswordSettings); disable/change via an official app: Settings → Privacy and Security → \
Two-Step Verification"
            ));
        }
    }
    tele_invocation(e)
}

fn prompt_current_password_proof(
    params: &SrpParams,
    srp_id: i64,
    srp_b: &[u8],
    random_a: &[u8],
    prompt: &str,
) -> TeleResult<grammers_client::tl::enums::InputCheckPasswordSrp> {
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let echo_disabled = disable_stdin_echo();
    if !echo_disabled {
        log_line(
            "warn",
            "secure password input unavailable; input will be echoed to the terminal",
        );
    }
    let read = prompt_line(prompt, &mut stdin, &mut stderr);
    restore_stdin_echo(echo_disabled);
    let Some(password_line) = read? else {
        return Err(TeleError::Auth(
            "cloud password required to remove it; stdin closed".to_string(),
        ));
    };
    let current = strip_line_ending(&password_line);
    input_check_password_srp(params, srp_id, srp_b, random_a, current)
}

async fn remove_cloud_password(guard: &ClientGuard) -> TeleResult<()> {
    use grammers_client::tl::{self, enums};
    guard.rate_limiter.acquire().await;
    let response = guard
        .client
        .invoke(&tl::functions::account::GetPassword {})
        .await
        .map_err(tele_invocation)?;
    let enums::account::Password::Password(password) = response;
    if !password.has_password {
        return Err(TeleError::Usage(
            "no cloud password is set on this account; use --set".to_string(),
        ));
    }
    let params = extract_srp_params(password.current_algo.as_ref())?;
    let srp_b = password
        .srp_b
        .clone()
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    let srp_id = password
        .srp_id
        .ok_or_else(|| TeleError::Other(NO_SRP_CHALLENGE_MSG.to_string()))?;
    let proof = prompt_current_password_proof(
        &params,
        srp_id,
        &srp_b,
        &password.secure_random,
        "Enter the current cloud password to remove it: ",
    )?;
    guard.rate_limiter.acquire().await;
    let request = tl::functions::account::UpdatePasswordSettings {
        password: proof,
        new_settings: enums::account::PasswordInputSettings::Settings(
            tl::types::account::PasswordInputSettings {
                new_algo: Some(grammers_client::tl::enums::PasswordKdfAlgo::Unknown),
                new_password_hash: None,
                hint: None,
                email: None,
                new_secure_settings: None,
            },
        ),
    };
    match guard.client.invoke(&request).await {
        Ok(_) => Ok(()),
        Err(e) => Err(map_update_password_error(e)),
    }
}

#[cfg(windows)]
fn disable_stdin_echo() -> bool {
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
    };
    unsafe {
        let Ok(handle) = GetStdHandle(STD_INPUT_HANDLE) else {
            return false;
        };
        let mut mode = Default::default();
        if GetConsoleMode(handle, &mut mode).is_err() {
            return false;
        }
        SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT).is_ok()
    }
}

#[cfg(windows)]
fn restore_stdin_echo(disabled: bool) {
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
    };
    if !disabled {
        return;
    }
    unsafe {
        let Ok(handle) = GetStdHandle(STD_INPUT_HANDLE) else {
            return;
        };
        let mut mode = Default::default();
        if GetConsoleMode(handle, &mut mode).is_err() {
            return;
        }
        let _ = SetConsoleMode(handle, mode | ENABLE_ECHO_INPUT);
    }
}

#[cfg(not(windows))]
fn disable_stdin_echo() -> bool {
    false
}

#[cfg(not(windows))]
fn restore_stdin_echo(_disabled: bool) {}

fn ensure_account_config_entry(
    name: &str,
    config_path: Option<&std::path::Path>,
) -> TeleResult<()> {
    let mut cfg = config::load_config(config_path)?;
    if cfg.accounts.contains_key(name) {
        return Ok(());
    }
    cfg.accounts
        .entry(name.to_string())
        .or_insert_with(config::AccountConfig::default);
    let path = config_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config::app_data_dir().join("config.toml"));
    config::write_config(&path, &cfg)
        .map_err(|e| TeleError::Config(format!("failed to write config: {e:#}")))?;
    log_line(
        "info",
        &format!("account {} registered in {}", name, path.display()),
    );
    Ok(())
}

const QR_TOKEN_NON_TTY_WARNING: &str =
    "printing one-time login token to a non-terminal stderr; treat the output as a secret";

fn qr_token_lines(uri: &str, stderr_is_terminal: bool) -> (Option<&'static str>, String) {
    let warning = if stderr_is_terminal {
        None
    } else {
        Some(QR_TOKEN_NON_TTY_WARNING)
    };
    (warning, format!("URI: {uri}"))
}

fn render_qr(uri: &str, show_token: bool) {
    eprintln!("Scan this QR code with Telegram (Settings > Devices > Link Desktop Device):");
    match qrcode::QrCode::new(uri.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<char>()
                .quiet_zone(true)
                .module_dimensions(2, 1)
                .build();
            let _ = writeln!(std::io::stderr(), "{rendered}");
        }
        Err(_) => {
            if should_print_token(show_token, std::io::stderr().is_terminal()) {
                let (warning, line) = qr_token_lines(uri, std::io::stderr().is_terminal());
                if let Some(warning) = warning {
                    output::log_line("warn", warning);
                }
                let _ = writeln!(std::io::stderr(), "{line}");
            } else {
                output::log_line(
                    "warn",
                    "QR rendering failed; re-run with --show-token to print the login URI",
                );
            }
        }
    }
}

fn should_print_token(show_token: bool, stderr_is_terminal: bool) -> bool {
    show_token || stderr_is_terminal
}

pub(crate) fn account_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let guard = crate::config::TEST_ENV_LOCK.lock().await;
        std::env::set_var("TELE_APP_DIR", &dir);
        std::env::remove_var(TELE_PHONE_ENV);
        let result = f(dir.clone()).await;
        std::env::remove_var("TELE_APP_DIR");
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn test_flags(
        command: &str,
        json: bool,
        dry_run: bool,
        config: &std::path::Path,
    ) -> GlobalFlags {
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
    fn resolve_phone_prefers_explicit_arg_over_env() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        std::env::set_var(TELE_PHONE_ENV, "+15550001111");
        let resolved = resolve_phone(Some("+15557772222"));
        std::env::remove_var(TELE_PHONE_ENV);
        assert_eq!(resolved.as_deref(), Some("+15557772222"));
    }

    #[test]
    fn resolve_phone_falls_back_to_nonempty_env() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        std::env::set_var(TELE_PHONE_ENV, " +15550001111 ");
        let resolved = resolve_phone(None);
        std::env::remove_var(TELE_PHONE_ENV);
        assert_eq!(resolved.as_deref(), Some("+15550001111"));
    }

    #[test]
    fn resolve_phone_ignores_empty_env_and_empty_arg() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        std::env::remove_var(TELE_PHONE_ENV);
        assert!(resolve_phone(None).is_none());
        std::env::set_var(TELE_PHONE_ENV, "+15550001111");
        let from_env = resolve_phone(Some("   "));
        std::env::remove_var(TELE_PHONE_ENV);
        assert_eq!(from_env.as_deref(), Some("+15550001111"));
    }

    #[test]
    fn code_prompt_includes_phone_only_on_terminal_stderr() {
        assert_eq!(
            code_prompt(Some("+15551234567"), true),
            "Enter the code sent to +15551234567: "
        );
        assert_eq!(code_prompt(Some("+15551234567"), false), "Enter the code: ");
        assert_eq!(code_prompt(None, true), "Enter the code: ");
        assert_eq!(code_prompt(None, false), "Enter the code: ");
    }

    #[test]
    fn argv_phone_warning_fires_whenever_phone_is_passed() {
        assert!(argv_phone_warning(Some("+15551234567"), false).is_some());
        assert!(argv_phone_warning(Some("+15551234567"), true).is_some());
        assert!(argv_phone_warning(None, false).is_none());
        assert!(argv_phone_warning(None, true).is_none());
    }

    #[test]
    fn should_print_token_gates_raw_uri() {
        assert!(should_print_token(true, false));
        assert!(should_print_token(false, true));
        assert!(!should_print_token(false, false));
    }

    #[test]
    fn qr_token_lines_non_terminal_stderr_warns_and_keeps_token() {
        let (warning, line) = qr_token_lines("tg://login?token=abc123", false);
        assert_eq!(warning, Some(QR_TOKEN_NON_TTY_WARNING));
        assert_eq!(line, "URI: tg://login?token=abc123");
    }

    #[test]
    fn qr_token_lines_terminal_stderr_emits_token_without_warning() {
        let (warning, line) = qr_token_lines("tg://login?token=abc123", true);
        assert_eq!(warning, None);
        assert_eq!(line, "URI: tg://login?token=abc123");
    }

    #[test]
    fn prompt_line_writes_prompt_then_reads_line() {
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
    fn prompt_line_eof_returns_none() {
        let mut out = Vec::new();
        let line =
            prompt_line("Enter password: ", &mut std::io::Cursor::new(b""), &mut out).unwrap();
        assert!(line.is_none());
        assert_eq!(String::from_utf8(out).unwrap(), "Enter password: ");
    }

    #[test]
    fn strip_line_ending_preserves_password_spaces() {
        assert_eq!(strip_line_ending(" pass word \r\n"), " pass word ");
        assert_eq!(strip_line_ending(" pass word \n"), " pass word ");
    }

    #[test]
    fn strip_line_ending_removes_only_line_terminator() {
        assert_eq!(strip_line_ending("password"), "password");
        assert_eq!(strip_line_ending("pass\n"), "pass");
    }

    #[test]
    fn action_envelope_matches_contract_shape() {
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
    fn dry_run_envelope_marks_dry_run_and_describes_action() {
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
    fn add_dry_run_data_carries_argument_keys() {
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
    fn list_envelope_keeps_accounts_and_adds_results() {
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
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        let dir =
            std::env::temp_dir().join(format!("telecli-remove-badname-{}", std::process::id()));
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
        let dir =
            std::env::temp_dir().join(format!("telecli-remove-drybad-{}", std::process::id()));
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
    async fn remove_session_file_retry_tolerates_missing_file() {
        let dir =
            std::env::temp_dir().join(format!("telecli-logout-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        remove_session_file_retry(&dir.join("ghost.session"))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_file_retry_waits_for_handle_release() {
        let dir = std::env::temp_dir().join(format!("telecli-logout-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("locked.session");
        let file = std::fs::File::create(&path).unwrap();
        let holder = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            drop(file);
        });
        remove_session_file_retry(&path).await.unwrap();
        assert!(!path.exists());
        holder.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn session_drop_then_remove_succeeds() {
        let dir =
            std::env::temp_dir().join(format!("telecli-session-close-{}", std::process::id()));
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
    fn ensure_account_config_entry_creates_missing_account() {
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
    fn ensure_account_config_entry_noop_when_already_present() {
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
    fn terminate_decision_refuses_current_session_hash() {
        let rows = vec![fixture_auth(111, true), fixture_auth(222, false)];
        let err = terminate_decision(111, &rows).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("current session"), "{err}");
        assert!(err.message().contains("111"), "{err}");
    }

    #[test]
    fn terminate_decision_allows_other_session_hash() {
        let rows = vec![fixture_auth(111, true), fixture_auth(222, false)];
        terminate_decision(222, &rows).unwrap();
    }

    #[test]
    fn terminate_decision_rejects_unknown_hash() {
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
        let err = extract_srp_params(None).unwrap_err();
        assert!(err.message().contains("unsupported"), "{err}");
        let err = extract_srp_params(Some(&enums::PasswordKdfAlgo::Unknown)).unwrap_err();
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
        let err = map_update_password_error(e);
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
        let err = map_update_password_error(e);
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
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
    fn export_row_and_data_carry_contract_keys() {
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
    fn import_row_and_data_carry_resulting_account() {
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
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        session::remove_session("telethon").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        drop(_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_import_rejects_foreign_schema_without_side_effects() {
        let dir =
            std::env::temp_dir().join(format!("telecli-imp-tgforeign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("telethon.session");
        let scaffold = grammers_client::session::storages::SqliteSession::open(&src)
            .await
            .unwrap();
        drop(scaffold);
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        let exported = session::export_session("keyed", Some(&dest)).unwrap();
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
        remove_session_file_retry(&dest).await.unwrap();
        session::remove_session("keyed").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        drop(_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_then_export_via_commands_preserves_bytes() {
        let dir =
            std::env::temp_dir().join(format!("telecli-cmd-roundtrip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inbox")).unwrap();
        let _guard = crate::config::TEST_ENV_LOCK.lock().await;
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
        session::remove_session("restored").unwrap();
        session::remove_session("origin").unwrap();
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
            let file_mode =
                std::fs::metadata(pending_dir_under(&dir).join(login_pending_file("work")))
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
        let expected =
            hex_to_bytes("4f42157ee0e0ddaeb1e9176c9337902b56241de35f2685fe605c9f3edae39228");
        assert_eq!(out.to_vec(), expected);
        assert_eq!(ph2(pw, s1, s2).to_vec(), expected);
        assert_ne!(ph2(b"other", s1, s2).to_vec(), expected);
    }

    #[test]
    fn sh_is_salt_wrapped_sha256() {
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
        let h1 = compute_new_password_hash("hello-ph2-deterministic", &extended, &salt2, 3, &p);
        let h2 = compute_new_password_hash("hello-ph2-deterministic", &extended, &salt2, 3, &p);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 256);
        assert!(h1.iter().any(|&b| b != 0));
        let h3 = compute_new_password_hash("different", &extended, &salt2, 3, &p);
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
        let (algo, hash) = new_password_algo_and_hash("pwd", &base, Some(extra));
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
        for (set, change, remove, confirm, resend, cancel, status, reset_start, decline) in one_each
        {
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
    fn web_authorization_row_fixture_shapes_expected_keys() {
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
    fn web_table_cells_flatten_specified_columns() {
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
        match validate_phone_modes(&phone_args(None, false, Some(" 54321 "), Some(" hash ")))
            .unwrap()
        {
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
}
