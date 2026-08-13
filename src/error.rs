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
    Invocation(String),
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
            | TeleError::Invocation(m)
            | TeleError::Other(m) => m,
        }
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
