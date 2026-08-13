pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_PARTIAL: i32 = 2;
pub const EXIT_ALL_FAILED: i32 = 3;
pub const EXIT_AUTH: i32 = 4;
#[allow(dead_code)]
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
}
