# Phase 09: `tracing` migration — structured logs, rotation, JSON layer

> Maps to: **P1-X** (replace handwritten `logger.rs`).

## Goal
Replace the manual `Mutex<File>` in `src/logger.rs` with the `tracing`
crate + `tracing-subscriber` + `tracing-appender::rolling`. Unblocks
Phase 10 (GDPR purge needs structured `session_id` fields), Phase 20
(doctor tool reads structured fields), and gives free OpenTelemetry
export when needed.

## Pre-conditions
- Phase 00 baseline green.
- Phase 03 (sanitizer) merged — `tracing` field formatters will call
  the sanitizer in their custom layer.

## Files to touch
- `Cargo.toml` — add deps.
- `src/logger.rs` — gut and rewrite as a thin re-export over `tracing`.
- `src/main.rs` — call `logger::init` early (before any `info!`).
- All existing `crate::logger::{info,warn,error,debug}` call sites — keep
  the same API surface (one-line wrapper macros) so this is a drop-in.
- `src/sanitize.rs` — extend with a `tracing_subscriber::fmt` field
  formatter that runs `for_terminal` on field values.

## Steps

### 9.1 — Cargo deps
```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter", "fmt"] }
tracing-appender = "0.2"
```
Verify versions against current latest at `crates.io/crates/tracing`.

### 9.2 — Rewrite `src/logger.rs`
```rust
//! Thin facade over `tracing` so existing call sites can keep using
//! `crate::logger::{info,warn,error,debug}`. New code should prefer
//! `tracing::{info,warn,error,debug}!` directly with structured fields.

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::config::{DuxPaths, LoggingConfig};

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn init(config: &LoggingConfig, paths: &DuxPaths) {
    let path = resolve_log_path(config, paths);
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_prefix = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dux.log");

    // Daily rotation; max 7 files retained.
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(file_prefix)
        .max_log_files(7)
        .build(dir)
        .expect("init log appender");
    let (nb, guard) = tracing_appender::non_blocking(file_appender);
    let _ = GUARD.set(guard);

    let level = config.level.parse::<tracing::Level>().unwrap_or(tracing::Level::INFO);
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

    tracing_subscriber::registry()
        .with(env_filter)
        .with(json_layer)
        .init();

    tracing::info!(target: "dux::logger", path = %path.display(), "logger initialized");
}

// Back-compat shims — DEPRECATED for new code.
pub fn warn(msg: &str)  { tracing::warn!(target: "dux::legacy", "{}", crate::sanitize::for_terminal(msg)); }
pub fn info(msg: &str)  { tracing::info!(target: "dux::legacy", "{}", crate::sanitize::for_terminal(msg)); }
pub fn debug(msg: &str) { tracing::debug!(target: "dux::legacy", "{}", crate::sanitize::for_terminal(msg)); }
pub fn error(msg: &str) { tracing::error!(target: "dux::legacy", "{}", crate::sanitize::for_terminal(msg)); }

pub fn resolve_log_path(config: &LoggingConfig, paths: &DuxPaths) -> PathBuf {
    let configured = PathBuf::from(&config.path);
    if configured.as_os_str().is_empty() {
        return paths.root.join("dux.log");
    }
    if configured.is_absolute() { configured } else { paths.root.join(configured) }
}
```

### 9.3 — Encourage structured fields in new code
Replace examples like:
```rust
crate::logger::error(&format!("Failed to spawn agent {}: {}", name, err));
```
with:
```rust
tracing::error!(target: "dux::workers", agent = %name, err = %err, "spawn failed");
```
This is what enables Phase 10's `session_id`-scoped purge of past log
lines and Phase 20's doctor field extraction. Migrate hot paths
(workers.rs, sessions.rs, pty.rs) first; back-compat shims keep
everything else working.

### 9.4 — Sanitizer-aware field formatter (optional but recommended)
If JSON output goes to a place where escape bytes still matter (someone
runs `cat dux.log.json` directly), wrap the writer:
```rust
struct SanitizingWriter<W: std::io::Write> { inner: W }
impl<W: std::io::Write> std::io::Write for SanitizingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let cleaned = crate::sanitize::for_terminal(&s);
        self.inner.write_all(cleaned.as_bytes())?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}
```
JSON-encoded strings already escape control bytes, so this is mostly
defense-in-depth.

### 9.5 — Tests
Inline:
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn legacy_shim_does_not_panic_when_logger_uninit() {
        // No init() called — should silently no-op (tracing handles this).
        super::error("test");
    }
}
```

### 9.6 — Document in CLAUDE.md / README
Add a "Logging" section under "Recommendations For Editing" (CLAUDE.md):
> New code should prefer `tracing::{info,warn,error,debug}!` with
> structured key-value fields. The legacy `crate::logger::*` shims are
> retained for back-compat only.

## Validation
- `cargo test` green.
- `cargo clippy --all-targets -- -D warnings` green.
- Manual: launch dux; tail `~/.dux/dux.log.<date>`; observe JSON
  records with `target`, `level`, `timestamp`, `fields`.
- Wait > 1 day on a long-running session; observe rotation to
  `dux.log.<yesterday>` + new file for today.

## Acceptance criteria
- [x] `Cargo.toml` adds `tracing`, `tracing-subscriber` (json+env-filter+fmt), `tracing-appender`.
- [x] `logger.rs` rewritten over `tracing` with daily rotation + 7-file
      retention.
- [x] All existing `crate::logger::*` call sites still compile (back-compat shims).
- [x] At least 5 hot-path call sites migrated to structured `tracing!`
      macros (workers.rs / sessions.rs / pty.rs / git.rs / app/mod.rs).
- [x] `dux.log` is now JSON Lines (`tests/logger_jsonlines.rs` proves it).
- [x] CLAUDE.md updated with the logging guidance.
- [x] PR: `feat(observability): tracing + JSON + rotation (P1-X)` — landed via PR #2.

## Known pitfalls
- `tracing_appender::rolling::Builder` API changed across 0.2.x; pin
  to a specific patch version.
- `EnvFilter::try_from_default_env` reads `RUST_LOG`; verify dux's
  existing config-driven `log_level` path still wins. Prefer config →
  build the filter expression directly.
- `WorkerGuard` from `non_blocking` MUST live for the program lifetime
  or you'll lose buffered writes on shutdown. The `OnceLock<WorkerGuard>`
  pattern is correct.
- Don't change file extension to `.json`; downstream tooling (greps,
  doctor scripts) expects `dux.log`. JSON is implicit per-line.
- Existing `eprintln!`-style output in `cli.rs` for `dux config diff`
  etc. is user-facing TTY output — leave alone, do not redirect through
  `tracing`.

## References
- audit02 P1-X.
- `tracing` docs: https://docs.rs/tracing/
- `tracing-appender` rolling: https://docs.rs/tracing-appender/
- OpenTelemetry Rust: opentelemetry.io/docs/languages/rust/
