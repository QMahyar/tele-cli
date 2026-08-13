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
    if std::env::var("TELE_LOG").is_ok() {
        return;
    }
    if quiet {
        log::set_max_level(LevelFilter::Error);
        MIN_LINE.store(LEVEL_ERROR, Ordering::Relaxed);
    } else if verbose > 0 {
        log::set_max_level(if verbose > 1 {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
    }
}

pub fn min_line_level() -> u8 {
    MIN_LINE.load(Ordering::Relaxed)
}
