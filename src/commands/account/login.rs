use clap::Args;
use std::io::{IsTerminal, Write};

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds;
use crate::config;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::GlobalFlags;
use crate::output::{self, log_line};
use crate::session;

use super::*;

#[derive(Args)]
pub struct LoginArgs {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, default_value = "code")]
    pub(crate) method: String,
    #[arg(long)]
    pub(crate) phone: Option<String>,
    #[arg(long, help = "print the QR login URI to stderr if QR rendering fails")]
    pub(crate) show_token: bool,
    #[arg(
        long,
        default_value_t = DEFAULT_QR_TIMEOUT_SECS,
        help = "overall QR login deadline in seconds"
    )]
    pub(crate) qr_timeout_secs: u64,
    #[arg(long, help = STAGE_HELP)]
    pub(crate) stage: Option<String>,
}

pub(crate) const STAGE_HELP: &str = "run code login stepwise across invocations: begin | code | status | cancel | resend | cancel-code (cancel discards the local pending state only; cancel-code also asks the server to invalidate the sent code)";

pub(crate) const DEFAULT_QR_TIMEOUT_SECS: u64 = 300;
pub(crate) const TELE_PHONE_ENV: &str = "TELE_PHONE";
pub(crate) const MAX_CODE_ATTEMPTS: usize = 3;
pub(crate) const MAX_PASSWORD_ATTEMPTS: usize = 3;

