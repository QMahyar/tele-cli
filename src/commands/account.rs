use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds;
use crate::config;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, select_sessions, GlobalFlags};
use crate::output::{self, log_line, AccountOutcome, Envelope};
use crate::session;
use clap::{Args, Subcommand};
use std::io::{IsTerminal, Write};

fn redact_phone(phone: &str) -> String {
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
}

const DEFAULT_QR_TIMEOUT_SECS: u64 = 300;
const TELE_PHONE_ENV: &str = "TELE_PHONE";
const MAX_CODE_ATTEMPTS: usize = 3;
const MAX_PASSWORD_ATTEMPTS: usize = 3;
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
pub async fn run(cmd: AccountCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        AccountCmd::List => list(flags).await,
        AccountCmd::Status => status(flags).await,
        AccountCmd::Add(args) => add(&args, flags).await,
        AccountCmd::Login(args) => login(&args, flags).await,
        AccountCmd::Logout(args) => logout(&args, flags).await,
        AccountCmd::Remove(args) => remove(&args, flags).await,
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
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let credentials = credentials.clone();
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
            Ok(serde_json::json!({ "authorized": authorized }))
        })
    })
    .await?;
    if !output::machine_mode(flags.json, flags.jsonl) {
        let rows = status_table_rows(&envelope);
        if !rows.is_empty() {
            output::print_table(&["account", "authorized"], &rows)?;
        }
    }
    crate::executor::finish(flags, &envelope)
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
const SESSION_REMOVE_RETRIES: usize = 20;
const SESSION_REMOVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

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
        }
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
                vec!["home".to_string(), "yes".to_string()],
                vec!["work".to_string(), "no".to_string()],
            ]
        );
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
}
