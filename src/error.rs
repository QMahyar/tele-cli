use std::sync::LazyLock;

use base64::Engine as _;
use regex::Regex;

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_PARTIAL: i32 = 2;
pub const EXIT_ALL_FAILED: i32 = 3;
pub const EXIT_AUTH: i32 = 4;
pub const EXIT_INTERRUPTED: i32 = 130;

#[derive(Debug)]
pub enum TeleError {
    Usage(String),
    Auth(String),
    Config(String),
    Invocation(String, Option<u32>),
    Rpc(String, i32, String, Option<u32>),
    TaskPanic(String),
    BrokenPipe,
    Timeout(String),
    Other(String),
}

impl TeleError {
    pub fn exit_code(&self) -> i32 {
        match self {
            TeleError::Usage(_) => EXIT_USAGE,
            TeleError::Config(_) => EXIT_USAGE,
            TeleError::Timeout(_) => EXIT_USAGE,
            TeleError::Auth(_) => EXIT_AUTH,
            TeleError::BrokenPipe => EXIT_OK,
            _ => EXIT_ALL_FAILED,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            TeleError::Usage(m)
            | TeleError::Auth(m)
            | TeleError::Config(m)
            | TeleError::Invocation(m, _)
            | TeleError::Rpc(m, ..)
            | TeleError::TaskPanic(m)
            | TeleError::Timeout(m)
            | TeleError::Other(m) => {
                let scrubbed = scrub(m.clone());
                if scrubbed == *m {
                    m.as_str()
                } else {
                    Box::leak(scrubbed.into_boxed_str())
                }
            }
            TeleError::BrokenPipe => "output stream closed",
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        let kind = match self {
            TeleError::Usage(_) => "UsageError",
            TeleError::Auth(_) => "AuthError",
            TeleError::Config(_) => "ConfigError",
            TeleError::Invocation(..) => "InvocationError",
            TeleError::Rpc(..) => "InvocationError",
            TeleError::TaskPanic(_) => "TaskPanicError",
            TeleError::Timeout(_) => "Timeout",
            TeleError::Other(_) => "Error",
            TeleError::BrokenPipe => "Error",
        };
        let mut value = serde_json::json!({ "type": kind, "message": self.message() });
        match self {
            TeleError::Invocation(_, Some(seconds)) => {
                value["seconds"] = serde_json::json!(seconds);
            }
            TeleError::Rpc(_, code, name, seconds) => {
                value["code"] = serde_json::json!(code);
                value["name"] = serde_json::json!(scrub(name.clone()));
                if let Some(seconds) = seconds {
                    value["seconds"] = serde_json::json!(seconds);
                }
            }
            _ => {}
        }
        value
    }

    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, TeleError::BrokenPipe)
    }
}

impl std::fmt::Display for TeleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TeleError {}

static PHONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{7,15}\b").unwrap());
static PHONE_PLUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+\d{7,15}\b").unwrap());
static FORMATTED_PHONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+?\d[\d\s\-\(\)]{5,30}\d").unwrap());
static LONG_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/=_-]{40,}").unwrap());
static QR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"tg://login\?token=[^\s]+").unwrap());
static PASSWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(password\s*[:=]\s*)\S+").unwrap());