pub(crate) fn validate_login(
    method: &str,
    phone: Option<&str>,
    qr_timeout_secs: u64,
) -> TeleResult<()> {
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

pub(crate) fn resolve_phone(explicit: Option<&str>) -> Option<String> {
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

pub(crate) async fn login(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let phone = resolve_phone(args.phone.as_deref());
    session::validate_name(&args.name).map_err(TeleError::Usage)?;
    let stage = match args.stage.as_deref() {
        Some(raw) => Some(staged_login::validate_staged(
            &args.method,
            raw,
            phone.as_deref(),
        )?),
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
        return staged_login::staged_login(args, flags, stage, phone).await;
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
        .map(|exists| !exists)
        .unwrap_or(true);
    let mut guard =
        match ClientGuard::connect(&args.name, credentials.api_id, flags.config_path.as_deref())
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                if !session_existed_before {
                    cleanup_partial_session(&args.name).await;
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
        purge_pending(&args.name);
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
        cleanup_partial_session(&args.name).await;
    }
    result
}

pub(crate) async fn cleanup_partial_session(name: &str) {
    match session::remove_session(name).await {
        Ok(()) => log_line("warn", "login failed; removed partial session files"),
        Err(e) => log_line(
            "warn",
            &format!("login failed; could not remove partial session files: {e:#}"),
        ),
    }
}

pub(crate) fn pending_dir_under(base: &std::path::Path) -> std::path::PathBuf {
    base.join("pending")
}

pub(crate) fn login_pending_file(name: &str) -> String {
    format!("{name}.login.json")
}

pub(crate) fn phone_pending_file(name: &str) -> String {
    format!("{name}.phone.json")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingDocument {
    pub(crate) version: u32,
    pub(crate) account: String,
    pub(crate) phone: String,
    pub(crate) phone_code_hash: String,
    pub(crate) created_at: String,
}

pub(crate) const PENDING_DOCUMENT_VERSION: u32 = 1;

impl PendingDocument {
    pub(crate) fn new(account: &str, phone: &str, phone_code_hash: String) -> Self {
        Self {
            version: PENDING_DOCUMENT_VERSION,
            account: account.to_string(),
            phone: phone.to_string(),
            phone_code_hash,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub(crate) type PendingLogin = PendingDocument;
pub(crate) type PendingPhone = PendingDocument;

pub(crate) fn save_pending_document_under(
    base: &std::path::Path,
    file: &str,
    text: &str,
) -> TeleResult<()> {
    let dir = pending_dir_under(base);
    crate::fs_util::create_dir_private(&dir)
        .map_err(|e| TeleError::Other(format!("failed to create pending dir {}: {e}", file)))?;
    let path = pending_dir_under(base).join(file);
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = std::path::PathBuf::from(tmp_name);
    let result = std::fs::write(&tmp_path, "")
        .and_then(|()| crate::fs_util::restrict_file_private(&tmp_path))
        .and_then(|()| std::fs::write(&tmp_path, text))
        .and_then(|()| std::fs::rename(&tmp_path, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result.map_err(|e| TeleError::Other(format!("failed to write {file}: {e}")))
}

pub(crate) fn load_pending_document_under(
    base: &std::path::Path,
    file: &str,
) -> TeleResult<Option<String>> {
    match std::fs::read_to_string(pending_dir_under(base).join(file)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(TeleError::Other(format!(
            "failed to read pending state {}: {e}",
            pending_dir_under(base).join(file).display()
        ))),
    }
}

pub(crate) fn remove_pending_document_under(
    base: &std::path::Path,
    file: &str,
) -> TeleResult<bool> {
    match std::fs::remove_file(pending_dir_under(base).join(file)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(TeleError::Other(format!(
            "failed to remove pending state {file}: {e}"
        ))),
    }
}

pub(crate) fn save_pending_generic<T: serde::Serialize>(
    base: &std::path::Path,
    file: &str,
    value: &T,
) -> TeleResult<()> {
    let text = serde_json::to_string_pretty(value)?;
    save_pending_document_under(base, file, &text)
}

pub(crate) fn load_pending_generic<T: serde::de::DeserializeOwned>(
    base: &std::path::Path,
    file: &str,
    corrupt: impl FnOnce(String) -> TeleError,
) -> TeleResult<Option<T>> {
    match load_pending_document_under(base, file)? {
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| corrupt(e.to_string())),
        None => Ok(None),
    }
}

pub(crate) fn remove_pending_generic(base: &std::path::Path, file: &str) -> TeleResult<bool> {
    remove_pending_document_under(base, file)
}

pub(crate) fn purge_pending(name: &str) {
    let _ = staged_login::remove_pending(name);
    let _ = phone::remove_pending_phone(name);
}

pub(crate) async fn login_flow(
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
                &guard.rate_limiter,
                credentials,
                args.qr_timeout_secs,
                |uri| {
                    render_qr(uri, show_token, flags.quiet);
                },
            )
            .await?;
            let _ = bootstrap_peer_cache(guard).await;
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

pub(crate) async fn sign_in_with_retries(
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
                return Err(TeleError::Auth("invalid 2FA password".to_string()));
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

pub(crate) async fn password_flow(
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
        let echo_disabled = password::disable_stdin_echo();
        if !echo_disabled && attempt == 1 {
            log_line(
                "warn",
                "secure password input unavailable; input will be echoed to the terminal",
            );
        }
        let read = prompt_line("Enter the 2FA password: ", stdin, stderr);
        password::restore_stdin_echo(echo_disabled);
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

pub(crate) async fn refresh_password_token(
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

pub(crate) async fn bootstrap_peer_cache(guard: &ClientGuard) -> TeleResult<()> {
    use grammers_client::{
        session::{
            types::{PeerAuth, PeerInfo, UpdateState, UpdatesState},
            Session as _,
        },
        tl,
    };
    let me = guard.client.get_me().await.map_err(tele_invocation)?;
    let me = match me.raw {
        tl::enums::User::User(u) => u,
        tl::enums::User::Empty(_) => {
            return Err(TeleError::Other(
                "get_me returned an empty user after sign in".to_string(),
            ));
        }
    };
    guard
        .session
        .cache_peer(&PeerInfo::User {
            id: me.id,
            auth: me.access_hash.filter(|_| !me.min).map(PeerAuth::from_hash),
            bot: Some(me.bot),
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

pub(crate) fn code_prompt(phone: Option<&str>, stderr_is_terminal: bool) -> String {
    match phone {
        Some(phone) if stderr_is_terminal => {
            format!("Enter the code sent to {}: ", redact_phone(phone))
        }
        _ => "Enter the code: ".to_string(),
    }
}

pub(crate) fn argv_phone_warning(
    phone: Option<&str>,
    _stderr_is_terminal: bool,
) -> Option<&'static str> {
    phone.map(|_| {
        "--phone is visible in process listings and shell history; prefer TELE_PHONE or an interactive prompt"
    })
}

pub(crate) fn prompt_line(
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

pub(crate) fn strip_line_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

pub(crate) fn ensure_account_config_entry(
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

pub(crate) const QR_TOKEN_NON_TTY_WARNING: &str =
    "printing one-time login token to a non-terminal stderr; treat the output as a secret";

pub(crate) fn qr_token_lines(
    uri: &str,
    stderr_is_terminal: bool,
) -> (Option<&'static str>, String) {
    let warning = if stderr_is_terminal {
        None
    } else {
        Some(QR_TOKEN_NON_TTY_WARNING)
    };
    (warning, format!("URI: {uri}"))
}

pub(crate) fn render_qr(uri: &str, show_token: bool, quiet: bool) {
    let stderr_is_terminal = std::io::stderr().is_terminal();
    if quiet {
        if should_print_token(show_token, stderr_is_terminal) {
            let (warning, line) = qr_token_lines(uri, stderr_is_terminal);
            if let Some(warning) = warning {
                output::log_line("warn", warning);
            }
            output::log_line("info", &line);
        } else {
            output::log_line(
                "warn",
                "QR rendering suppressed in quiet mode; re-run with --show-token to log the login URI",
            );
        }
        return;
    }
    if !stderr_is_terminal {
        output::log_line(
            "warn",
            "QR suppressed: stderr is not a terminal and the QR encodes the login token; re-run with --show-token or on a TTY to render it",
        );
        if should_print_token(show_token, false) {
            let (warning, line) = qr_token_lines(uri, false);
            if let Some(warning) = warning {
                output::log_line("warn", warning);
            }
            let _ = writeln!(std::io::stderr(), "{line}");
        }
        return;
    }
    output::log_line(
        "info",
        "Scan this QR code with Telegram (Settings > Devices > Link Desktop Device):",
    );
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
            if should_print_token(show_token, stderr_is_terminal) {
                let (warning, line) = qr_token_lines(uri, stderr_is_terminal);
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

pub(crate) fn should_print_token(show_token: bool, stderr_is_terminal: bool) -> bool {
    show_token || stderr_is_terminal
}
