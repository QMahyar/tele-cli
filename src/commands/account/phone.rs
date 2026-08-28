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
#[derive(Args)]
pub struct PhoneArgs {
    #[arg(
        long,
        value_name = "+PHONE",
        help = "send the change-phone code to this new number"
    )]
    pub change_phone: Option<String>,
    #[arg(
        long,
        help = "allow flash-call verification when sending the change-phone code"
    )]
    pub allow_flashcall: bool,
    #[arg(
        long,
        value_name = "CODE",
        help = "confirm the pending phone change with this code"
    )]
    pub confirm_code: Option<String>,
    #[arg(
        long,
        value_name = "HASH",
        help = "phone_code_hash printed by --change-phone"
    )]
    pub phone_hash: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingPhone {
    pub(crate) version: u32,
    pub(crate) account: String,
    pub(crate) phone: String,
    pub(crate) phone_code_hash: String,
    pub(crate) created_at: String,
}

pub(crate) const PENDING_PHONE_VERSION: u32 = 1;

impl PendingPhone {
    pub(crate) fn new(account: &str, phone: &str, phone_code_hash: String) -> Self {
        Self {
            version: PENDING_PHONE_VERSION,
            account: account.to_string(),
            phone: phone.to_string(),
            phone_code_hash,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub(crate) fn save_pending_phone(pending: &PendingPhone) -> TeleResult<()> {
    save_pending_phone_under(&config::app_data_dir(), pending)
}

pub(crate) fn save_pending_phone_under(
    base: &std::path::Path,
    pending: &PendingPhone,
) -> TeleResult<()> {
    let text = serde_json::to_string_pretty(pending)?;
    save_pending_document_under(base, &phone_pending_file(&pending.account), &text)
}

pub(crate) fn load_pending_phone_under(
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

pub(crate) fn require_pending_phone(name: &str) -> TeleResult<PendingPhone> {
    require_pending_phone_under(&config::app_data_dir(), name)
}

pub(crate) fn require_pending_phone_under(
    base: &std::path::Path,
    name: &str,
) -> TeleResult<PendingPhone> {
    load_pending_phone_under(base, name)?.ok_or_else(|| {
        TeleError::Usage(format!(
            "no pending phone change for account {name}; run tele account phone --change-phone first"
        ))
    })
}

pub(crate) fn remove_pending_phone(name: &str) -> TeleResult<bool> {
    remove_pending_phone_under(&config::app_data_dir(), name)
}

pub(crate) fn remove_pending_phone_under(base: &std::path::Path, name: &str) -> TeleResult<bool> {
    remove_pending_document_under(base, &phone_pending_file(name))
}

pub(crate) fn phone_hash_matches(pending: &PendingPhone, hash: &str) -> bool {
    pending.phone_code_hash == hash.trim()
}

#[derive(Debug, Clone)]
pub(crate) enum PhoneAction {
    Send { phone: String, flashcall: bool },
    Confirm { code: String, hash: String },
}

impl PhoneAction {
    pub(crate) fn describe(&self, name: &str) -> String {
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

pub(crate) fn validate_phone_modes(args: &PhoneArgs) -> TeleResult<PhoneAction> {
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

pub(crate) fn phone_dry_run_data(name: &str, action: &PhoneAction) -> serde_json::Value {
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

pub(crate) async fn send_change_phone_code(
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

pub(crate) async fn confirm_change_phone(
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

pub(crate) async fn execute_phone_action(
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

pub(crate) async fn phone(args: &PhoneArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
