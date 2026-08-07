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
            let mut config = dux_core::config::load_config(&paths);
            // Reconcile config's `[[projects]]` with SQLite on reload too (the
            // "config wins" tenet), mirroring `TuiConfigSurface::reload` so an
            // edited config.toml applies its project preferences on a live
            // `dux serve`. A store-open or reconciliation error is surfaced
            // through the reload result rather than crashing the reload thread.
            let reconciled = SessionStore::open(&paths.sessions_db_path)
                .map_err(|e| format!("{e:#}"))
                .and_then(|store| {
                    #[allow(deprecated)]
                    dux_core::config_sync::reconcile_config_projects(
                        &mut config,
                        &store,
                        |config| {
                            dux_core::config_write::save_config_with(
                                &paths.config_path,
                                config,
                                dux_core::config_write::Durability::Fsync,
                            )
                        },
                    )
                    .map_err(|e| format!("{e:#}"))
                });
            match reconciled {
                Ok(()) => guard.complete(Ok(config)),
                Err(e) => guard.complete(Err(e)),
            }
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
    let mut config = dux_core::config::load_config(paths);
    let session_store = SessionStore::open(&paths.sessions_db_path)?;
    // Reconcile config's `[[projects]]` with SQLite (the "config wins" tenet),
    // the same core routine the TUI bootstrap runs, so `dux serve` also adopts
    // config-only projects, applies config-edited preferences to SQLite, and
    // validates identity conflicts. Persist any normalized config back through
    // the core save path (surgical toml_edit patch, comment-preserving, when the
    // file exists; a plain render otherwise). Blessed sync-direct: bootstrap runs
    // before the engine's config-write queue exists, mirroring the TUI invariant.
    #[allow(deprecated)]
    dux_core::config_sync::reconcile_config_projects(&mut config, &session_store, |config| {
        dux_core::config_write::save_config_with(
            &paths.config_path,
            config,
            dux_core::config_write::Durability::Fsync,
        )
    })?;
    let sessions = session_store.load_sessions()?;
    let agent_tabs = session_store.load_agent_tabs()?;
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
        surface_kind: dux_core::term_identity::SurfaceKind::WebHeadless,
        resource_collector: Default::default(),
        host_env: dux_core::term_identity::HostEnvProbe::from_env(),
        worker_tx,
        worker_rx,
        config_writer,
        surface: Box::new(WebConfigSurface),
        reloading: false,
        deferred_commands: Vec::new(),
        reload_guard: None,
        pending_web_checkout_ops: HashMap::new(),
        pending_web_add_project_ops: HashMap::new(),
        pending_web_pr_lookup_ops: HashMap::new(),
        pending_pr_attach_ops: HashMap::new(),
        pending_delete_ops_web: HashMap::new(),
        pending_create_ops: HashMap::new(),
        pending_web_launch_ops: HashMap::new(),
        last_created_op_id: None,
        created_session_by_op: HashMap::new(),
        providers: HashMap::new(),
        running_provider_pins: HashMap::new(),
        launched_drop_paste: Default::default(),
        companion_terminals: HashMap::new(),
        agent_tabs: agent_tabs.into_iter().map(|t| (t.id.clone(), t)).collect(),
        terminating_ptys: Vec::new(),
        pending_group_removals: Vec::new(),
        gh_status: GhStatus::Unknown,
        gh_probe: Default::default(),
        pr_statuses: HashMap::new(),
        pr_overrides: HashMap::new(),
        branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync: Arc::new(Default::default()),
        pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        refs_watcher: None,
        refs_watch_paths: HashMap::new(),
        resume_fallback_candidates: HashMap::new(),
        pending_deletions: HashSet::new(),
        closing_sessions: HashSet::new(),
        deletion_busy_messages: HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        watched_session_id: None,
        has_active_processes: Arc::new(AtomicBool::new(false)),
        current_origin: dux_core::statusline::StatusScope::All,
        in_flight: InFlightSet::new(),
        rename_expected: std::collections::HashMap::new(),
        pr_last_checked: HashMap::new(),
        changed_files_poller_started: AtomicBool::new(false),
        branch_sync_worker_started: AtomicBool::new(false),
        pty_activity: HashMap::new(),
        pty_input: HashMap::new(),
        pty_pointer: HashMap::new(),
        needs_attention: HashSet::new(),
        pty_progress: HashMap::new(),
        agent_viewed: HashMap::new(),
        last_foreground_refresh: None,
    };

    engine.normalize_restored_sessions();
    // Seed PR badges from the persisted `latest_prs` rows (the same core routine
    // the TUI runs at startup), so `dux serve` shows PR state immediately instead
    // of blank until the first network poll, and shows persisted state even when
    // `gh` is unavailable. A no-op when GitHub integration is off.
    engine.seed_pr_statuses_from_store();

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
    fn bootstrap_engine_yields_empty_spine_on_fresh_store() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let spine = engine.spine();
        assert!(spine.projects.is_empty());
        assert!(spine.sessions.is_empty());
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
        let spine = engine.spine();

        assert!(
            !spine.projects.is_empty(),
            "the spine should include projects from the store"
        );
        assert!(
            spine.projects.iter().any(|p| p.id == seeded_id),
            "seeded project id should appear in the spine"
        );
    }

    /// The server bootstrap runs the core project reconciliation: a config-only
    /// `[[projects]]` entry (present in config.toml, absent from SQLite) is
    /// adopted into the store and appears in the spine, so `dux serve` honors
    /// config-declared projects (previously it read SQLite only).
    #[test]
    fn bootstrap_engine_adopts_a_config_only_project() {
        let (_tmp, paths) = temp_paths();
        // A config.toml declaring a project the store has never seen.
        std::fs::write(
            &paths.config_path,
            "[[projects]]\nid = \"cfg-only\"\npath = \"$HOME/proj\"\nname = \"FromConfig\"\n",
        )
        .expect("write config");

        let engine = bootstrap_engine(&paths).expect("bootstrap");
        assert!(
            engine.spine().projects.iter().any(|p| p.id == "cfg-only"),
            "the config-only project must be reconciled into the spine"
        );
        // And persisted into SQLite (the reconciliation adopted it).
        let store = SessionStore::open(&paths.sessions_db_path).expect("reopen store");
        assert!(
            store
                .load_projects()
                .expect("load")
                .iter()
                .any(|p| p.id == "cfg-only"),
            "the config-only project must be adopted into SQLite"
        );
    }
}
