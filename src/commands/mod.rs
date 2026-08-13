pub mod account;
pub mod chat;
pub mod contact;
pub mod dialog;
pub mod listen;
pub mod msg;
pub mod privacy;
pub mod profile;
pub mod raw;
pub mod takeout;
pub mod topic;

pub use crate::error::TeleError;

pub fn parse_unixtime(value: &str) -> Result<chrono::DateTime<chrono::Utc>, TeleError> {
    if let Ok(ts) = value.parse::<i64>() {
        return chrono::DateTime::from_timestamp(ts, 0)
            .ok_or_else(|| TeleError::Usage(format!("invalid timestamp {value}")));
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| TeleError::Usage(format!("invalid date {value}: {e}")))
}
