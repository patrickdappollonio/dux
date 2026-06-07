//! Headless `Engine` bootstrap for the web server. Mirrors the TUI's field-by-field
//! assembly (crates/dux-tui/src/app/mod.rs) but with a read-only config load and a
//! `WebConfigSaver`. Config is loaded via `dux_core::config::load_config`, which reads
//! `config.toml` read-only and falls back to defaults on missing/malformed files.
//! Sessions and projects come from the SQLite store.

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

/// Config saver for the web surface. Persists `config.toml` through the shared
/// `dux_core::config_write` writer (which patches an existing file in place to
/// preserve comments, or writes a plain serialization when none exists), and
/// reloads by re-reading the real on-disk config. Each call runs on its own
/// thread and reports completion through the worker pipeline, mirroring the
/// TUI's `TuiConfigSaver`.
pub struct WebConfigSaver;

impl ConfigSaver for WebConfigSaver {
    fn persist_global_env(
        &self,
        env: BTreeMap<String, String>,
        config: Config,
        config_path: PathBuf,
        worker_tx: mpsc::Sender<WorkerEvent>,
    ) {
        std::thread::spawn(move || {
            // `config` already carries the new env (the engine set it before calling).
            let result = dux_core::config_write::save_config(&config_path, &config)
                .map_err(|err| format!("{err:#}"));
            let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted { env, result });
        });
    }

    fn persist_macros(
        &self,
        config: Config,
        config_path: PathBuf,
        worker_tx: mpsc::Sender<WorkerEvent>,
    ) {
        std::thread::spawn(move || {
            // `config` already carries the new macros (the engine set it before calling).
            let result = dux_core::config_write::save_config(&config_path, &config)
                .map_err(|err| format!("{err:#}"));
            let macros = config.macros;
            let _ = worker_tx.send(WorkerEvent::MacrosPersistenceCompleted { macros, result });
        });
    }

    fn reload_config(&self, paths: DuxPaths, worker_tx: mpsc::Sender<WorkerEvent>) {
        std::thread::spawn(move || {
            // Re-read config from disk (read-only load — same as bootstrap). Returns the
            // REAL config, not Config::default(). (Applying it to the running engine is a
            // later slice; this at least surfaces the true on-disk config.)
            let config = dux_core::config::load_config(&paths);
            let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(Ok(config))));
        });
    }

    fn recover_config(
        &self,
        config_path: PathBuf,
        config: Config,
        worker_tx: mpsc::Sender<WorkerEvent>,
    ) {
        std::thread::spawn(move || {
            // Recover must overwrite unconditionally, even when the on-disk file is
            // corrupt/unparseable. The web has no canonical commented renderer (that
            // needs the TUI's RuntimeBindings); write a valid plain serialization via
            // the shared force-plain writer, which never patches an existing file.
            let result = dux_core::config_write::write_config_plain(&config_path, &config)
                .map_err(|err| format!("failed to write {}: {err}", config_path.display()));
            let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(result));
        });
    }
}

/// Assemble a headless `Engine` from `paths`, loading sessions from the store and
/// acquiring the single-instance lock at `paths.lock_path`. Config is loaded
/// read-only from `config.toml` via `load_config` — no file creation, migration,
/// or write-back occurs here. Persisted session statuses are normalized before
/// returning (the headless counterpart of the TUI's `restore_sessions`): nothing
/// is running yet, so a session whose worktree still exists is `Detached` and one
/// whose worktree vanished is `Exited`.
pub fn bootstrap_engine(paths: &DuxPaths) -> Result<Engine> {
    let config = dux_core::config::load_config(paths);
    let session_store = SessionStore::open(&paths.sessions_db_path)?;
    let single_instance_lock = SingleInstanceLock::acquire(&paths.lock_path)?;
    let sessions = session_store.load_sessions()?;
    let projects = dux_core::project_browser::load_projects(
        &session_store.load_projects()?,
        &session_store.load_project_created_ats()?,
        &config,
    );
    let (worker_tx, worker_rx): (mpsc::Sender<WorkerEvent>, mpsc::Receiver<WorkerEvent>) =
        mpsc::channel();

    let github_integration_enabled = config.ui.github_integration;

    let mut engine = Engine {
        config,
        paths: paths.clone(),
        session_store,
        projects,
        sessions,
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        terminal_counter: 0,
        github_integration_enabled,
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
        watched_session_id: None,
        has_active_processes: Arc::new(AtomicBool::new(false)),
        in_flight: InFlightSet::new(),
        pr_last_checked: HashMap::new(),
        changed_files_poller_started: AtomicBool::new(false),
        branch_sync_worker_started: AtomicBool::new(false),
        pty_activity: HashMap::new(),
        last_foreground_refresh: None,
    };

    engine.normalize_restored_sessions();

    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dux_core::config::ProjectConfig;

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
    fn web_config_saver_persists_global_env() {
        use std::time::Duration;

        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        // Existing file with a user comment so the in-place patch path runs and
        // comment preservation is meaningful.
        std::fs::write(&config_path, "# user comment\n").expect("seed config");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        let (tx, rx) = mpsc::channel();
        WebConfigSaver.persist_global_env(config.env.clone(), config, config_path.clone(), tx);

        let event = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("completion event");
        match event {
            WorkerEvent::GlobalEnvPersistenceCompleted { result, .. } => {
                assert!(result.is_ok(), "persist failed: {result:?}");
            }
            _ => panic!("expected GlobalEnvPersistenceCompleted event"),
        }

        let written = std::fs::read_to_string(&config_path).expect("read back config");
        assert!(written.contains("FOO"), "env key missing: {written}");
        assert!(written.contains("bar"), "env value missing: {written}");
        assert!(
            written.contains("# user comment"),
            "user comment should survive in-place patch: {written}"
        );
    }

    #[test]
    fn bootstrap_engine_yields_empty_view_model_on_fresh_store() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let vm = engine.view_model();
        assert!(vm.projects.is_empty());
        assert!(vm.sessions.is_empty());
    }

    #[test]
    fn bootstrap_engine_includes_projects_from_store() {
        let (_tmp, paths) = temp_paths();
        let seeded_id = "web-bootstrap-test-project".to_string();

        // Seed one project into the store before bootstrapping.
        let store = SessionStore::open(&paths.sessions_db_path).expect("open store");
        store
            .upsert_project(&ProjectConfig {
                id: seeded_id.clone(),
                path: "/nonexistent/path/for/test".to_string(),
                name: Some("test-project".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("upsert project");
        drop(store);

        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let vm = engine.view_model();

        assert!(
            !vm.projects.is_empty(),
            "ViewModel should include projects from the store"
        );
        assert!(
            vm.projects.iter().any(|p| p.id == seeded_id),
            "seeded project id should appear in the ViewModel"
        );
    }
}
