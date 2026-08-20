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
    AgentSession, AgentTab, GhStatus, Project, ProjectBranchStatus, ProviderKind, SessionStatus,
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
        surface_kind: crate::term_identity::SurfaceKind::Tui,
        resource_collector: Default::default(),
        host_env: crate::term_identity::HostEnvProbe::default(),
        worker_tx,
        worker_rx,
        config_writer,
        surface: Box::new(crate::engine::NoopConfigSurface),
        reloading: false,
        deferred_commands: Vec::new(),
        reload_guard: None,
        providers: HashMap::new(),
        running_provider_pins: HashMap::new(),
        launched_drop_paste: HashMap::new(),
        companion_terminals: HashMap::new(),
        agent_tabs: HashMap::new(),
        terminating_ptys: Vec::new(),
        pending_group_removals: Vec::new(),
        gh_status: GhStatus::Unknown,
        force_worker_spawn_failure: false,
        force_loop_worker_spawn_failure: AtomicBool::new(false),
        gh_probe: Default::default(),
        pr_statuses: HashMap::new(),
        pr_overrides: HashMap::new(),
        pr_suppressions: HashSet::new(),
        branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync: Arc::new(Default::default()),
        pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        refs_watcher: None,
        refs_watch_paths: HashMap::new(),
        resume_fallback_candidates: HashMap::new(),
        pending_deletions: HashSet::new(),
        folder_repo_statuses: HashMap::new(),
        closing_sessions: HashSet::new(),
        deletion_busy_messages: HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        watched_session_id: None,
        has_active_processes: Arc::new(AtomicBool::new(false)),
        current_origin: crate::statusline::StatusScope::All,
        in_flight: HashSet::new(),
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
        pending_web_checkout_ops: HashMap::new(),
        pending_web_add_project_ops: HashMap::new(),
        pending_web_pr_lookup_ops: HashMap::new(),
        pending_pr_attach_ops: HashMap::new(),
        pending_delete_ops_web: HashMap::new(),
        pending_create_ops: HashMap::new(),
        pending_web_launch_ops: HashMap::new(),
        last_created_op_id: None,
        created_session_by_op: HashMap::new(),
    };
    (engine, tmp)
}

/// A Support-tab record (`agent_tabs` entry) owned by `session_id`.
pub(crate) fn sample_tab(id: &str, session_id: &str, provider: &str, sort_order: i64) -> AgentTab {
    AgentTab {
        id: id.to_string(),
        session_id: session_id.to_string(),
        provider: ProviderKind::new(provider),
        sort_order,
        created_at: Utc::now(),
    }
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
        provider: ProviderKind::new("claude"),
        workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
            project_id: project_id.to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            initial_branch: branch.to_string(),
            branch_provenance: crate::model::BranchProvenance::CreatedByDux,
            worktree_path: format!("/tmp/{id}-worktree"),
        }),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
    }
}

/// A STANDALONE agent: a folder the user already had, no project, no branch,
/// no worktree dux owns. The title is always set, as creation guarantees.
pub(crate) fn sample_standalone_session(id: &str, folder: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
        provider: ProviderKind::new("claude"),
        workspace: crate::model::AgentWorkspace::Folder(crate::model::FolderWorkspace {
            folder_path: folder.to_string(),
        }),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
    }
}

/// Pump worker events until the `gh` host probe's result has been applied.
///
/// Shared by the engine's own probe tests and the wire toggle's lifecycle
/// tests, because both need to distinguish "the probe was launched" from "the
/// probe's answer has landed", which is the whole point of the off-to-on rule:
/// an enable site launches the probe and does nothing else, and the completion
/// is what arms the pull-request work.
pub(crate) fn settle_gh_probe(engine: &mut Engine) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let Ok(event) = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_millis(500))
        else {
            continue;
        };
        let is_probe = matches!(event, crate::worker::WorkerEvent::GhStatusChecked { .. });
        engine.process_worker_event(event);
        if is_probe {
            return;
        }
    }
    panic!("host probe never reported");
}
