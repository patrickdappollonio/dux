//! Front-end-specific configuration persistence.
//!
//! The `Engine` does not itself know how to render the `[keys]` section of
//! the user config (rendering depends on `dux_tui::keybindings::RuntimeBindings`,
//! a TUI-only type) and does not own the project-sync helpers
//! (`sync_config_projects_with_store`, `load_projects`,
//! `persist_runtime_projects_to_config_and_store`). It delegates those
//! responsibilities to a front-end via this trait.
//!
//! The TUI provides `dux_tui::TuiConfigSaver`. A future web layer
//! (`dux-web`) provides its own implementation (or a no-op variant if web
//! sessions don't persist config the same way).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::config::{Config, DuxPaths};
use crate::worker::WorkerEvent;

/// Front-end-specific configuration persistence. Methods are expected to
/// spawn their own worker threads and send a corresponding `WorkerEvent`
/// when they complete (`GlobalEnvPersistenceCompleted`,
/// `ConfigReloadReady`, `ConfigRecoverCompleted`).
pub trait ConfigSaver: Send + Sync {
    /// Persist the global `env` block to the user config file. The
    /// implementation should derive whatever frontend-specific state it
    /// needs (e.g., keybindings) from `config`, then write to
    /// `config_path` and post `WorkerEvent::GlobalEnvPersistenceCompleted`.
    fn persist_global_env(
        &self,
        env: BTreeMap<String, String>,
        config: Config,
        config_path: PathBuf,
        worker_tx: Sender<WorkerEvent>,
    );

    /// Persist the `[macros]` block to the user config file. The engine has
    /// already adopted the new macros into `config`; the implementation writes
    /// `config` to `config_path` (preserving comments via the in-place patch)
    /// and posts `WorkerEvent::MacrosPersistenceCompleted`. Mirrors
    /// [`ConfigSaver::persist_global_env`].
    fn persist_macros(&self, config: Config, config_path: PathBuf, worker_tx: Sender<WorkerEvent>);

    /// Reload the user config from disk, validate it, and re-sync project
    /// records against the session store. Post `WorkerEvent::ConfigReloadReady`
    /// when done.
    fn reload_config(&self, paths: DuxPaths, worker_tx: Sender<WorkerEvent>);

    /// Write a canonical (fully-templated) config to `config_path`,
    /// overwriting the existing file. Used for recovery when the on-disk
    /// config is corrupted. Post `WorkerEvent::ConfigRecoverCompleted` when done.
    fn recover_config(&self, config_path: PathBuf, config: Config, worker_tx: Sender<WorkerEvent>);
}

/// A no-op implementation for tests that need to construct an `Engine`
/// without a real front-end attached. All methods immediately post a
/// success `WorkerEvent` so any caller draining the worker channel can
/// observe completion.
#[doc(hidden)]
pub struct NoopConfigSaver;

impl ConfigSaver for NoopConfigSaver {
    fn persist_global_env(
        &self,
        env: BTreeMap<String, String>,
        _config: Config,
        _config_path: PathBuf,
        worker_tx: Sender<WorkerEvent>,
    ) {
        let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted {
            env,
            result: Ok(()),
        });
    }

    fn persist_macros(
        &self,
        config: Config,
        _config_path: PathBuf,
        worker_tx: Sender<WorkerEvent>,
    ) {
        let _ = worker_tx.send(WorkerEvent::MacrosPersistenceCompleted {
            macros: config.macros,
            result: Ok(()),
        });
    }

    fn reload_config(&self, _paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
        let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(Ok(
            Config::default(),
        ))));
    }

    fn recover_config(
        &self,
        _config_path: PathBuf,
        _config: Config,
        worker_tx: Sender<WorkerEvent>,
    ) {
        let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(Ok(())));
    }
}
