//! A minimal stderr logger. Debug builds log everything; release builds log warnings and errors
//! only. Nothing in this shell logs message content, ids, tokens or keys.

use log::{LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{} {}] {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

/// Installs the logger. Calling it twice is harmless.
pub fn init() {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}
