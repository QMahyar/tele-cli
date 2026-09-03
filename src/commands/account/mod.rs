use crate::client::ClientGuard;
use crate::commands::credentials::creds;
use crate::config;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, select_sessions, GlobalFlags};
use crate::output::{self, log_line, AccountOutcome, Envelope};
use crate::session;
use clap::{Args, Subcommand};

mod login;
mod password;
mod phone;
mod staged_login;

pub use login::LoginArgs;
pub use phone::PhoneArgs;

pub(crate) fn refuse_interactive_with_multiple_accounts(
    command: &str,
    flags: &GlobalFlags,
) -> TeleResult<()> {
    let count = flags.account.len().max(flags.tag.len());
    if count > 1 {
        return Err(TeleError::Usage(format!(
            "{command} with interactive prompts requires a single --account; \
             use --account <name> to select exactly one account"
        )));
    }
    Ok(())
}

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

    fn needs_prompt(&self) -> bool {
        matches!(self, PasswordAction::Mode(_))
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
    #[arg(
        long,
        help = "destination path; defaults to <app data>/sessions/<name>.session.export"
    )]
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
        AccountCmd::Phone(args) => phone::phone(&args, flags).await,
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
    if flags.dry_run {
        let mut names = crate::executor::select_accounts(flags)?;
        names.sort();
        let outcomes = names
            .iter()
            .map(|name| crate::output::AccountOutcome {
                account: name.clone(),
                ok: true,
                error: None,
                data: Some(serde_json::json!({
                    "dry_run": true,
                    "would": "probe authorization status",
                })),
                exit_code: Some(crate::error::EXIT_OK),
            })
            .collect();
        let envelope = crate::output::Envelope::new(outcomes, true, &flags.command);
        output::log_line("info", "[dry-run] would probe authorization status");
        return crate::executor::finish(flags, &envelope);
    }
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

