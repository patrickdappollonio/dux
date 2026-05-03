//! Thin facade over the [`tracing`] ecosystem.
//!
//! `init` wires a daily-rotating, JSON-Lines file appender at the configured
//! `dux.log` path. New code should prefer `tracing::{info,warn,error,debug}!`
//! macros directly with structured key-value fields — those produce parseable
//! records and let downstream tooling (Phase 10 GDPR purge, Phase 20 doctor
//! tool) filter on `session_id`, `agent`, etc.
//!
//! The legacy `crate::logger::{info,warn,error,debug}` free functions are
//! retained as back-compat shims so existing call sites compile unchanged.
//! They route through the `tracing` macros under a `dux::legacy` target and
//! sanitize the message via [`crate::sanitize::for_terminal`] before
//! emitting it. **Never** call those shims from inside `crate::sanitize` —
//! the sanitizer is on the same call path and would recurse.
//!
//! ## Lifetime of the non-blocking writer
//!
//! `tracing-appender::non_blocking` returns a [`WorkerGuard`] that flushes
//! buffered writes when dropped. Losing it mid-program drops in-flight log
//! lines, so we stash it in a `OnceLock` for the lifetime of the process.

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder as RollingBuilder, Rotation};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::config::{DuxPaths, LoggingConfig};

/// Holds the [`WorkerGuard`] returned by `tracing_appender::non_blocking`
/// for the lifetime of the program. Dropping the guard would flush and
/// close the background writer, losing any buffered records.
static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Initialize the global tracing subscriber.
///
/// On success, future `tracing::*!` macro invocations (and the back-compat
/// shims below) will write JSON Lines records into a daily-rotated file
/// rooted at `paths.root` (or the configured override). Up to seven days
/// of logs are retained; older files are pruned on rotation.
///
/// Re-entrant: only the first call wins. Subsequent calls are silent
/// no-ops, which is what tests want.
pub fn init(config: &LoggingConfig, paths: &DuxPaths) {
    if GUARD.get().is_some() {
        return;
    }

    let path = resolve_log_path(config, paths);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let file_prefix = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dux.log")
        .to_string();

    let appender = match RollingBuilder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(file_prefix)
        .max_log_files(7)
        .build(&dir)
    {
        Ok(appender) => appender,
        Err(_) => {
            // Without a working appender there is no point installing a
            // subscriber — fall back to a silent no-op so the rest of the
            // program continues to run.
            return;
        }
    };

    let (nb, guard) = tracing_appender::non_blocking(appender);
    let _ = GUARD.set(guard);

    // Config-driven default; `RUST_LOG` overrides it for ad-hoc debugging.
    let level: tracing::Level = config.level.parse().unwrap_or(tracing::Level::INFO);
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("dux={level}")));

    let json_layer = fmt::layer()
        .json()
        .with_writer(nb)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_current_span(true)
        .with_span_list(false);

    // `try_init` so concurrent test binaries don't panic if a sibling
    // already installed a subscriber.
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(json_layer)
        .try_init();

    tracing::info!(
        target: "dux::logger",
        path = %path.display(),
        "logger initialized"
    );
}

/// Back-compat shim: route `crate::logger::warn(msg)` through `tracing::warn!`
/// after sanitization. Prefer `tracing::warn!(target: "...", field = %v, "msg")`
/// in new code.
#[allow(dead_code)] // Retained for symmetry with info/debug/error.
pub fn warn(message: &str) {
    tracing::warn!(
        target: "dux::legacy",
        "{}",
        crate::sanitize::for_terminal(message)
    );
}

/// Back-compat shim: route `crate::logger::info(msg)` through `tracing::info!`.
pub fn info(message: &str) {
    tracing::info!(
        target: "dux::legacy",
        "{}",
        crate::sanitize::for_terminal(message)
    );
}

/// Back-compat shim: route `crate::logger::debug(msg)` through `tracing::debug!`.
pub fn debug(message: &str) {
    tracing::debug!(
        target: "dux::legacy",
        "{}",
        crate::sanitize::for_terminal(message)
    );
}

/// Back-compat shim: route `crate::logger::error(msg)` through `tracing::error!`.
pub fn error(message: &str) {
    tracing::error!(
        target: "dux::legacy",
        "{}",
        crate::sanitize::for_terminal(message)
    );
}

/// Resolve the configured log path against the dux config root.
///
/// An empty `config.path` falls back to `<root>/dux.log`. Relative paths are
/// joined onto `paths.root`; absolute paths are used as-is.
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

    /// The legacy shims must be safe to call before (or without) `init` — many
    /// crate-level call sites fire during early startup, and tests routinely
    /// skip initialization.
    #[test]
    fn legacy_shim_does_not_panic_when_logger_uninit() {
        // No `init()` call. tracing's default subscriber is a no-op, so these
        // must not panic, allocate unbounded, or recurse.
        super::error("uninitialized: error");
        super::warn("uninitialized: warn");
        super::info("uninitialized: info");
        super::debug("uninitialized: debug");
    }

    /// The shim sanitizes its input — embedded ESC bytes must not reach the
    /// underlying writer (defense in depth; the JSON encoder escapes too,
    /// but the sanitizer also rewrites them as `\xNN` for grep-ability).
    #[test]
    fn legacy_shim_sanitizes_control_bytes() {
        // Indirectly verified: just make sure the call doesn't panic with
        // adversarial input. The sanitization itself is unit-tested in
        // `crate::sanitize`.
        super::error("\x1b]0;evil\x07");
        super::warn("\x1b]0;evil\x07");
        super::info("\x1b]0;evil\x07");
        super::debug("\x1b]0;evil\x07");
    }

    fn fake_paths(root: &str) -> DuxPaths {
        let root = PathBuf::from(root);
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        }
    }

    #[test]
    fn resolve_log_path_uses_root_for_empty_config() {
        let paths = fake_paths("/tmp/dux");
        let cfg = LoggingConfig {
            level: "info".into(),
            path: String::new(),
        };
        assert_eq!(
            resolve_log_path(&cfg, &paths),
            PathBuf::from("/tmp/dux/dux.log")
        );
    }

    #[test]
    fn resolve_log_path_joins_relative_path_onto_root() {
        let paths = fake_paths("/tmp/dux");
        let cfg = LoggingConfig {
            level: "info".into(),
            path: "logs/dux.log".into(),
        };
        assert_eq!(
            resolve_log_path(&cfg, &paths),
            PathBuf::from("/tmp/dux/logs/dux.log")
        );
    }

    #[test]
    fn resolve_log_path_respects_absolute_path() {
        let paths = fake_paths("/tmp/dux");
        let cfg = LoggingConfig {
            level: "info".into(),
            path: "/var/log/dux.log".into(),
        };
        assert_eq!(
            resolve_log_path(&cfg, &paths),
            PathBuf::from("/var/log/dux.log")
        );
    }
}
