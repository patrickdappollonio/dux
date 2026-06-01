//! Headless `Engine` bootstrap for the web server. Mirrors the TUI's field-by-field
//! assembly (crates/dux-tui/src/app/mod.rs) but with no UI, a default config, and a
//! `WebConfigSaver`. Config-file loading is intentionally skipped (Plan 2 skeleton);
//! sessions come from the SQLite store. Project loading (the `ProjectConfig` -> `Project`
//! converter currently in `dux-tui`) is a documented follow-up; we start with none.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use dux_core::config::{Config, DuxPaths};
use dux_core::engine::{ConfigSaver, Engine, InFlightSet};
use dux_core::lockfile::SingleInstanceLock;
use dux_core::model::GhStatus;
use dux_core::storage::SessionStore;
use dux_core::worker::WorkerEvent;

/// Config saver for the web surface. Plan 2 skeleton: no-op persistence, echoing
/// completion events so the worker pipeline stays uniform. (Copied from the proven
/// smoke-test stub.)
pub struct WebConfigSaver;

impl ConfigSaver for WebConfigSaver {
    fn persist_global_env(
        &self,
        env: BTreeMap<String, String>,
        _config: Config,
        _config_path: PathBuf,
        worker_tx: mpsc::Sender<WorkerEvent>,
    ) {
        let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted {
            env,
            result: Ok(()),
        });
    }

    fn reload_config(&self, _paths: DuxPaths, worker_tx: mpsc::Sender<WorkerEvent>) {
        let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(Ok(
            Config::default(),
        ))));
    }

    fn recover_config(
        &self,
        _config_path: PathBuf,
        _config: Config,
        worker_tx: mpsc::Sender<WorkerEvent>,
    ) {
        let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(Ok(())));
    }
}

/// Assemble a headless `Engine` from `paths`, loading sessions from the store and
/// acquiring the single-instance lock at `paths.lock_path`.
pub fn bootstrap_engine(paths: &DuxPaths) -> Result<Engine> {
    let config = Config::default();
    let session_store = SessionStore::open(&paths.sessions_db_path)?;
    let single_instance_lock = SingleInstanceLock::acquire(&paths.lock_path)?;
    let sessions = session_store.load_sessions()?;
    let (worker_tx, worker_rx): (mpsc::Sender<WorkerEvent>, mpsc::Receiver<WorkerEvent>) =
        mpsc::channel();

    Ok(Engine {
        config,
        paths: paths.clone(),
        session_store,
        projects: Vec::new(),
        sessions,
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        terminal_counter: 0,
        github_integration_enabled: false,
        single_instance_lock,
        worker_tx,
        worker_rx,
        config_saver: Box::new(WebConfigSaver),
        providers: HashMap::new(),
        running_provider_pins: HashMap::new(),
        companion_terminals: HashMap::new(),
        gh_status: GhStatus::Unknown,
        pr_statuses: HashMap::new(),
        branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_enabled: Arc::new(AtomicBool::new(false)),
        refs_watcher: None,
        refs_watch_paths: HashMap::new(),
        resume_fallback_candidates: HashMap::new(),
        pending_deletions: HashSet::new(),
        deletion_busy_messages: HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        has_active_processes: Arc::new(AtomicBool::new(false)),
        in_flight: InFlightSet::new(),
        pr_last_checked: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        (tmp, paths)
    }

    #[test]
    fn bootstrap_engine_yields_empty_view_model_on_fresh_store() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let vm = engine.view_model();
        assert!(vm.projects.is_empty());
        assert!(vm.sessions.is_empty());
    }
}
