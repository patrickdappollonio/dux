use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::Utc;

use crate::config::{DuxPaths, LoggingConfig};

static LOGGER: OnceLock<Logger> = OnceLock::new();

struct Logger {
    level: LogLevel,
    file: Mutex<std::fs::File>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    fn from_str(value: &str) -> Self {
        match value {
            "debug" => Self::Debug,
            "error" => Self::Error,
            "warn" => Self::Warn,
            _ => Self::Info,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

pub fn init(config: &LoggingConfig, paths: &DuxPaths) {
    let path = resolve_log_path(config, paths);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let logger = Logger {
            level: LogLevel::from_str(&config.level),
            file: Mutex::new(file),
        };
        let _ = LOGGER.set(logger);
        info(&format!("logger initialized at {}", path.display()));
        install_panic_hook();
    }
}

/// Route every panic through the log file BEFORE the default hook prints it to
/// stderr. Motivated by a real incident: the engine runs on a dedicated OS
/// thread, so a panic there silently killed it (every later request answered
/// "the engine is unavailable") while the only evidence, the panic message,
/// went to a stderr nobody had captured. dux.log now records the thread,
/// message, and location; the previous hook still runs so terminal users and
/// `RUST_BACKTRACE` behavior are unchanged. Installed once, only after the
/// logger has a file to write to.
fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown location>".to_string());
            error(&format!(
                "thread '{name}' panicked at {location}: {message}"
            ));
            previous(info);
        }));
    });
}

#[allow(dead_code)] // Public API for future callers
pub fn warn(message: &str) {
    log(LogLevel::Warn, message);
}

pub fn info(message: &str) {
    log(LogLevel::Info, message);
}

pub fn debug(message: &str) {
    log(LogLevel::Debug, message);
}

pub fn error(message: &str) {
    log(LogLevel::Error, message);
}

fn log(level: LogLevel, message: &str) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    if level > logger.level {
        return;
    }
    let line = format!(
        "{} {:<5} {}\n",
        Utc::now().to_rfc3339(),
        level.as_str(),
        message
    );
    if let Ok(mut file) = logger.file.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

pub fn resolve_log_path(config: &LoggingConfig, paths: &DuxPaths) -> PathBuf {
    let configured = PathBuf::from(&config.path);
    if configured.as_os_str().is_empty() {
        return paths.root.join("dux.log");
    }
    if configured.is_absolute() {
        configured
    } else {
        paths.root.join(configured)
    }
}
