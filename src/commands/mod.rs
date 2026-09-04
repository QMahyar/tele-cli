pub mod account;
pub mod cache;
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
pub mod skill;
pub mod stickers;
pub mod stories;
pub mod takeout;
pub mod topic;

use crate::error::TeleError;

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
    if let Ok(dt) = parse_duration_from_now(value) {
        return Ok(dt);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| TeleError::Usage(format!("invalid date {value}: {e}")))
}

// Accepts relative durations like `90s`, `30m`, `24h`, `7d`, `2w`, or a
// leading `+` variant (`+90s`), mirroring the chat invite `--expire` syntax.
// The value must parse fully; anything else falls through to the caller's
// error path.
fn parse_duration_from_now(value: &str) -> Result<chrono::DateTime<chrono::Utc>, ()> {
    let raw = value.strip_prefix('+').unwrap_or(value);
    if raw.is_empty() || !raw.chars().last().is_some_and(|c| c.is_ascii_alphabetic()) {
        return Err(());
    }
    // A leading minus is never a valid relative schedule (nothing schedules
    // into the past); reject before stripping so `-3h` does not become `3h`.
    if raw.starts_with('-') {
        return Err(());
    }
    let (num, unit) = raw.split_at(raw.len() - 1);
    let secs: i64 = match (num.parse::<i64>(), unit) {
        (Ok(n), "s") => n,
        (Ok(n), "m") => match n.checked_mul(60) {
            Some(v) => v,
            None => return Err(()),
        },
        (Ok(n), "h") => match n.checked_mul(3600) {
            Some(v) => v,
            None => return Err(()),
        },
        (Ok(n), "d") => match n.checked_mul(86_400) {
            Some(v) => v,
            None => return Err(()),
        },
        (Ok(n), "w") => match n.checked_mul(604_800) {
            Some(v) => v,
            None => return Err(()),
        },
        _ => return Err(()),
    };
    let now = chrono::Utc::now();
    now.checked_add_signed(chrono::Duration::seconds(secs))
        .ok_or(())
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

    #[test]
    fn unixtime_accepts_duration_suffixes() {
        let base = chrono::Utc::now();
        for (value, expect_secs) in [
            ("90s", 90i64),
            ("30m", 1_800),
            ("24h", 86_400),
            ("7d", 604_800),
            ("2w", 1_209_600),
            ("+90s", 90),
        ] {
            let dt = parse_unixtime(value).unwrap_or_else(|e| panic!("{value}: {e}"));
            let got = (dt - base).num_seconds();
            assert!(
                (got - expect_secs).abs() <= 5,
                "{value}: expected ~{expect_secs}s from now, got {got}s"
            );
        }
    }

    #[test]
    fn unixtime_still_accepts_timestamps_and_rfc3339() {
        let dt = parse_unixtime("1788595200").unwrap();
        assert_eq!(dt.timestamp(), 1_788_595_200);
        let dt = parse_unixtime("2026-09-05T08:00:00Z").unwrap();
        assert_eq!(dt.timestamp(), 1_788_595_200);
    }

    #[test]
    fn unixtime_rejects_garbage_and_zero() {
        // A bare number must stay a timestamp, so `0` is epoch, not a failure.
        assert_eq!(parse_unixtime("0").unwrap().timestamp(), 0);
        assert!(parse_unixtime("90x").is_err());
        assert!(parse_unixtime("hh").is_err());
        assert!(parse_unixtime("").is_err());
        assert!(parse_unixtime("-3h").is_err());
        assert!(parse_unixtime("1.5h").is_err());
        assert!(parse_unixtime("99999999999999999999w").is_err());
    }
}
