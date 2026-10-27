//! TUI implementation of `dux_core::engine::ConfigSurface`. Owns the two
//! front-end-specific config concerns the engine can't: reloading + validating
//! config (the TUI validates `[keys]` via `RuntimeBindings` and runs the
//! project-sync helpers) and rendering a fully-commented canonical config for
//! recovery. The engine owns the config *write* path (the `ConfigWriteQueue`).

use std::sync::mpsc::Sender;
use std::thread;

use dux_core::config::{Config, DuxPaths};
use dux_core::engine::{ConfigSurface, ReloadCompletionGuard};
use dux_core::worker::{ReloadedConfig, WorkerEvent};

use crate::keybindings::RuntimeBindings;
use crate::storage::SessionStore;

/// The TUI's `ConfigSurface` implementation. Stateless because each operation
/// derives the runtime bindings it needs from the `Config` passed in.
pub(crate) struct TuiConfigSurface;

impl ConfigSurface for TuiConfigSurface {
    /// Its reload (`ensure_config`) refuses what stops the terminal UI. The
    /// flip and the background server both run on this engine, so both judge
    /// a file the terminal UI's way.
    fn start_surface(&self) -> dux_core::config::Surface {
        dux_core::config::Surface::TerminalUi
    }

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
                    crate::config::ensure_config_for_reload(&paths)
                        .map_err(|err| format!("{err:#}"))
                })
                .and_then(
                    |mut config| match crate::config::validate_keys(&config.keys) {
                        Ok(()) => {
                            let bindings = RuntimeBindings::from_keys_config(&config.keys);
                            let store = SessionStore::open_live(&paths.sessions_db_path)
                                .map_err(|err| format!("{err:#}"))?;
                            crate::app::sync_config_projects_with_store(
                                &mut config,
                                &paths,
                                &bindings,
                                &store,
                            )
                            .map_err(|err| format!("{err:#}"))?;
                            // A list that cannot be read or saved leaves
                            // building and saving it to the apply, as before
                            // the worker built one: the sync above may already
                            // have written, so the reload still stands, with
                            // the config as the sync left it.
                            let projects = dux_core::project_browser::reload_project_list(
                                &store, &config,
                            )
                            .and_then(|projects| {
                                let mut saved = config.clone();
                                match crate::app::persist_runtime_projects_to_config_and_store(
                                    &projects, &mut saved, &paths, &bindings, &store,
                                ) {
                                    Ok(()) => {
                                        config = saved;
                                        Some(projects)
                                    }
                                    Err(err) => {
                                        dux_core::logger::warn(&format!(
                                            "the config reload could not save the \
                                                     project list, so it is built when the \
                                                     reload applies: {err:#}"
                                        ));
                                        None
                                    }
                                }
                            });
                            Ok(ReloadedConfig { config, projects })
                        }
                        Err(message) => Err(message),
                    },
                );
            guard.complete_with_projects(result);
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

    /// An editor that has emptied config.toml mid-save: the terminal UI's
    /// reload is refused (the defaults it would read have no password), the
    /// file is left as it is, and a start with the same empty file still
    /// reads as the defaults.
    #[test]
    fn a_reload_of_an_emptied_config_is_refused_but_a_start_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
            root: root.clone(),
        };
        paths.ensure_dirs().unwrap();
        std::fs::write(&paths.config_path, " \n").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        TuiConfigSurface.reload(paths.clone(), tx);
        let result = loop {
            if let WorkerEvent::ConfigReloadReady(result) = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("reload ends")
            {
                break *result;
            }
        };
        let message = result.expect_err("refused");
        assert!(message.contains("is empty"), "{message}");
        assert_eq!(std::fs::read_to_string(&paths.config_path).unwrap(), " \n");

        crate::config::ensure_config(&paths).expect("a start takes an empty file as defaults");
    }

    /// The worker built the list but could not save it back: the reload,
    /// whose sync may already have written, still answers with its config and
    /// leaves the list to the apply.
    #[test]
    fn a_reload_whose_project_list_cannot_be_saved_still_answers_with_its_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
            root: root.clone(),
        };
        paths.ensure_dirs().unwrap();
        // A repository whose leading branch the list detects, so saving the
        // list writes the project's row.
        let repo = root.join("repo");
        let status = dux_core::test_git::fixture_git()
            .args(["init", "-q", "-b", "main"])
            .arg(&repo)
            .status()
            .unwrap();
        assert!(status.success());
        let project = dux_core::config::ProjectConfig {
            id: "p1".to_string(),
            path: repo.display().to_string(),
            name: Some("repo".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
        };
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store.upsert_project(&project).unwrap();
        store.run_sql_for_tests(
            "create trigger refuse_project_updates before update on projects begin \
             select raise(abort, 'refused by a test'); end;",
        );
        drop(store);
        std::fs::write(
            &paths.config_path,
            format!(
                "[[projects]]\nid = \"p1\"\npath = \"{}\"\nname = \"repo\"\n",
                repo.display()
            ),
        )
        .unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        TuiConfigSurface.reload(paths, tx);
        let result = loop {
            if let WorkerEvent::ConfigReloadReady(result) = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("reload ends")
            {
                break *result;
            }
        };
        let reloaded = result.expect("the reload still stands");
        assert!(reloaded.projects.is_none());
        assert!(reloaded.config.projects.iter().any(|p| p.id == "p1"));
    }

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
            socket_path: root.join("dux.sock"),
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
            if let WorkerEvent::ConfigReloadReady(result) = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("reload ends")
            {
                break *result;
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
