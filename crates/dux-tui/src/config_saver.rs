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
            // load/validate/sync work below panics; otherwise the engine's
            // reload barrier would never close and config saves would freeze.
            let guard = ReloadCompletionGuard::new(worker_tx);
            // A file deleted while dux runs is refused rather than recreated
            // from defaults, which would drop the running password.
            let result = dux_core::config::config_present_for_reload(&paths)
                .map_err(|err| format!("{err}"))
                .and_then(|()| {
                    crate::config::ensure_config(&paths).map_err(|err| format!("{err:#}"))
                })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// config.toml is a symlink whose target vanished while dux runs: the
    /// terminal UI's reload is refused like a deleted file, the running
    /// config (password included) stays, and nothing is written over the
    /// symlink.
    #[test]
    fn a_reload_through_a_dangling_symlink_is_refused_and_keeps_the_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().unwrap();
        let target = root.join("dotfiles-config.toml");
        let hash = dux_core::auth::hash_password(&dux_core::auth::Password::new(
            "correct horse battery staple".to_string(),
        ))
        .unwrap();
        std::fs::write(
            &target,
            format!("[server.auth]\npassword_hash = \"{hash}\"\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, &paths.config_path).unwrap();
        let running = crate::config::ensure_config(&paths).expect("starts");
        assert!(running.server.auth.has_password());

        std::fs::remove_file(&target).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        TuiConfigSurface.reload(paths.clone(), tx);
        let result = loop {
            match rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("reload ends")
            {
                WorkerEvent::ConfigReloadReady(result) => break *result,
                _ => continue,
            }
        };
        let message = result.expect_err("refused");
        assert!(message.contains(&target.display().to_string()), "{message}");
        assert!(
            std::fs::symlink_metadata(&paths.config_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!target.exists(), "nothing was created at the target");
    }
}