static CACHED_FILE_SECRETS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let path = crate::config::app_data_dir().join(".env");
    let map = crate::config::load_env(&path);
    let mut secrets = Vec::new();
    for key in ["TELE_API_HASH", "TELE_API_ID"] {
        if let Some(v) = map.get(key) {
            let t = v.trim();
            if !t.is_empty() {
                secrets.push(t.to_string());
                let enc = url_encode(t);
                if enc != t {
                    secrets.push(enc);
                }
            }
        }
    }
    secrets
});

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn scrub_phones(s: String) -> String {
    let out = PHONE_RE.replace_all(&s, "[REDACTED]").into_owned();
    let out = PHONE_PLUS_RE.replace_all(&out, "[REDACTED]").into_owned();
    FORMATTED_PHONE_RE
        .replace_all(&out, |caps: &regex::Captures| {
            let m = caps.get(0).unwrap().as_str();
            if m.contains("[REDACTED]") {
                return "[REDACTED]".to_string();
            }
            let digits: String = m.chars().filter(|c| c.is_ascii_digit()).collect();
            if (7..=15).contains(&digits.len()) {
                "[REDACTED]".to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned()
}

fn scrub(s: String) -> String {
    let mut out = scrub_phones(s);
    for key in ["TELE_API_HASH", "TELE_API_ID"] {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim().to_string();
            if !t.is_empty() && out.contains(&t) {
                out = out.replace(&t, "[REDACTED]");
            }
            let enc = url_encode(&t);
            if enc != t && out.contains(&enc) {
                out = out.replace(&enc, "[REDACTED]");
            }
            if !t.is_empty() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(t.as_bytes());
                if b64 != t && out.contains(&b64) {
                    out = out.replace(&b64, "[REDACTED]");
                }
            }
        }
    }
    for secret in CACHED_FILE_SECRETS.iter() {
        if out.contains(secret) {
            out = out.replace(secret, "[REDACTED]");
        }
        let enc = url_encode(secret);
        if enc != *secret && out.contains(&enc) {
            out = out.replace(&enc, "[REDACTED]");
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
        if out.contains(&b64) {
            out = out.replace(&b64, "[REDACTED]");
        }
    }
    out = LONG_TOKEN_RE.replace_all(&out, "[REDACTED]").into_owned();
    out = QR_RE.replace_all(&out, "[REDACTED]").into_owned();
    out = PASSWORD_RE.replace_all(&out, "${1}[REDACTED]").into_owned();
    out
}

impl From<anyhow::Error> for TeleError {
    fn from(e: anyhow::Error) -> Self {
        TeleError::Other(scrub(format!("{e:#}")))
    }
}

impl From<std::io::Error> for TeleError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            TeleError::BrokenPipe
        } else {
            TeleError::Other(scrub(e.to_string()))
        }
    }
}

impl From<serde_json::Error> for TeleError {
    fn from(e: serde_json::Error) -> Self {
        TeleError::Usage(scrub(e.to_string()))
    }
}

pub type TeleResult<T> = Result<T, TeleError>;

pub fn aggregate_exit_code(ok_count: usize, failed: &[i32]) -> i32 {
    let filtered: Vec<i32> = failed.iter().copied().filter(|c| *c != EXIT_OK).collect();
    if filtered.is_empty() {
        EXIT_OK
    } else if ok_count > 0 {
        EXIT_PARTIAL
    } else if filtered.contains(&EXIT_AUTH) {
        EXIT_AUTH
    } else if filtered.iter().any(|c| *c != EXIT_USAGE) {
        EXIT_ALL_FAILED
    } else {
        EXIT_USAGE
    }
}

pub fn invocation_is_unauthorized(e: &grammers_client::InvocationError) -> bool {
    matches!(e, grammers_client::InvocationError::Rpc(rpc) if rpc.code == 401)
}

pub const PEER_UNKNOWN_HINT: &str =
    "peer unknown to this session; run tele dialog list to refresh the peer cache";

pub fn invocation_message(e: &grammers_client::InvocationError) -> String {
    match e {
        grammers_client::InvocationError::Rpc(rpc) => rpc.to_string(),
        grammers_client::InvocationError::Dropped => PEER_UNKNOWN_HINT.to_string(),
        other => other.to_string(),
    }
}

pub fn invocation_wait_seconds(e: &grammers_client::InvocationError) -> Option<u32> {
    match e {
        grammers_client::InvocationError::Rpc(rpc) if rpc.code == 420 && rpc.value.is_some() => {
            rpc.value
        }
        _ => None,
    }
}

pub fn invocation_error_ref(e: &grammers_client::InvocationError) -> TeleError {
    if invocation_is_unauthorized(e) {
        if let grammers_client::InvocationError::Rpc(rpc) = e {
            TeleError::Auth(format!("not logged in (session invalid): {}", rpc.name))
        } else {
            TeleError::Auth("not logged in (session invalid)".to_string())
        }
    } else {
        match e {
            grammers_client::InvocationError::Rpc(rpc) => {
                let seconds = if rpc.code == 420 { rpc.value } else { None };
                TeleError::Rpc(scrub(rpc.to_string()), rpc.code, scrub(rpc.name.clone()), seconds)
            }
            other => TeleError::Invocation(scrub(invocation_message(other)), None),
        }
    }
}

pub fn invocation_error(e: grammers_client::InvocationError) -> TeleError {
    invocation_error_ref(&e)
}

