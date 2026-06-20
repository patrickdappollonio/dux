//! Front-end-specific configuration surface.
//!
//! The `Engine` owns the config *write* path (the off-thread, atomic
//! `ConfigWriteQueue`), but two config concerns still depend on front-end-only
//! knowledge and stay behind this seam:
//!
//! - **reload**: re-reading + validating config and re-syncing project records
//!   against the session store. The TUI validates `[keys]` (which needs the
//!   TUI-only `RuntimeBindings`) and runs the project-sync helpers; the web does
//!   a plain read-only load.
//! - **recover_render**: rendering the full config text to write back when the
//!   on-disk file is corrupt. The TUI produces a fully-commented canonical
//!   render (needs `RuntimeBindings`); the web produces a plain serialization.
//!
//! The TUI provides `dux_tui::TuiConfigSurface`; the web provides
//! `dux_web::WebConfigSurface`. Tests use [`NoopConfigSurface`].

use std::sync::mpsc::Sender;

use crate::config::{Config, DuxPaths};
use crate::worker::WorkerEvent;

/// Guarantees that a reload worker ALWAYS posts exactly one
/// `WorkerEvent::ConfigReloadReady`, even if the worker panics or returns early
/// before producing a result.
///
/// A reload opens a barrier on the engine (quiesces the config writer and defers
/// config-mutating commands); the barrier only closes when `ConfigReloadReady`
/// lands. If a panicking reload worker never posted a completion, the writer
/// would stay paused and saves would be frozen forever (F5). Every
/// [`ConfigSurface::reload`] implementation must drive its completion through
/// this guard: call [`ReloadCompletionGuard::complete`] with the real result on
/// the success/error path, and the guard's `Drop` posts an `Err` completion if
/// `complete` was never reached (e.g. a panic unwound past it).
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
        // Only fires when `complete` was never called (early return / panic):
        // post a failure completion so the engine closes the reload barrier
        // (resume the writer, clear `reloading`, drain deferred) rather than
        // freezing saves forever.
        self.send(Err(
            "the config reload worker stopped before producing a result".to_string(),
        ));
    }
}

/// Front-end-specific configuration surface. [`ConfigSurface::reload`] spawns
/// its own worker thread and posts `WorkerEvent::ConfigReloadReady` when done;
/// [`ConfigSurface::recover_render`] is a pure function that returns the config
/// text to write (the Engine performs the actual write through its writer).
pub trait ConfigSurface: Send + Sync {
    /// Reload the user config from disk, validate it, and re-sync project
    /// records against the session store. Post `WorkerEvent::ConfigReloadReady`
    /// when done. Runs on its own worker thread.
    fn reload(&self, paths: DuxPaths, worker_tx: Sender<WorkerEvent>);

    /// Render the full config file text for `config`. Used by the Engine's
    /// `RecoverConfig` handler to overwrite a corrupt on-disk config. This does
    /// NOT write or post an event — it only produces the bytes; the Engine
    /// writes them through `config_write::write_config_secure`.
    fn recover_render(&self, config: &Config) -> String;
}

/// A no-op implementation for tests that need to construct an `Engine`
/// without a real front-end attached. `reload` immediately posts a success
/// `WorkerEvent` so any caller draining the worker channel can observe
/// completion; `recover_render` returns a plain serialization.
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
