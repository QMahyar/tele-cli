use crate::client::ClientGuard;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::output::{self, log_line};
use hmac::Hmac;
use num_bigint::BigUint;
use sha2::{Digest, Sha256, Sha512};
use std::io::Write;

use super::*;

pub(crate) const NO_SRP_CHALLENGE_MSG: &str =
    "GetPassword response is missing SRP challenge parameters; retry the command";

pub(crate) fn plan_password_step(mode: PasswordMode, has_password: bool) -> TeleResult<()> {
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

pub(crate) fn sh(data: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(data);
    hasher.update(salt);
    hasher.finalize().into()
}

pub(crate) fn ph1(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    sh(&sh(password, salt1), salt2)
}

pub(crate) fn ph2(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    let hash1 = ph1(password, salt1, salt2);
    let mut dk = [0u8; 64];
    pbkdf2::pbkdf2::<Hmac<Sha512>>(&hash1, salt1, 100000, &mut dk)
        .expect("pbkdf2 cannot fail for a 64-byte output");
    sh(&dk, salt2)
}

pub(crate) fn compute_new_password_hash(
    password: &str,
    salt1: &[u8],
    salt2: &[u8],
    g: i32,
    p: &[u8],
) -> TeleResult<Vec<u8>> {
    use grammers_crypto::two_factor_auth::check_p_and_g;
    if !(2..=7).contains(&g) {
        return Err(TeleError::Other(format!(
            "unsupported SRP generator g={g}; cannot compute password hash"
        )));
    }
    if !check_p_and_g(p, &g) {
        return Err(TeleError::Other(
            "invalid SRP prime parameters; cannot compute password hash".to_string(),
        ));
    }
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
    Ok(out)
}

pub(crate) fn extend_salt1(base_salt1: &[u8], extra: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(base_salt1.len() + 32);
    out.extend_from_slice(base_salt1);
    out.extend_from_slice(extra);
    out
}

pub(crate) fn generate_secure_32() -> TeleResult<[u8; 32]> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| TeleError::Other(format!("system entropy unavailable: {e}")))?;
    Ok(buf)
}

pub(crate) fn new_password_algo_and_hash(
    password: &str,
    base: &grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow,
    extra: Option<[u8; 32]>,
) -> TeleResult<(grammers_client::tl::enums::PasswordKdfAlgo, Vec<u8>)> {
    let extra = match extra {
        Some(e) => e,
        None => generate_secure_32()?,
    };
    let new_salt1 = extend_salt1(&base.salt1, &extra);
    let hash = compute_new_password_hash(password, &new_salt1, &base.salt2, base.g, &base.p)?;
    let algo = grammers_client::tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
        salt1: new_salt1,
        salt2: base.salt2.clone(),
        g: base.g,
        p: base.p.clone(),
    };
    let algo_enum = grammers_client::tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(algo);
    Ok((algo_enum, hash))
}

pub(crate) fn extract_new_algo(
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

pub(crate) fn prompt_password_with_echo(prompt: &str) -> TeleResult<String> {
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

pub(crate) fn prompt_new_password_pair() -> TeleResult<String> {
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

pub(crate) async fn set_cloud_password(
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
    let (algo, hash) = new_password_algo_and_hash(&new_password, &base, None)?;
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

pub(crate) async fn update_settings_with_email_loop(
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

pub(crate) fn is_email_unconfirmed(e: &grammers_client::InvocationError) -> bool {
    e.to_string().contains("EMAIL_UNCONFIRMED")
}

pub(crate) async fn fetch_password(
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

pub(crate) async fn confirm_pending_password_email(guard: &ClientGuard) -> TeleResult<()> {
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

pub(crate) fn prompt_plain_line(prompt: &str) -> TeleResult<String> {
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let read = prompt_line(prompt, &mut stdin, &mut stderr);
    let _ = writeln!(stderr);
    let Some(line) = read? else {
        return Err(TeleError::Usage("input required; stdin closed".to_string()));
    };
    Ok(strip_line_ending(&line).to_string())
}

pub(crate) async fn confirm_password_email(guard: &ClientGuard, code: &str) -> TeleResult<()> {
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

pub(crate) async fn resend_password_email(guard: &ClientGuard) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::ResendPasswordEmail {})
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

pub(crate) async fn cancel_password_email(guard: &ClientGuard) -> TeleResult<()> {
    guard.rate_limiter.acquire().await;
    guard
        .client
        .invoke(&grammers_client::tl::functions::account::CancelPasswordEmail {})
        .await
        .map_err(map_update_password_error)?;
    Ok(())
}

pub(crate) async fn password_status(guard: &ClientGuard) -> TeleResult<serde_json::Value> {
    let pw = fetch_password(guard).await?;
    Ok(serde_json::json!({
        "has_password": pw.has_password,
        "has_recovery": pw.has_recovery,
        "hint": pw.hint,
        "email_unconfirmed_pattern": pw.email_unconfirmed_pattern,
        "pending_reset_date": pw.pending_reset_date.map(|d| {
            chrono::DateTime::from_timestamp(i64::from(d), 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        }),
    }))
}

pub(crate) async fn start_password_reset(guard: &ClientGuard) -> TeleResult<serde_json::Value> {
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
                "until_date": chrono::DateTime::from_timestamp(i64::from(w.until_date), 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })
        }
        grammers_client::tl::enums::account::ResetPasswordResult::ResetPasswordFailedWait(w) => {
            serde_json::json!({
                "result": "failed_wait",
                "retry_date": chrono::DateTime::from_timestamp(i64::from(w.retry_date), 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })
        }
    };
    Ok(value)
}

pub(crate) async fn decline_password_reset(guard: &ClientGuard) -> TeleResult<()> {
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

pub(crate) async fn change_cloud_password(
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
    let (algo, hash) = new_password_algo_and_hash(&new_password, &base, None)?;
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
pub(crate) struct SrpParams {
    pub(crate) salt1: Vec<u8>,
    pub(crate) salt2: Vec<u8>,
    pub(crate) p: Vec<u8>,
    pub(crate) g: i32,
}

pub(crate) fn extract_srp_params(
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

pub(crate) fn input_check_password_srp(
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

pub(crate) fn map_update_password_error(e: grammers_client::InvocationError) -> TeleError {
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

pub(crate) fn prompt_current_password_proof(
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

pub(crate) async fn remove_cloud_password(guard: &ClientGuard) -> TeleResult<()> {
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
pub(crate) fn disable_stdin_echo() -> bool {
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
pub(crate) fn restore_stdin_echo(disabled: bool) {
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
pub(crate) fn disable_stdin_echo() -> bool {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().lock().as_raw_fd();
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return false;
        }
        let orig = termios;
        termios.c_lflag &= !libc::ECHO;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return false;
        }
        ECHO_RESTORE.store(true, std::sync::atomic::Ordering::Relaxed);
        ORIG_TERMIOS.lock().unwrap().replace(orig);
        true
    }
}

#[cfg(not(windows))]
pub(crate) fn restore_stdin_echo(disabled: bool) {
    if !disabled {
        return;
    }
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().lock().as_raw_fd();
    unsafe {
        if let Some(orig) = ORIG_TERMIOS.lock().unwrap().take() {
            let _ = libc::tcsetattr(fd, libc::TCSANOW, &orig);
        }
    }
    ECHO_RESTORE.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(windows))]
pub(crate) static ECHO_RESTORE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(not(windows))]
pub(crate) static ORIG_TERMIOS: std::sync::Mutex<Option<libc::termios>> =
    std::sync::Mutex::new(None);
