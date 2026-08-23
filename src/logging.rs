//! Minimal `log` backend: prints leveled messages to stderr. The level comes
//! from `RUST_LOG` (error/warn/info/debug/trace/off), defaulting to debug in
//! debug builds and info in release. Release is a no-console GUI, so this output
//! is mainly for `cargo run` / the `--dump-sensors` diagnostic.

use log::{LevelFilter, Metadata, Record};

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{:<5} {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

pub fn init() {
    let level = level_from_env();
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

fn level_from_env() -> LevelFilter {
    let default = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    // `LevelFilter: FromStr` is case-insensitive over off/error/warn/info/debug/trace.
    std::env::var("RUST_LOG")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}
