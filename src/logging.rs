use std::sync::atomic::{AtomicU8, Ordering};

use log::{LevelFilter, Metadata, Record};

const LEVEL_INFO: u8 = 1;
const LEVEL_ERROR: u8 = 3;

static MIN_LINE: AtomicU8 = AtomicU8::new(LEVEL_INFO);

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

pub fn init() {
    let level = match std::env::var("TELE_LOG").as_deref() {
        Ok("trace") => LevelFilter::Trace,
        Ok("debug") => LevelFilter::Debug,
        Ok("info") => LevelFilter::Info,
        Ok("warn") => LevelFilter::Warn,
        Ok("error") => LevelFilter::Error,
        _ => LevelFilter::Off,
    };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

pub fn set_flags(verbose: u8, quiet: bool) {
    let Some(level) = resolve_log_level(verbose, quiet, std::env::var("TELE_LOG").ok().as_deref())
    else {
        return;
    };
    log::set_max_level(level);
    if quiet {
        MIN_LINE.store(LEVEL_ERROR, Ordering::Relaxed);
    }
}

fn resolve_log_level(verbose: u8, quiet: bool, env: Option<&str>) -> Option<LevelFilter> {
    if quiet {
        return Some(LevelFilter::Error);
    }
    if verbose > 0 {
        return Some(if verbose > 1 {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
    }
    match env {
        Some("trace") => Some(LevelFilter::Trace),
        Some("debug") => Some(LevelFilter::Debug),
        Some("info") => Some(LevelFilter::Info),
        Some("warn") => Some(LevelFilter::Warn),
        Some("error") => Some(LevelFilter::Error),
        _ => None,
    }
}

pub fn min_line_level() -> u8 {
    MIN_LINE.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_line_level_default_is_info() {
        let level = min_line_level();
        assert_eq!(
            level, LEVEL_INFO,
            "default min_line_level should be LEVEL_INFO"
        );
    }

    #[test]
    fn set_flags_quiet_sets_error_level() {
        // Save original
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        set_flags(0, true);
        assert_eq!(min_line_level(), LEVEL_ERROR);
        assert_eq!(log::max_level(), LevelFilter::Error);

        // Restore
        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn set_flags_verbose_1_sets_info_level() {
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        set_flags(1, false);
        assert_eq!(log::max_level(), LevelFilter::Info);

        // Restore
        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn set_flags_verbose_2_sets_debug_level() {
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        set_flags(2, false);
        assert_eq!(log::max_level(), LevelFilter::Debug);

        // Restore
        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn set_flags_verbose_0_no_quiet_does_not_change_level() {
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        set_flags(0, false);
        // Should remain unchanged (whatever the default was)
        assert_eq!(min_line_level(), original);

        // Restore
        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn set_flags_quiet_overrides_verbose() {
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        set_flags(2, true);
        assert_eq!(min_line_level(), LEVEL_ERROR);
        assert_eq!(log::max_level(), LevelFilter::Error);

        // Restore
        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn resolve_quiet_beats_tele_log() {
        assert_eq!(
            resolve_log_level(2, true, Some("debug")),
            Some(LevelFilter::Error)
        );
        assert_eq!(
            resolve_log_level(0, true, Some("debug")),
            Some(LevelFilter::Error)
        );
    }

    #[test]
    fn resolve_verbose_beats_tele_log() {
        assert_eq!(
            resolve_log_level(2, false, Some("off")),
            Some(LevelFilter::Debug)
        );
        assert_eq!(
            resolve_log_level(1, false, Some("error")),
            Some(LevelFilter::Info)
        );
    }

    #[test]
    fn resolve_tele_log_applies_without_flags() {
        assert_eq!(
            resolve_log_level(0, false, Some("debug")),
            Some(LevelFilter::Debug)
        );
        assert_eq!(
            resolve_log_level(0, false, Some("trace")),
            Some(LevelFilter::Trace)
        );
    }

    #[test]
    fn resolve_no_flags_no_env_keeps_default() {
        assert_eq!(resolve_log_level(0, false, None), None);
        assert_eq!(resolve_log_level(0, false, Some("garbage")), None);
    }

    #[test]
    fn set_flags_quiet_beats_tele_log_env() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        std::env::set_var("TELE_LOG", "debug");
        set_flags(0, true);
        assert_eq!(min_line_level(), LEVEL_ERROR);
        assert_eq!(log::max_level(), LevelFilter::Error);
        std::env::remove_var("TELE_LOG");

        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }

    #[test]
    fn set_flags_verbose_beats_tele_log_env() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let original = MIN_LINE.load(Ordering::Relaxed);
        let original_max = log::max_level();

        std::env::set_var("TELE_LOG", "off");
        set_flags(2, false);
        assert_eq!(log::max_level(), LevelFilter::Debug);
        std::env::remove_var("TELE_LOG");

        MIN_LINE.store(original, Ordering::Relaxed);
        log::set_max_level(original_max);
    }
}
