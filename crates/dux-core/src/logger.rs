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
    if let Ok(file) = open_log_file(&path) {
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

/// Open the log for appending, creating it if absent, and restrict it to its
/// owner. The log records the user's project paths, agent names, and error text,
/// so it gets the same treatment as the rest of the config directory; see
/// [`crate::file_modes`] for why the directory's own mode is what really settles
/// this. Tightening runs on every open, so a log left `0644` by an older
/// installation is corrected rather than left as it was.
///
/// The tightening is BEST EFFORT and its failure is not this function's
/// failure. `logging.path` accepts any absolute path, so a log under `/var/log`
/// owned by an admin, on a Windows mount under WSL2, or on a FAT or NFS volume
/// is a path dux can append to but cannot `chmod`. Propagating that error was
/// swallowed whole by `init`'s `if let Ok(file)`, so the outcome was no logging
/// at all and no message anywhere, from a configuration change alone. A
/// slightly loose log is better than no log. This now matches what
/// [`crate::storage`] has always done deliberately for the database.
///
/// Separate from [`init`] because `init` installs a process-global logger and a
/// panic hook, neither of which a test can do twice; this is the part with
/// on-disk behaviour worth pinning.
fn open_log_file(path: &PathBuf) -> std::io::Result<std::fs::File> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // A warning about the log file itself may have nowhere to go, since the
    // logger this warns through is installed by `init` just after this returns.
    // That is inherent, and it is still better than the silent no-op it
    // replaces: the mode is applied, or dux keeps logging.
    crate::file_modes::restrict_to_owner_best_effort(path, "log file");
    Ok(file)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// `dux.log` records paths, project names, and error text from the user's
    /// own work, so it gets the same owner-only treatment as the rest of the
    /// config directory. `init` installs a process-global logger and so cannot
    /// be called from a test; the open is the seam.
    #[test]
    fn open_log_file_leaves_it_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        drop(open_log_file(&path).unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "expected owner-only, got {mode:o}");
    }

    #[test]
    fn open_log_file_tightens_a_log_left_world_readable_by_an_older_install() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_log_file(&path).unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    /// A log dux can append to but cannot tighten must still be OPENED.
    /// `open_log_file`'s error was swallowed whole by `init`'s
    /// `if let Ok(file)`, so a `logging.path` dux could not `chmod` (under
    /// `/var/log`, on a Windows mount under WSL2, on FAT or NFS) meant no
    /// logging at all and no message anywhere. A symlinked log is the case that
    /// can be built here: the tightening is deliberately skipped and the open
    /// must still hand back a usable, appendable file.
    #[test]
    fn open_log_file_still_opens_when_the_mode_cannot_be_applied() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.log");
        fs::write(&target, "old\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let path = dir.path().join("dux.log");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        {
            use std::io::Write;
            let mut file = open_log_file(&path).expect("the log must still open");
            file.write_all(b"new\n").unwrap();
        }

        assert_eq!(fs::read_to_string(&target).unwrap(), "old\nnew\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644,
            "the symlink target's mode must have been left alone"
        );
    }

    #[test]
    fn open_log_file_appends_rather_than_truncating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        fs::write(&path, "old\n").unwrap();
        drop(open_log_file(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
    }
}
