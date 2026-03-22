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
    Info,
    Debug,
}

impl LogLevel {
    fn from_str(value: &str) -> Self {
        match value {
            "debug" => Self::Debug,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
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
    }
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
