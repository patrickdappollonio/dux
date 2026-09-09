//! Front-end-specific configuration surface.
//!
//! The `Engine` owns the config write path; two concerns need front-end-only
//! knowledge and stay behind this seam:
//!
//! - `reload`: re-read and validate config and re-sync project records against
//!   the session store. The TUI validates `[keys]`, which needs the TUI-only
//!   `RuntimeBindings`; the web does a plain read-only load.
//! - `recover_render`: render the text to write over a corrupt on-disk config.
//!   The TUI renders the commented canonical form, the web a plain
//!   serialization.
//!
//! `dux_tui::TuiConfigSurface` and `dux_web::WebConfigSurface` implement it;
//! tests use [`NoopConfigSurface`].

use std::sync::mpsc::Sender;

use crate::config::{Config, DuxPaths};
use crate::worker::WorkerEvent;

/// Guarantees a reload worker posts exactly one
/// `WorkerEvent::ConfigReloadReady`, even on an early return or a panic.
///
/// A reload opens a barrier on the engine (the config writer quiesces and
/// config-mutating commands defer) that only `ConfigReloadReady` closes, so a
/// missing completion freezes saves for the rest of the process. Every
/// [`ConfigSurface::reload`] implementation must drive its completion through
/// this guard: call [`ReloadCompletionGuard::complete`] with the real result on
/// the success and error paths; `Drop` posts an `Err` if it was never reached.
pub struct ReloadCompletionGuard {
    worker_tx: Sender<WorkerEvent>,
    sent: bool,
}

impl ReloadCompletionGuard {
    /// Wrap the worker's completion channel. Construct this at the very top of a
    /// reload worker so the `Drop` safety net covers the whole worker body.
    pub fn new(worker_tx: Sender<WorkerEvent>) -> Self {
        Self {
            worker_tx,
            sent: false,
        }
    }

    /// Post the real reload result and mark the guard satisfied so `Drop` does
    /// not also post an `Err`. Call this exactly once, on the normal path.
    pub fn complete(mut self, result: Result<Config, String>) {
        self.send(result);
    }

    fn send(&mut self, result: Result<Config, String>) {
        if !self.sent {
            self.sent = true;
            let _ = self
                .worker_tx
                .send(WorkerEvent::ConfigReloadReady(Box::new(result)));
        }
    }
}

impl Drop for ReloadCompletionGuard {
    fn drop(&mut self) {
        // Only fires when `complete` was never called (early return or panic):
        // the engine closes the reload barrier on a completion of either kind.
        self.send(Err(
            "the config reload worker stopped before producing a result".to_string(),
        ));
    }
}

/// Front-end-specific configuration surface. [`ConfigSurface::reload`] spawns
/// its own worker thread and posts `WorkerEvent::ConfigReloadReady` when done;
/// [`ConfigSurface::recover_render`] is pure and the Engine does the writing.
pub trait ConfigSurface: Send + Sync {
    /// Reload the user config from disk, validate it, and re-sync project
    /// records against the session store. Post `WorkerEvent::ConfigReloadReady`
    /// when done. Runs on its own worker thread.
    fn reload(&self, paths: DuxPaths, worker_tx: Sender<WorkerEvent>);

    /// Render the full config file text for `config`. Produces bytes only: the
    /// Engine's `RecoverConfig` handler writes them over a corrupt on-disk
    /// config through `config_write::write_config_secure`.
    fn recover_render(&self, config: &Config) -> String;
}

/// A no-op implementation for tests constructing an `Engine` with no front end.
/// `reload` posts a success `WorkerEvent` immediately so a caller draining the
/// worker channel still observes completion; `recover_render` serializes plainly.
#[doc(hidden)]
pub struct NoopConfigSurface;

impl ConfigSurface for NoopConfigSurface {
    fn reload(&self, _paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
        ReloadCompletionGuard::new(worker_tx).complete(Ok(Config::default()));
    }

    fn recover_render(&self, config: &Config) -> String {
        crate::config_write::render_config_plain(config)
    }
}
