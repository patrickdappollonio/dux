//! TUI implementation of `dux_core::engine::ConfigSurface`. Owns the two
//! front-end-specific config concerns the engine can't: reloading + validating
//! config (the TUI validates `[keys]` via `RuntimeBindings` and runs the
//! project-sync helpers) and rendering a fully-commented canonical config for
//! recovery. The engine owns the config *write* path (the `ConfigWriteQueue`).

use std::sync::mpsc::Sender;
use std::thread;

use dux_core::config::{Config, DuxPaths};
use dux_core::engine::{ConfigSurface, ReloadCompletionGuard};
use dux_core::worker::WorkerEvent;

use crate::keybindings::RuntimeBindings;
use crate::storage::SessionStore;

/// The TUI's `ConfigSurface` implementation. Stateless because each operation
/// derives the runtime bindings it needs from the `Config` passed in.
pub struct TuiConfigSurface;

impl ConfigSurface for TuiConfigSurface {
    fn reload(&self, paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
        thread::spawn(move || {
            // The guard guarantees a `ConfigReloadReady` is posted even if the
            // load/validate/sync work below panics — otherwise the engine's
            // reload barrier would never close and config saves would freeze (F5).
            let guard = ReloadCompletionGuard::new(worker_tx);
            let result = crate::config::ensure_config(&paths)
                .map_err(|err| format!("{err:#}"))
                .and_then(
                    |mut config| match crate::config::validate_keys(&config.keys) {
                        Ok(()) => {
                            let bindings = RuntimeBindings::from_keys_config(&config.keys);
                            let store = SessionStore::open(&paths.sessions_db_path)
                                .map_err(|err| format!("{err:#}"))?;
                            crate::app::sync_config_projects_with_store(
                                &mut config,
                                &paths,
                                &bindings,
                                &store,
                            )
                            .map_err(|err| format!("{err:#}"))?;
                            let projects = crate::app::load_projects(
                                &store.load_projects().map_err(|err| format!("{err:#}"))?,
                                &store
                                    .load_project_created_ats()
                                    .map_err(|err| format!("{err:#}"))?,
                                &config,
                            );
                            crate::app::persist_runtime_projects_to_config_and_store(
                                &projects,
                                &mut config,
                                &paths,
                                &bindings,
                                &store,
                            )
                            .map_err(|err| format!("{err:#}"))?;
                            Ok(config)
                        }
                        Err(message) => Err(message),
                    },
                );
            guard.complete(result);
        });
    }

    fn recover_render(&self, config: &Config) -> String {
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        crate::config::render_config_with(config, &bindings)
    }
}
