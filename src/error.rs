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
    Other(String),
}

impl TeleError {
    pub fn exit_code(&self) -> i32 {
        match self {
            TeleError::Usage(_) => EXIT_USAGE,
            TeleError::Config(_) => EXIT_USAGE,
            TeleError::Auth(_) => EXIT_AUTH,
            _ => EXIT_ALL_FAILED,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            TeleError::Usage(m)
            | TeleError::Auth(m)
            | TeleError::Config(m)
            | TeleError::Invocation(m, _)
            | TeleError::Other(m) => m,
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        let kind = match self {
            TeleError::Usage(_) => "UsageError",
            TeleError::Auth(_) => "AuthError",
            TeleError::Config(_) => "ConfigError",
            TeleError::Invocation(..) => "InvocationError",
            TeleError::Other(_) => "Error",
        };
        let mut value = serde_json::json!({ "type": kind, "message": self.message() });
        if let TeleError::Invocation(_, Some(seconds)) = self {
            value["seconds"] = serde_json::json!(seconds);
        }
        value
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
        TeleError::Other(e.to_string())
    }
}

impl From<serde_json::Error> for TeleError {
    fn from(e: serde_json::Error) -> Self {
        TeleError::Other(e.to_string())
    }
}

pub type TeleResult<T> = Result<T, TeleError>;

pub fn invocation_is_unauthorized(e: &grammers_client::InvocationError) -> bool {
    matches!(e, grammers_client::InvocationError::Rpc(rpc) if rpc.code == 401)
}

pub fn invocation_message(e: &grammers_client::InvocationError) -> String {
    match e {
        grammers_client::InvocationError::Rpc(rpc) => rpc.to_string(),
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
        TeleError::Auth("not logged in (session invalid)".to_string())
    } else {
        TeleError::Invocation(invocation_message(&e), invocation_wait_seconds(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::sender::RpcError;

    fn flood(seconds: u32) -> TeleError {
        TeleError::Invocation(
            invocation_message(&grammers_client::InvocationError::Rpc(RpcError {
                code: 420,
                name: "FLOOD_WAIT".to_string(),
                value: Some(seconds),
                caused_by: None,
            })),
            invocation_wait_seconds(&grammers_client::InvocationError::Rpc(RpcError {
                code: 420,
                name: "FLOOD_WAIT".to_string(),
                value: Some(seconds),
                caused_by: None,
            })),
        )
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
        assert_eq!(err.message(), "not logged in (session invalid)");
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
        assert!(matches!(err, TeleError::Invocation(_, None)));
        assert_eq!(err.exit_code(), EXIT_ALL_FAILED);
        assert_eq!(err.message(), "rpc error 400: CHAT_INVALID");
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
        assert!(matches!(err, TeleError::Invocation(_, Some(17))));
        assert_eq!(err.message(), "rpc error 420: FLOOD_WAIT (value: 17)");
    }
}
