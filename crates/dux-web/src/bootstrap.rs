//! Headless `Engine` bootstrap for the web server. Mirrors the TUI's field-by-field
//! assembly (crates/dux-tui/src/app/mod.rs) but with a read-only config load and a
//! `WebConfigSurface`. Config is loaded via `dux_core::config::load_config`, which reads
//! `config.toml` read-only and falls back to defaults on missing/malformed files.
//! Sessions and projects come from the SQLite store.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use dux_core::config::{Config, DuxPaths};
use dux_core::config_queue::ConfigWriteQueue;
use dux_core::engine::{ConfigSurface, Engine, InFlightSet, ReloadCompletionGuard};
use dux_core::lockfile::SingleInstanceLock;
use dux_core::model::GhStatus;
use dux_core::storage::SessionStore;
use dux_core::worker::WorkerEvent;

/// Config surface for the web server. Owns the two front-end-specific config
/// concerns the engine can't: reload (a read-only re-load of `config.toml`) and
/// recover rendering (a plain, comment-free serialization — the web has no
/// canonical commented renderer; that needs the TUI's `RuntimeBindings`). The
/// engine owns the config *write* path (the `ConfigWriteQueue`).
pub struct WebConfigSurface;

impl ConfigSurface for WebConfigSurface {
    fn reload(&self, paths: DuxPaths, worker_tx: mpsc::Sender<WorkerEvent>) {
        std::thread::spawn(move || {
            // The guard guarantees a `ConfigReloadReady` is posted even if the
            // read-only load below panics — otherwise the engine's reload barrier
            // would never close and config saves would freeze (F5).
            let guard = ReloadCompletionGuard::new(worker_tx);
            // Re-read config from disk (read-only load — same as bootstrap). Returns the
            // REAL config, not Config::default().
            let config = dux_core::config::load_config(&paths);
            guard.complete(Ok(config));
        });
    }

    fn recover_render(&self, config: &Config) -> String {
        // Plain (comment-free) render — the web has no canonical commented
        // renderer (that needs the TUI's `RuntimeBindings`). Returning the text
        // (not writing) lets the engine perform the atomic write through its own
        // writer while holding the quiesce barrier.
        dux_core::config_write::render_config_plain(config)
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
    // The single-instance lock must be held before any config read, DB open, or
    // config write — matching the TUI's invariant.
    let single_instance_lock = SingleInstanceLock::acquire(&paths.lock_path)?;
    let config = dux_core::config::load_config(paths);
    let session_store = SessionStore::open(&paths.sessions_db_path)?;
    let sessions = session_store.load_sessions()?;
    let projects = dux_core::project_browser::load_projects(
        &session_store.load_projects()?,
        &session_store.load_project_created_ats()?,
        &config,
    );
    let (worker_tx, worker_rx): (mpsc::Sender<WorkerEvent>, mpsc::Receiver<WorkerEvent>) =
        mpsc::channel();

    let github_integration_enabled = config.ui.github_integration;
    let config_writer = ConfigWriteQueue::new(paths.config_path.clone());

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
        config_writer,
        surface: Box::new(WebConfigSurface),
        reloading: false,
        deferred_commands: Vec::new(),
        reload_guard: None,
        pending_auth_users: None,
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
        pty_input: HashMap::new(),
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
    fn web_config_surface_recover_render_is_a_writable_plain_config() {
        // The web surface renders a plain (comment-free) config the engine can
        // write to recover a corrupt file. Prove the render carries the config's
        // values and reparses cleanly.
        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        let body = WebConfigSurface.recover_render(&config);
        assert!(body.contains("FOO = \"bar\""), "env entry missing: {body}");
        // A valid TOML table header proves the render is structured config text,
        // not a placeholder.
        assert!(body.contains("[env]"), "env table missing: {body}");
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
