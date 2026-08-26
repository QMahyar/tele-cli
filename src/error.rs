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
    Other(String),
}

impl TeleError {
    pub fn exit_code(&self) -> i32 {
        match self {
            TeleError::Usage(_) => EXIT_USAGE,
            TeleError::Config(_) => EXIT_USAGE,
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
            | TeleError::Other(m) => m,
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
                value["name"] = serde_json::json!(name);
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

impl From<anyhow::Error> for TeleError {
    fn from(e: anyhow::Error) -> Self {
        TeleError::Other(format!("{e:#}"))
    }
}

impl From<std::io::Error> for TeleError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            TeleError::BrokenPipe
        } else {
            TeleError::Other(e.to_string())
        }
    }
}

impl From<serde_json::Error> for TeleError {
    fn from(e: serde_json::Error) -> Self {
        TeleError::Other(e.to_string())
    }
}

pub type TeleResult<T> = Result<T, TeleError>;

pub fn aggregate_exit_code(ok_count: usize, failed: &[i32]) -> i32 {
    if failed.is_empty() {
        EXIT_OK
    } else if ok_count > 0 {
        EXIT_PARTIAL
    } else if failed.contains(&EXIT_AUTH) {
        EXIT_AUTH
    } else if failed.iter().any(|c| *c != EXIT_USAGE) {
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

pub fn invocation_error(e: grammers_client::InvocationError) -> TeleError {
    if invocation_is_unauthorized(&e) {
        if let grammers_client::InvocationError::Rpc(rpc) = &e {
            TeleError::Auth(format!("not logged in (session invalid): {}", rpc.name))
        } else {
            TeleError::Auth("not logged in (session invalid)".to_string())
        }
    } else {
        match e {
            grammers_client::InvocationError::Rpc(rpc) => {
                let seconds = if rpc.code == 420 { rpc.value } else { None };
                TeleError::Rpc(rpc.to_string(), rpc.code, rpc.name, seconds)
            }
            other => TeleError::Invocation(invocation_message(&other), None),
        }
    }
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
}
