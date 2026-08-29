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

use super::*;

#[allow(unused_imports)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginStage {
    Begin,
    Code,
    Status,
    Cancel,
    Resend,
    CancelCode,
}

impl LoginStage {
    pub(crate) fn as_str(self) -> &'static str {
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

pub(crate) fn parse_login_stage(raw: &str) -> TeleResult<LoginStage> {
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

pub(crate) fn validate_staged(
    method: &str,
    raw_stage: &str,
    phone: Option<&str>,
) -> TeleResult<LoginStage> {
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

pub(crate) use crate::commands::account::PendingLogin;
#[allow(dead_code)]
pub(crate) const PENDING_LOGIN_VERSION: u32 = crate::commands::account::PENDING_DOCUMENT_VERSION;

pub(crate) fn save_pending(pending: &PendingLogin) -> TeleResult<()> {
    save_pending_under(&config::app_data_dir(), pending)
}

pub(crate) fn save_pending_under(base: &std::path::Path, pending: &PendingLogin) -> TeleResult<()> {
    save_pending_generic(base, &login_pending_file(&pending.account), pending)
}

pub(crate) fn load_pending(name: &str) -> TeleResult<Option<PendingLogin>> {
    load_pending_under(&config::app_data_dir(), name)
}

pub(crate) fn load_pending_under(
    base: &std::path::Path,
    name: &str,
) -> TeleResult<Option<PendingLogin>> {
    load_pending_generic(base, &login_pending_file(name), |e| {
        TeleError::Other(format!(
            "pending login state for {name} is corrupt ({e}); run tele account login --stage begin again"
        ))
    })
}

pub(crate) fn require_pending(name: &str) -> TeleResult<PendingLogin> {
    require_pending_under(&config::app_data_dir(), name)
}

pub(crate) fn require_pending_under(
    base: &std::path::Path,
    name: &str,
) -> TeleResult<PendingLogin> {
    load_pending_under(base, name)?.ok_or_else(|| {
        TeleError::Usage(format!(
            "no pending login for account {name}; run tele account login --name {name} --stage begin first"
        ))
    })
}

pub(crate) fn remove_pending(name: &str) -> TeleResult<bool> {
    remove_pending_under(&config::app_data_dir(), name)
}

pub(crate) fn remove_pending_under(base: &std::path::Path, name: &str) -> TeleResult<bool> {
    remove_pending_generic(base, &login_pending_file(name))
}

pub(crate) fn stage_status_data(pending: Option<&PendingLogin>) -> serde_json::Value {
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

pub(crate) fn stage_cancel_data(cancelled: bool) -> serde_json::Value {
    serde_json::json!({"stage": "cancel", "cancelled": cancelled})
}

pub(crate) fn stage_status_line(pending: Option<&PendingLogin>, name: &str) -> String {
    match pending {
        Some(p) => format!(
            "pending login for account {name}: run tele account login --name {name} --stage code (requested {})",
            p.created_at
        ),
        None => format!("no pending login for account {name}"),
    }
}

pub(crate) async fn staged_login(
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

pub(crate) async fn staged_begin(
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
        .map(|exists| !exists)
        .unwrap_or(true);
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

pub(crate) async fn staged_begin_flow(
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

pub(crate) async fn send_login_code(
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
                .map(|v| {
                    i32::try_from(v).map_err(|_| {
                        TeleError::Other(format!(
                            "DC migration target value {v} does not fit in i32"
                        ))
                    })
                })
                .transpose()?
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

pub(crate) async fn staged_code(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .map(|exists| !exists)
        .unwrap_or(true);
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
    let mut signed_in = false;
    let result = staged_code_flow(&guard, &pending, flags, &mut signed_in).await;
    drop(guard);
    if result.is_err() && !session_existed_before && !signed_in {
        cleanup_partial_session(&args.name);
    }
    result
}

pub(crate) async fn staged_code_flow(
    guard: &ClientGuard,
    pending: &PendingLogin,
    flags: &GlobalFlags,
    signed_in: &mut bool,
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
                *signed_in = true;
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

pub(crate) fn code_envelope(
    flags: &GlobalFlags,
    pending: &PendingLogin,
    authorized: bool,
) -> TeleResult<i32> {
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

pub(crate) async fn staged_resend(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .map(|exists| !exists)
        .unwrap_or(true);
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

pub(crate) async fn staged_resend_flow(
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
            let _ = bootstrap_peer_cache(guard).await;
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

pub(crate) async fn staged_cancel_code(args: &LoginArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let pending = require_pending(&args.name)?;
    let credentials = creds()?;
    ensure_account_config_entry(&args.name, flags.config_path.as_deref())?;
    let session_existed_before = session::session_path(&args.name)
        .try_exists()
        .map(|exists| !exists)
        .unwrap_or(true);
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

pub(crate) async fn staged_cancel_code_flow(
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

pub(crate) enum StagedSignIn {
    SignedIn(Box<grammers_client::tl::types::auth::Authorization>),
    PasswordNeeded,
    InvalidCode,
    CodeExpired,
    SignUpRequired,
    Failed(TeleError),
}

pub(crate) async fn raw_sign_in(
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

pub(crate) async fn complete_staged_login(
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
