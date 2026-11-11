//! GameMode core: detection, optimisation and exact restore for Windows.
//!
//! The crate is split so each layer is independently testable:
//!
//! * [`win`] - safe wrappers over the Win32/NT APIs.
//! * [`config`] - user settings and TOML persistence.
//! * [`monitor`] - process sampling and game detection.
//! * [`optimizer`] - the ordered list of tuning actions GameMode performs.
//! * [`session`] - orchestrates activation/deactivation and journaling.
//! * [`snapshot`] - durable record of every system change so it can be undone.

#![cfg(windows)]

pub mod config;
pub mod error;
pub mod monitor;
pub mod optimizer;
pub mod prelude;
pub mod session;
pub mod snapshot;
pub mod win;

#[cfg(test)]
pub mod testlock;

pub use config::{BoostLevel, Config, DetectMode, GameEntry, PowerPlan};
pub use error::{GmError, GmResult};
pub use monitor::{GameDetection, Monitor, ProcessSample};
pub use optimizer::{ActionReport, ActionStatus, Optimizer};
pub use session::{Session, SessionState};
pub use snapshot::Snapshot;

/// Human-readable version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Product name used in window titles, registry keys and notifications.
pub const PRODUCT: &str = "GameMode";

/// Initialise logging once for the whole process.
///
/// Writes to `<data dir>/gamemode.log` and, when `verbose`, also to stderr.
/// Falls back silently to stderr-only if the file cannot be created.
pub fn init_logging(verbose: bool) -> Option<std::path::PathBuf> {
    use std::io::Write;
    use std::sync::Mutex;

    /// Minimal thread-safe writer that appends to a file.
    struct FileLogger {
        file: Mutex<Option<std::fs::File>>,
        verbose: bool,
    }

    impl log::Log for FileLogger {
        fn enabled(&self, meta: &log::Metadata) -> bool {
            meta.level() <= log::Level::Debug || self.verbose
        }
        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let line = format!(
                "{} [{:<5}] {}\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.args()
            );
            if self.verbose {
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
            if let Ok(mut guard) = self.file.lock() {
                if let Some(f) = guard.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.flush();
                }
            }
        }
        fn flush(&self) {}
    }

    let log_path = Config::data_dir().join("gamemode.log");
    let _ = Config::ensure_dirs();
    // Keep the previous run's log once, so a crash can still be investigated.
    if log_path.exists() {
        let prev = log_path.with_extension("log.prev");
        let _ = std::fs::remove_file(&prev);
        let _ = std::fs::rename(&log_path, &prev);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let logger = FileLogger {
        file: Mutex::new(file),
        verbose,
    };
    let level = if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(level);
    Some(log_path)
}

/// Is this build running with an elevated token?
pub fn is_elevated() -> bool {
    win::elevation::is_elevated()
}

/// Convenience accessor used by the GUI and CLI.
pub fn data_dir() -> std::path::PathBuf {
    Config::data_dir()
}
