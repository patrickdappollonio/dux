//! Shared test fixtures for engine submodule unit tests. Test-only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use chrono::Utc;
use tempfile::TempDir;

use crate::config::{Config, DuxPaths};
use crate::engine::Engine;
use crate::lockfile::SingleInstanceLock;
use crate::model::{
    AgentSession, GhStatus, Project, ProjectBranchStatus, ProviderKind, SessionStatus,
};
use crate::storage::SessionStore;

/// Construct a minimally-wired `Engine` for tests, alongside the `TempDir`
/// that backs its on-disk state (sqlite, lockfile). Keep the `TempDir`
/// alive for the lifetime of the test so it is cleaned up afterwards.
pub(crate) fn test_engine() -> (Engine, TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        root: root.clone(),
    };
    std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
    let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
    let single_instance_lock =
        SingleInstanceLock::acquire(&paths.lock_path).expect("single-instance lock");
    let (worker_tx, worker_rx) = mpsc::channel();
    let config_writer = crate::config_queue::ConfigWriteQueue::new(paths.config_path.clone());
    let engine = Engine {
        config: Config::default(),
        paths,
        session_store,
        projects: Vec::new(),
        sessions: Vec::new(),
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        terminal_counter: 0,
        github_integration_enabled: false,
        single_instance_lock,
        worker_tx,
        worker_rx,
        config_writer,
        surface: Box::new(crate::engine::NoopConfigSurface),
        reloading: false,
        deferred_commands: Vec::new(),
        reload_guard: None,
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
        in_flight: HashSet::new(),
        pr_last_checked: HashMap::new(),
        changed_files_poller_started: AtomicBool::new(false),
        branch_sync_worker_started: AtomicBool::new(false),
        pty_activity: HashMap::new(),
        pty_input: HashMap::new(),
        last_foreground_refresh: None,
    };
    (engine, tmp)
}

pub(crate) fn sample_project(id: &str, path: &str) -> Project {
    Project {
        id: id.to_string(),
        name: format!("{id}-name"),
        path: path.to_string(),
        explicit_default_provider: None,
        default_provider: ProviderKind::new("claude"),
        leading_branch: Some("main".to_string()),
        auto_reopen_agents: None,
        startup_command: None,
        env: BTreeMap::new(),
        current_branch: "main".to_string(),
        branch_status: ProjectBranchStatus::Leading,
        path_missing: false,
        created_at: None,
    }
}

pub(crate) fn sample_session(id: &str, project_id: &str, branch: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        project_path: None,
        provider: ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: branch.to_string(),
        worktree_path: format!("/tmp/{id}-worktree"),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
    }
}
