pub mod account;
pub mod chat;
pub mod completions;
pub mod contact;
pub mod credentials;
pub mod dialog;
pub mod helpers;
pub mod listen;
pub mod mcp;
pub mod msg;
pub mod privacy;
pub mod profile;
pub mod raw;
pub mod serve;
pub mod stickers;
pub mod stories;
pub mod takeout;
pub mod topic;

pub use crate::error::{TeleError, TeleResult};

pub fn validate_limit(value: u32, max: u32, flag: &str) -> Result<u32, TeleError> {
    if value == 0 {
        return Err(TeleError::Usage(format!(
            "--{flag} must be between 1 and {max} (got 0)"
        )));
    }
    if value > max {
        Err(TeleError::Usage(format!(
            "--{flag} too large: {value} (max {max})"
        )))
    } else {
        Ok(value)
    }
}

pub fn parse_unixtime(value: &str) -> Result<chrono::DateTime<chrono::Utc>, TeleError> {
    if let Ok(ts) = value.parse::<i64>() {
        return chrono::DateTime::from_timestamp(ts, 0)
            .ok_or_else(|| TeleError::Usage(format!("invalid timestamp {value}")));
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| TeleError::Usage(format!("invalid date {value}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_within_max_passes() {
        assert_eq!(validate_limit(10, 10_000, "limit").unwrap(), 10);
    }

    #[test]
    fn limit_over_max_rejected() {
        assert!(matches!(
            validate_limit(10_001, 10_000, "limit"),
            Err(TeleError::Usage(_))
        ));
    }
}