pub(crate) fn status_device_data(identity: &config::DeviceIdentity) -> serde_json::Value {
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

pub(crate) fn status_device_summary(data: Option<&serde_json::Value>) -> String {
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
pub(crate) use login::{
    bootstrap_peer_cache, cleanup_partial_session, code_prompt, login, password_flow, prompt_line,
    purge_pending, refresh_password_token, strip_line_ending,
};
pub(crate) use login::{
    ensure_account_config_entry, load_pending_generic, login_pending_file, phone_pending_file,
    remove_pending_generic, save_pending_generic, PendingLogin, PendingPhone,
};

pub(crate) use login::MAX_CODE_ATTEMPTS;

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
    purge_pending(&args.name);
    drop(guard);
    if let Err(e) = session::remove_session(&args.name).await {
        log_line("warn", &format!("could not remove session files: {e:#}"));
    }
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
    purge_pending(&args.name);
    session::remove_session(&args.name).await?;
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

pub(crate) fn export_row(exported: &session::ExportedSession) -> Vec<String> {
    vec![
        exported.account.clone(),
        exported.path.display().to_string(),
        exported.bytes.to_string(),
        exported.sha256.clone(),
    ]
}

pub(crate) fn export_data(exported: &session::ExportedSession) -> serde_json::Value {
    serde_json::json!({
        "account": exported.account,
        "path": exported.path.display().to_string(),
        "bytes": exported.bytes,
        "sha256": exported.sha256,
    })
}

pub(crate) fn import_data(imported: &session::ImportedSession) -> serde_json::Value {
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
        let dest = args
            .out
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| format!("<app data>/sessions/{}.session.export", args.name));
        let would = format!("export session {dest} from account {}", args.name);
        return crate::executor::finish(
            flags,
            &dry_run_envelope(&args.name, &would, &flags.command),
        );
    }
    let exported =
        session::export_session(&args.name, args.out.as_deref().map(std::path::Path::new)).await?;
    let mut data = export_data(&exported);
    if output::machine_mode(flags.json, flags.jsonl) {
        data["warning"] = serde_json::Value::String(session::SESSION_FILE_WARNING.to_string());
    } else {
        log_line("warn", session::SESSION_FILE_WARNING);
    }
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

pub(crate) fn import_row(imported: &session::ImportedSession) -> Vec<String> {
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
    let mut data = import_data(imported);
    if output::machine_mode(flags.json, flags.jsonl) {
        data["warning"] = serde_json::Value::String(session::SESSION_FILE_WARNING.to_string());
    } else {
        log_line("warn", session::SESSION_FILE_WARNING);
    }
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
    let password = password::fetch_password(guard).await?;
    if !password.has_password {
        return Ok(grammers_client::tl::enums::InputCheckPasswordSrp::InputCheckPasswordEmpty);
    }
    let params = password::extract_srp_params(password.current_algo.as_ref())?;
    let srp_b = password
        .srp_b
        .clone()
        .ok_or_else(|| TeleError::Other(password::NO_SRP_CHALLENGE_MSG.to_string()))?;
    let srp_id = password
        .srp_id
        .ok_or_else(|| TeleError::Other(password::NO_SRP_CHALLENGE_MSG.to_string()))?;
    password::prompt_current_password_proof(
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
    refuse_interactive_with_multiple_accounts("account delete", flags)?;
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
                Err(e) => Err(password::map_update_password_error(e)),
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

pub(crate) async fn fetch_authorizations(
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
    if action.needs_prompt() {
        refuse_interactive_with_multiple_accounts("account password", flags)?;
    }
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
                PasswordMode::Remove => password::remove_cloud_password(guard).await?,
                PasswordMode::Set => password::set_cloud_password(guard, hint, email).await?,
                PasswordMode::Change => password::change_cloud_password(guard, hint, email).await?,
            }
            Ok(serde_json::json!({"updated": true, "mode": mode.as_str()}))
        }
        PasswordAction::ConfirmEmail(code) => {
            password::confirm_password_email(guard, code).await?;
            Ok(serde_json::json!({"confirmed": true}))
        }
        PasswordAction::ResendEmail => {
            password::resend_password_email(guard).await?;
            Ok(serde_json::json!({"resent": true}))
        }
        PasswordAction::CancelEmail => {
            password::cancel_password_email(guard).await?;
            Ok(serde_json::json!({"cancelled": true}))
        }
        PasswordAction::Status => password::password_status(guard).await,
        PasswordAction::ResetStart => password::start_password_reset(guard).await,
        PasswordAction::DeclineReset => {
            password::decline_password_reset(guard).await?;
            Ok(serde_json::json!({"declined": true}))
        }
    }
}

pub(crate) fn single_outcome(account: &str, data: serde_json::Value) -> AccountOutcome {
    AccountOutcome {
        account: account.to_string(),
        ok: true,
        error: None,
        data: Some(data),
        exit_code: None,
    }
}

pub(crate) fn action_envelope(
    account: &str,
    data: serde_json::Value,
    dry_run: bool,
    command: &str,
) -> Envelope {
    Envelope::new(vec![single_outcome(account, data)], dry_run, command)
}

pub(crate) fn add_dry_run_data(
    name: &str,
    tags: &Option<Vec<String>>,
    would: &str,
) -> serde_json::Value {
    serde_json::json!({
        "would": would,
        "dry_run": true,
        "name": name,
        "tags": tags,
    })
}

pub(crate) fn dry_run_envelope(account: &str, would: &str, command: &str) -> Envelope {
    action_envelope(
        account,
        serde_json::json!({"would": would, "dry_run": true}),
        true,
        command,
    )
}

pub(crate) fn list_envelope(
    rows: &[serde_json::Value],
    flags: &GlobalFlags,
) -> TeleResult<serde_json::Value> {
    let outcomes: Vec<AccountOutcome> = rows
        .iter()
        .map(|r| single_outcome(r["name"].as_str().unwrap_or_default(), r.clone()))
        .collect();
    let mut value = serde_json::to_value(Envelope::new(outcomes, flags.dry_run, &flags.command))?;
    value["accounts"] = serde_json::Value::Array(rows.to_vec());
    Ok(value)
}

