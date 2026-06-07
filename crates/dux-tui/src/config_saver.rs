//! TUI implementation of `dux_core::engine::ConfigSaver`. Wraps the existing
//! save/reload/recover spawn helpers so the engine can dispatch
//! configuration persistence through its `Command` enum.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use dux_core::config::{Config, DuxPaths};
use dux_core::engine::ConfigSaver;
use dux_core::worker::WorkerEvent;

use crate::keybindings::RuntimeBindings;
use crate::storage::SessionStore;

/// The TUI's `ConfigSaver` implementation. Stateless because each operation
/// derives the runtime bindings it needs from the `Config` passed in.
pub struct TuiConfigSaver;

impl ConfigSaver for TuiConfigSaver {
    fn persist_global_env(
        &self,
        env: BTreeMap<String, String>,
        config: Config,
        config_path: PathBuf,
        worker_tx: Sender<WorkerEvent>,
    ) {
        thread::spawn(move || {
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            let result = crate::config::save_config(&config_path, &config, &bindings)
                .map_err(|err| format!("{err:#}"));
            let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted { env, result });
        });
    }

    fn persist_macros(&self, config: Config, config_path: PathBuf, worker_tx: Sender<WorkerEvent>) {
        thread::spawn(move || {
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            let result = crate::config::save_config(&config_path, &config, &bindings)
                .map_err(|err| format!("{err:#}"));
            let macros = config.macros;
            let _ = worker_tx.send(WorkerEvent::MacrosPersistenceCompleted { macros, result });
        });
    }

    fn reload_config(&self, paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
        thread::spawn(move || {
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
            let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(result)));
        });
    }

    fn recover_config(&self, config_path: PathBuf, config: Config, worker_tx: Sender<WorkerEvent>) {
        thread::spawn(move || {
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            let rendered = crate::config::render_config_with(&config, &bindings);
            let result = std::fs::write(&config_path, rendered)
                .map_err(|err| format!("failed to write {}: {err}", config_path.display()));
            let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(result));
        });
    }
}
