use crate::client::{self, ClientGuard};
use crate::config;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output::{self, log_line};
use crate::session;
use clap::{Args, Subcommand};
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
    #[arg(long)]
    password: Option<String>,
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
    let names = session::list_session_names();
    let mut rows = Vec::new();
    for name in names {
        let tags = cfg
            .accounts
            .get(&name)
            .map(|a| a.tags.join(","))
            .unwrap_or_default();
        rows.push(serde_json::json!({            "name": name,            "tags": tags,            "session": "present",        }));
    }
    if output::machine_mode(flags.json, flags.jsonl) {
        output::print_json(&serde_json::json!({"accounts": rows}));
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
        println!("no sessions yet: tele account add + tele account login");
    } else {
        output::print_table(&["name", "tags", "session"], &table_rows);
    }
    Ok(crate::error::EXIT_OK)
}
async fn status(flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let creds = config::credentials()?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let creds = creds.clone();
        Box::pin(async move {
            let guard = ClientGuard::connect(&name, creds.api_id, config_path.as_deref()).await?;
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
    crate::executor::finish(flags, &envelope)
}
async fn add(args: &AddArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let mut cfg = config::load_config(flags.config_path.as_deref())?;
    cfg.accounts
        .entry(args.name.clone())
        .or_insert_with(config::AccountConfig::default)
        .tags = args.tags.clone().unwrap_or_default();
    let path = flags
        .config_path
        .clone()
        .unwrap_or_else(|| config::app_data_dir().join("config.toml"));
    config::write_config(&path, &cfg)?;
    log_line(
        "info",
        &format!("account {} registered in {}", args.name, path.display()),
    );
    if output::machine_mode(flags.json, flags.jsonl) {
        println!("{}", serde_json::json!({"ok": true, "name": args.name}));
    }
    Ok(crate::error::EXIT_OK)
}
fn validate_login(args: &LoginArgs) -> TeleResult<()> {
    match args.method.as_str() {
        "qr" => {}
        "code" => {
            if args.phone.is_none() {
                return Err(TeleError::Usage(
                    "--phone required for code login".to_string(),
                ));
            }
        }
        other => {
            return Err(TeleError::Usage(format!(
                "unknown login method {other} (use code or qr)"
            )));
        }
    }
    Ok(())
}

async fn login(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if flags.dry_run {
        log_line("info", "[dry-run] would log in account");
        return Ok(crate::error::EXIT_OK);
    }
    validate_login(args)?;
    let creds = config::credentials()?;
    let mut guard =
        ClientGuard::connect(&args.name, creds.api_id, flags.config_path.as_deref()).await?;
    if guard
        .client
        .is_authorized()
        .await
        .map_err(tele_invocation)?
    {
        log_line("info", "account already authorized");
        return Ok(crate::error::EXIT_OK);
    }
    match args.method.as_str() {
        "qr" => {
            client::qr_login(&guard.client, &mut guard.updates, &creds, |uri| {
                render_qr(uri);
            })
            .await?;
        }
        "code" => {
            let phone = args
                .phone
                .clone()
                .ok_or_else(|| TeleError::Usage("--phone required for code login".to_string()))?;
            let token = guard
                .client
                .request_login_code(&phone, &creds.api_hash)
                .await
                .map_err(tele_invocation)?;
            print!("Enter the code sent to {phone}: ");
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut code = String::new();
            std::io::stdin().read_line(&mut code)?;
            let code = code.trim().to_string();
            match guard.client.sign_in(&token, &code).await {
                Ok(_user) => {
                    log_line("info", &format!("account {} logged in", args.name));
                }
                Err(grammers_client::SignInError::PasswordRequired(pw_token)) => {
                    let password = args.password.clone().ok_or_else(|| {
                        TeleError::Auth("2FA password required (use --password)".to_string())
                    })?;
                    match guard.client.check_password(pw_token, &password).await {
                        Ok(_) => log_line("info", "2FA passed"),
                        Err(grammers_client::SignInError::InvalidPassword(_)) => {
                            return Err(TeleError::Auth("invalid 2FA password".to_string()));
                        }
                        Err(grammers_client::SignInError::Other(e)) => {
                            return Err(tele_invocation(e));
                        }
                        Err(_) => {
                            return Err(TeleError::Auth("2FA check failed".to_string()));
                        }
                    }
                }
                Err(grammers_client::SignInError::InvalidCode) => {
                    return Err(TeleError::Usage("invalid code".to_string()));
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
        other => {
            return Err(TeleError::Usage(format!(
                "unknown login method {other} (use code or qr)"
            )));
        }
    }
    if output::machine_mode(flags.json, flags.jsonl) {
        println!("{}", serde_json::json!({"ok": true, "account": args.name}));
    }
    Ok(crate::error::EXIT_OK)
}
async fn logout(args: &LogoutArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if flags.dry_run {
        log_line("info", "[dry-run] would log out account");
        return Ok(crate::error::EXIT_OK);
    }
    let creds = config::credentials()?;
    let guard =
        ClientGuard::connect(&args.name, creds.api_id, flags.config_path.as_deref()).await?;
    if let Err(e) = guard.client.sign_out().await {
        if crate::error::invocation_is_unauthorized(&e) {
            log_line("info", "account was not authorized; removing session");
        } else {
            return Err(tele_invocation(e));
        }
    }
    drop(guard);
    session::remove_session(&args.name)?;
    log_line("info", &format!("account {} logged out", args.name));
    if output::machine_mode(flags.json, flags.jsonl) {
        println!("{}", serde_json::json!({"ok": true, "account": args.name}));
    }
    Ok(crate::error::EXIT_OK)
}
async fn remove(args: &RemoveArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if flags.dry_run {
        log_line("info", "[dry-run] would remove account");
        return Ok(crate::error::EXIT_OK);
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
    if output::machine_mode(flags.json, flags.jsonl) {
        println!("{}", serde_json::json!({"ok": true, "account": args.name}));
    }
    Ok(crate::error::EXIT_OK)
}
pub fn tele_invocation(e: grammers_client::InvocationError) -> TeleError {
    if crate::error::invocation_is_unauthorized(&e) {
        TeleError::Auth("not logged in (session invalid)".to_string())
    } else {
        TeleError::Invocation(
            crate::error::invocation_message(&e),
            crate::error::invocation_wait_seconds(&e),
        )
    }
}
fn render_qr(uri: &str) {
    eprintln!("Scan this QR code with Telegram (Settings > Devices > Link Desktop Device):");
    match qrcode::QrCode::new(uri.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<char>()
                .quiet_zone(true)
                .module_dimensions(2, 1)
                .build();
            eprintln!("{rendered}");
        }
        Err(_) => {
            eprintln!("URI: {uri}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login_args(method: &str, phone: Option<&str>) -> LoginArgs {
        LoginArgs {
            name: "x".to_string(),
            method: method.to_string(),
            phone: phone.map(str::to_string),
            password: None,
        }
    }

    #[test]
    fn login_code_requires_phone() {
        assert!(matches!(
            validate_login(&login_args("code", None)),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_login(&login_args("code", Some("+1"))).is_ok());
    }

    #[test]
    fn login_qr_needs_no_phone() {
        assert!(validate_login(&login_args("qr", None)).is_ok());
    }

    #[test]
    fn login_unknown_method_rejected() {
        assert!(matches!(
            validate_login(&login_args("sms", Some("+1"))),
            Err(TeleError::Usage(_))
        ));
    }
}