pub(crate) fn status_table_rows(envelope: &Envelope) -> Vec<Vec<String>> {
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

pub(crate) fn authorization_row(
    auth: &grammers_client::tl::types::Authorization,
) -> serde_json::Value {
    let date = |ts: i32| {
        chrono::DateTime::from_timestamp(i64::from(ts), 0)
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

pub(crate) fn sessions_table_cells(account: &str, row: &serde_json::Value) -> Vec<String> {
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

pub(crate) fn terminate_decision(
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

pub(crate) async fn fetch_web_authorizations(
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

pub(crate) fn parse_bool_arg(value: &str, flag: &str) -> TeleResult<bool> {
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

pub(crate) fn web_authorization_row(
    web: &grammers_client::tl::types::WebAuthorization,
) -> serde_json::Value {
    let date = |ts: i32| {
        chrono::DateTime::from_timestamp(i64::from(ts), 0)
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

pub(crate) fn web_table_cells(account: &str, row: &serde_json::Value) -> Vec<String> {
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

pub(crate) fn web_hash_decision(
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct AccountStatusParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct TtlGetParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct TtlSetParams {
    pub(crate) days: i64,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SessionsListParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SessionsWebParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug)]
struct AccountStatusArgs;

#[derive(Clone, Debug)]
struct TtlGetArgs;

#[derive(Clone, Debug)]
struct TtlSetArgs {
    days: i64,
}

#[derive(Clone, Debug)]
struct SessionsListArgs;

#[derive(Clone, Debug)]
struct SessionsWebArgs;

impl From<&AccountStatusParams> for AccountStatusArgs {
    fn from(_: &AccountStatusParams) -> Self {
        Self
    }
}

impl From<&TtlGetParams> for TtlGetArgs {
    fn from(_: &TtlGetParams) -> Self {
        Self
    }
}

impl From<&TtlSetParams> for TtlSetArgs {
    fn from(p: &TtlSetParams) -> Self {
        Self { days: p.days }
    }
}

impl From<&SessionsListParams> for SessionsListArgs {
    fn from(_: &SessionsListParams) -> Self {
        Self
    }
}

impl From<&SessionsWebParams> for SessionsWebArgs {
    fn from(_: &SessionsWebParams) -> Self {
        Self
    }
}

fn validate_serve_ttl_set(args: &TtlSetArgs) -> TeleResult<()> {
    let parsed = i32::try_from(args.days)
        .map_err(|_| TeleError::Usage("--days must be between 1 and 365".to_string()))?;
    if !(1..=365).contains(&parsed) {
        return Err(TeleError::Usage(format!(
            "--days must be between 1 and 365, got {parsed}"
        )));
    }
    Ok(())
}

pub(crate) async fn account_status_core(
    shares: &crate::client::ServeShares,
    _params: AccountStatusParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let mut authorized = shares.client.is_authorized().await.map_err(|e| {
        if crate::error::invocation_is_unauthorized(&e) {
            TeleError::Auth("not logged in".to_string())
        } else {
            TeleError::Invocation(
                crate::error::invocation_message(&e),
                crate::error::invocation_wait_seconds(&e),
            )
        }
    })?;
    if authorized {
        shares.rate_limiter.acquire().await;
        match shares
            .client
            .invoke(&grammers_client::tl::functions::account::GetPassword {})
            .await
        {
            Ok(_) => {}
            Err(e) => {
                if crate::error::invocation_is_unauthorized(&e) {
                    authorized = false;
                } else {
                    return Err(tele_invocation(e));
                }
            }
        }
    }
    let device = status_device_data(&config::DeviceIdentity::default());
    Ok(serde_json::json!({ "authorized": authorized, "device": device }))
}

pub(crate) async fn account_ttl_get_core(
    shares: &crate::client::ServeShares,
    _params: TtlGetParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let response = shares
        .client
        .invoke(&grammers_client::tl::functions::account::GetAccountTtl {})
        .await
        .map_err(tele_invocation)?;
    let grammers_client::tl::enums::AccountDaysTtl::Ttl(ttl) = response;
    Ok(ttl_data(ttl.days))
}

pub(crate) async fn account_ttl_set_core(
    shares: &crate::client::ServeShares,
    params: TtlSetParams,
) -> TeleResult<serde_json::Value> {
    let days = i32::try_from(params.days)
        .map_err(|_| TeleError::Usage("--days must be between 1 and 365".to_string()))?;
    shares.rate_limiter.acquire().await;
    let request = grammers_client::tl::functions::account::SetAccountTtl {
        ttl: grammers_client::tl::enums::AccountDaysTtl::Ttl(
            grammers_client::tl::types::AccountDaysTtl { days },
        ),
    };
    let result = shares.client.invoke(&request).await;
    match result {
        Ok(true) => Ok(ttl_set_data(days)),
        Ok(false) => Err(TeleError::Other(
            "server refused to update the account TTL".to_string(),
        )),
        Err(e) => Err(tele_invocation(e)),
    }
}

pub(crate) async fn account_sessions_list_core(
    shares: &crate::client::ServeShares,
    _params: SessionsListParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let authorizations = fetch_authorizations(&shares.client).await?;
    let rows: Vec<serde_json::Value> = authorizations.iter().map(authorization_row).collect();
    Ok(serde_json::json!({ "count": rows.len(), "authorizations": rows }))
}

pub(crate) async fn account_sessions_web_core(
    shares: &crate::client::ServeShares,
    _params: SessionsWebParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let webs = fetch_web_authorizations(&shares.client).await?;
    let rows: Vec<serde_json::Value> = webs.iter().map(web_authorization_row).collect();
    Ok(serde_json::json!({
        "count": rows.len(),
        "web": true,
        "authorizations": rows
    }))
}

crate::serve_runner!(run_account_status, account_status_core, AccountStatusParams);
crate::serve_runner!(run_ttl_get, account_ttl_get_core, TtlGetParams);
crate::serve_runner!(run_ttl_set, account_ttl_set_core, TtlSetParams);
crate::serve_runner!(
    run_sessions_list,
    account_sessions_list_core,
    SessionsListParams
);
crate::serve_runner!(
    run_sessions_web,
    account_sessions_web_core,
    SessionsWebParams
);

pub(crate) fn account_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "account status",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "probe authorization and account-level API health",
            AccountStatusParams,
            AccountStatusArgs,
            |_: &AccountStatusArgs| Ok::<_, TeleError>(()),
            |_: &AccountStatusArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "would": "probe authorization status",
            })),
            run_account_status,
            crate::commands::serve::params_schema::<AccountStatusParams>
        ),
        crate::serve_route!(
            "account sessions list",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list active device sessions",
            SessionsListParams,
            SessionsListArgs,
            |_: &SessionsListArgs| Ok::<_, TeleError>(()),
            |_: &SessionsListArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "would": "list device sessions",
            })),
            run_sessions_list,
            crate::commands::serve::params_schema::<SessionsListParams>
        ),
        crate::serve_route!(
            "account sessions web",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list active web login sessions",
            SessionsWebParams,
            SessionsWebArgs,
            |_: &SessionsWebArgs| Ok::<_, TeleError>(()),
            |_: &SessionsWebArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "would": "list web login sessions",
            })),
            run_sessions_web,
            crate::commands::serve::params_schema::<SessionsWebParams>
        ),
        crate::serve_route!(
            "account ttl get",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "show the inactive-account self-destruct TTL",
            TtlGetParams,
            TtlGetArgs,
            |_: &TtlGetArgs| Ok::<_, TeleError>(()),
            |_: &TtlGetArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "would": "show the inactive-account TTL",
            })),
            run_ttl_get,
            crate::commands::serve::params_schema::<TtlGetParams>
        ),
        crate::serve_route!(
            "account ttl set",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "set the inactive-account self-destruct TTL",
            TtlSetParams,
            TtlSetArgs,
            validate_serve_ttl_set,
            |a: &TtlSetArgs| Ok::<_, TeleError>(serde_json::json!({
                "dry_run": true,
                "days": a.days,
                "would": format!("set the inactive-account TTL to {} days", a.days),
            })),
            run_ttl_set,
            crate::commands::serve::params_schema::<TtlSetParams>
        ),
    ]
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "tests.rs"]
mod tests;