pub use invocation_error as tele_invocation;

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::sender::RpcError;

    fn rpc420(name: &str, seconds: u32) -> grammers_client::InvocationError {
        grammers_client::InvocationError::Rpc(RpcError {
            code: 420,
            name: name.to_string(),
            value: Some(seconds),
            caused_by: None,
        })
    }

    fn flood(seconds: u32) -> TeleError {
        let e = rpc420("FLOOD_WAIT", seconds);
        TeleError::Invocation(invocation_message(&e), invocation_wait_seconds(&e))
    }

    #[test]
    fn slowmode_wait_error_carries_seconds_like_flood() {
        let e = rpc420("SLOWMODE_WAIT", 30);
        assert_eq!(invocation_wait_seconds(&e), Some(30));
        let err = invocation_error(e);
        assert!(matches!(err, TeleError::Rpc(_, 420, _, Some(30))));
        assert_eq!(err.message(), "rpc error 420: SLOWMODE_WAIT (value: 30)");
        let v = err.as_json();
        assert_eq!(v["type"], "InvocationError");
        assert_eq!(v["code"], 420);
        assert_eq!(v["name"], "SLOWMODE_WAIT");
        assert_eq!(v["seconds"], 30);
    }

    #[test]
    fn slowmode_wait_zero_seconds_is_some_zero() {
        let e = rpc420("SLOWMODE_WAIT", 0);
        assert_eq!(invocation_wait_seconds(&e), Some(0));
    }

    #[test]
    fn non_wait_420_names_still_carry_value_seconds() {
        let e = rpc420("FLOOD_PREMIUM_WAIT_X", 5);
        assert_eq!(invocation_wait_seconds(&e), Some(5));
    }

    #[test]
    fn flood_wait_error_carries_seconds() {
        let v = flood(17).as_json();
        assert_eq!(v["type"], "InvocationError");
        assert_eq!(v["seconds"], 17);
    }

    #[test]
    fn non_flood_invocation_has_no_seconds() {
        let e = TeleError::Invocation("request error: dropped (cancelled)".to_string(), None);
        let v = e.as_json();
        assert_eq!(v["type"], "InvocationError");
        assert!(v.get("seconds").is_none());
    }

    #[test]
    fn wait_seconds_only_for_flood_code() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 400,
            name: "CHAT_INVALID".to_string(),
            value: Some(17),
            caused_by: None,
        });
        assert_eq!(invocation_wait_seconds(&e), None);
    }

    #[test]
    fn flood_wait_zero_seconds_is_some_zero() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 420,
            name: "FLOOD_WAIT".to_string(),
            value: Some(0),
            caused_by: None,
        });
        assert_eq!(invocation_wait_seconds(&e), Some(0));
        let v = TeleError::Invocation("FLOOD_WAIT 0".to_string(), Some(0)).as_json();
        assert_eq!(v["seconds"], 0);
    }

    #[test]
    fn flood_code_without_seconds_is_none() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 420,
            name: "FLOOD_WAIT".to_string(),
            value: None,
            caused_by: None,
        });
        assert_eq!(invocation_wait_seconds(&e), None);
    }

    #[test]
    fn exit_code_taxonomy_is_locked() {
        assert_eq!(EXIT_OK, 0);
        assert_eq!(EXIT_USAGE, 1);
        assert_eq!(EXIT_PARTIAL, 2);
        assert_eq!(EXIT_ALL_FAILED, 3);
        assert_eq!(EXIT_AUTH, 4);
        assert_eq!(EXIT_INTERRUPTED, 130);
    }

    #[test]
    fn usage_errors_exit_one() {
        assert_eq!(TeleError::Usage("x".to_string()).exit_code(), EXIT_USAGE);
    }

    #[test]
    fn auth_errors_exit_four() {
        assert_eq!(TeleError::Auth("x".to_string()).exit_code(), EXIT_AUTH);
    }

    #[test]
    fn config_exits_usage() {
        assert_eq!(TeleError::Config("x".to_string()).exit_code(), EXIT_USAGE);
    }

    #[test]
    fn invocation_and_other_exit_all_failed() {
        assert_eq!(
            TeleError::Invocation("x".to_string(), None).exit_code(),
            EXIT_ALL_FAILED
        );
        assert_eq!(
            TeleError::TaskPanic("x".to_string()).exit_code(),
            EXIT_ALL_FAILED
        );
        assert_eq!(
            TeleError::Other("x".to_string()).exit_code(),
            EXIT_ALL_FAILED
        );
    }

    #[test]
    fn auth_error_json_kind() {
        let v = TeleError::Auth("session invalid".to_string()).as_json();
        assert_eq!(v["type"], "AuthError");
        assert!(v.get("seconds").is_none());
    }

    #[test]
    fn unauthorized_invocation_is_rpc_401() {
        let unauthorized = grammers_client::InvocationError::Rpc(RpcError {
            code: 401,
            name: "AUTH_KEY_UNREGISTERED".to_string(),
            value: None,
            caused_by: None,
        });
        assert!(invocation_is_unauthorized(&unauthorized));
        let denied = grammers_client::InvocationError::Rpc(RpcError {
            code: 403,
            name: "AUTH_KEY_INVALID".to_string(),
            value: None,
            caused_by: None,
        });
        assert!(!invocation_is_unauthorized(&denied));
    }

    #[test]
    fn invocation_error_classifies_401_as_auth() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 401,
            name: "AUTH_KEY_UNREGISTERED".to_string(),
            value: None,
            caused_by: None,
        });
        let err = invocation_error(e);
        assert!(matches!(err, TeleError::Auth(_)));
        assert_eq!(err.exit_code(), EXIT_AUTH);
        assert_eq!(
            err.message(),
            "not logged in (session invalid): AUTH_KEY_UNREGISTERED"
        );
    }

    #[test]
    fn invocation_error_classifies_other_rpc_as_invocation() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 400,
            name: "CHAT_INVALID".to_string(),
            value: None,
            caused_by: None,
        });
        let err = invocation_error(e);
        assert!(matches!(err, TeleError::Rpc(_, 400, _, None)));
        assert_eq!(err.exit_code(), EXIT_ALL_FAILED);
        assert_eq!(err.message(), "rpc error 400: CHAT_INVALID");
        let v = err.as_json();
        assert_eq!(v["code"], 400);
        assert_eq!(v["name"], "CHAT_INVALID");
        assert!(v.get("seconds").is_none());
    }

    #[test]
    fn rpc_error_json_carries_code_and_name() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 403,
            name: "CHAT_WRITE_FORBIDDEN".to_string(),
            value: None,
            caused_by: None,
        });
        let v = invocation_error(e).as_json();
        assert_eq!(v["type"], "InvocationError");
        assert_eq!(v["code"], 403);
        assert_eq!(v["name"], "CHAT_WRITE_FORBIDDEN");
    }

    #[test]
    fn invocation_error_carries_flood_seconds() {
        let e = grammers_client::InvocationError::Rpc(RpcError {
            code: 420,
            name: "FLOOD_WAIT".to_string(),
            value: Some(17),
            caused_by: None,
        });
        let err = invocation_error(e);
        assert!(matches!(err, TeleError::Rpc(_, 420, _, Some(17))));
        assert_eq!(err.message(), "rpc error 420: FLOOD_WAIT (value: 17)");
    }

    #[test]
    fn dropped_invocation_translates_to_peer_hint() {
        let err = invocation_error(grammers_client::InvocationError::Dropped);
        assert_eq!(err.message(), PEER_UNKNOWN_HINT);
    }

    #[test]
    fn broken_pipe_maps_from_io_error_and_exits_ok() {
        let io_err = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        let err: TeleError = io_err.into();
        assert!(err.is_broken_pipe());
        assert_eq!(err.exit_code(), EXIT_OK);
        assert_eq!(err.message(), "output stream closed");
    }

    #[test]
    fn broken_pipe_is_not_confused_with_other_errors() {
        assert!(!TeleError::Other("pipe?".to_string()).is_broken_pipe());
    }

    #[test]
    fn aggregate_exit_code_filters_broken_pipe_ok() {
        let failed = vec![EXIT_OK, EXIT_ALL_FAILED];
        assert_eq!(aggregate_exit_code(0, &failed), EXIT_ALL_FAILED);
    }

    #[test]
    fn aggregate_exit_code_only_broken_pipe_ok_is_ok() {
        let failed = vec![EXIT_OK];
        assert_eq!(aggregate_exit_code(0, &failed), EXIT_OK);
    }

    #[test]
    fn aggregate_exit_code_empty_failed_is_ok() {
        assert_eq!(aggregate_exit_code(0, &[]), EXIT_OK);
    }

    #[test]
    fn test_scrub_phones_bare_number() {
        let err = TeleError::Usage("call 1234567 now".to_string());
        assert!(!err.message().contains("1234567"));
        assert!(err.message().contains("[REDACTED]"));
        let err2 = TeleError::Other("number 123456789012345 is secret".to_string());
        assert!(!err2.message().contains("123456789012345"));
        assert!(err2.message().contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_phones_with_plus() {
        let err = TeleError::Usage("phone +1234567890 failed".to_string());
        assert!(!err.message().contains("1234567890"));
        assert!(err.message().contains("[REDACTED]"));
        assert!(!err.as_json()["message"].as_str().unwrap().contains("1234567890"));
    }

    #[test]
    fn test_scrub_phones_with_spaces_and_dashes() {
        let err = TeleError::Usage("contact 123 456 7890 error".to_string());
        assert!(!err.message().contains("123 456 7890"));
        assert!(err.message().contains("[REDACTED]"));
        let err2 = TeleError::Usage("dial 123-456-7890 now".to_string());
        assert!(!err2.message().contains("123-456-7890"));
        assert!(err2.message().contains("[REDACTED]"));
        let err3 = TeleError::Usage("dial (123) 456-7890".to_string());
        assert!(err3.message().contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_phones_boundary_7_to_15() {
        let err_short = TeleError::Usage("code 123456 is ok".to_string());
        assert_eq!(err_short.message(), "code 123456 is ok");
        let err_long = TeleError::Usage("id 1234567890123456 is long".to_string());
        assert_eq!(err_long.message(), "id 1234567890123456 is long");
        let err_ok = TeleError::Usage("phone 1234567".to_string());
        assert!(err_ok.message().contains("[REDACTED]"));
        let err_ok2 = TeleError::Usage("phone 123456789012345".to_string());
        assert!(err_ok2.message().contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_hash_exact_and_encoded() {
        let hash = "deadbeefdeadbeefdeadbeefdeadbeef";
        let encoded = url_encode(hash);
        let err = TeleError::Usage(format!("hash {hash} leaked"));
        assert!(!err.message().contains(hash));
        assert!(err.message().contains("[REDACTED]"));
        let err2 = TeleError::Rpc(format!("hash {encoded}"), 400, "TEST".to_string(), None);
        if encoded != hash {
            assert!(!err2.message().contains(&encoded));
        }
        assert!(err2.as_json()["message"].as_str().unwrap().contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_api_id() {
        let id = "1234567";
        let err = TeleError::Usage(format!("api_id {id} leaked"));
        assert!(!err.message().contains(id));
        assert!(err.message().contains("[REDACTED]"));
        let err2 = TeleError::Auth(format!("id {id} in auth"));
        assert!(!err2.as_json()["message"].as_str().unwrap().contains(id));
    }

    #[test]
    fn test_scrub_all_variants() {
        let secret = "1234567890";
        for err in [
            TeleError::Usage(format!("bad {secret}")),
            TeleError::Auth(format!("auth {secret}")),
            TeleError::Config(format!("cfg {secret}")),
            TeleError::Invocation(format!("inv {secret}"), None),
            TeleError::Rpc(format!("rpc {secret}"), 400, "TEST".to_string(), None),
            TeleError::TaskPanic(format!("panic {secret}")),
            TeleError::Timeout(format!("timeout {secret}")),
            TeleError::Other(format!("other {secret}")),
        ] {
            assert!(
                !err.message().contains(secret),
                "variant {} leaked: {}",
                err.as_json()["type"],
                err.message()
            );
            assert!(
                !err.as_json()["message"].as_str().unwrap().contains(secret),
                "json leaked for {}",
                err.as_json()["type"]
            );
        }
    }

    #[test]
    fn test_scrub_session_and_qr_and_password() {
        let session = "A".repeat(50);
        let err = TeleError::Usage(format!("session {session} leaked"));
        assert!(!err.message().contains(&session));
        assert!(err.message().contains("[REDACTED]"));
        let qr = "tg://login?token=abc123DEF456";
        let err2 = TeleError::Other(format!("qr {qr}"));
        assert!(!err2.message().contains("abc123"));
        assert!(err2.message().contains("[REDACTED]"));
        let err3 = TeleError::Usage("password: supersecret123".to_string());
        assert!(!err3.message().contains("supersecret123"));
        assert!(err3.message().contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_no_secrets_passthrough() {
        let msg = "rpc error 420: FLOOD_WAIT (value: 30)";
        let err = TeleError::Rpc(msg.to_string(), 420, "FLOOD_WAIT".to_string(), Some(30));
        assert_eq!(err.message(), msg);
    }

    #[test]
    fn test_scrub_via_from_impls() {
        let err: TeleError = anyhow::anyhow!("phone +1234567890 leaked").into();
        assert!(!err.message().contains("1234567890"));
        assert!(err.message().contains("[REDACTED]"));
        let io_err = std::io::Error::other("hash deadbeefdeadbeef");
        let err2: TeleError = io_err.into();
        assert!(!err2.message().contains("deadbeef") || err2.message().contains("[REDACTED]"));
    }
}
