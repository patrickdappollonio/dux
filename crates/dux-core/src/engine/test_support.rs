//! Shared test fixtures for engine submodule unit tests. Test-only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use crate::test_scratch::ScratchDir;
use chrono::Utc;

use crate::config::DuxPaths;
use crate::engine::Engine;
use crate::lockfile::SingleInstanceLock;
use crate::model::{
    AgentSession, AgentTab, GhStatus, Project, ProjectBranchStatus, ProviderKind, SessionStatus,
};
use crate::storage::SessionStore;

/// Construct a minimally-wired `Engine` for tests, alongside the scratch
/// directory that backs its on-disk state (sqlite, lockfile, config writes).
/// Keep it alive for the test; it is removed with retries on drop, because the
/// engine's workers may still be writing into it when the test ends.
pub(crate) fn test_engine() -> (Engine, ScratchDir) {
    let tmp = ScratchDir::new();
    let engine = test_engine_at(tmp.path());
    (engine, tmp)
}

/// An engine over the state under `root`, as [`test_engine`] builds one: a
/// second call on the same root, once the first engine is dropped, is the
/// next start of dux on the same config directory.
pub(crate) fn test_engine_at(root: &std::path::Path) -> Engine {
    let root = root.to_path_buf();
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
    let config_writer = crate::config_queue::ConfigWriteQueue::with_status_lane(
        paths.config_path.clone(),
        worker_tx.clone(),
        &crate::config::Config::default(),
    );
    let engine = Engine {
        // Stock provider names, harmless commands: a test that launches an agent
        // must never exec the developer's real CLI.
        config: crate::test_provider::harmless_config(),
        paths,
        session_store,
        projects: Vec::new(),
        sessions: Vec::new(),
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        changed_files_revision: 0,
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
        command_applies: 0,
        deferred_commands: Vec::new(),
        reload_guard: None,
        providers: HashMap::new(),
        running_provider_pins: HashMap::new(),
        launched_drop_paste: HashMap::new(),
        companion_terminals: HashMap::new(),
        agent_tabs: HashMap::new(),
        terminating_ptys: Vec::new(),
        process_registry: Default::default(),
        removal_workers: Vec::new(),
        pending_group_removals: Vec::new(),
        pending_detachments: Vec::new(),
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
        pr_poll_inactive_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        pr_inactive_sessions: Default::default(),
        pr_inactive_sweep_at: Default::default(),
        pr_return_checks_owed: Default::default(),
        branch_sync_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        branch_sync_wait: Arc::new(Default::default()),
        pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        refs_watcher: None,
        refs_watch_paths: HashMap::new(),
        resume_fallback_candidates: HashMap::new(),
        resumed_tab_runs: HashSet::new(),
        pending_deletions: HashSet::new(),
        folder_repo_statuses: HashMap::new(),
        changed_files_failures: Default::default(),
        closing_sessions: HashSet::new(),
        deletion_busy_messages: HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        changed_files_refresh: Default::default(),
        watched_session_id: None,
        has_active_processes: Arc::new(AtomicBool::new(false)),
        serve_memory: Default::default(),
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
        failed_tab_runs: HashMap::new(),
        pty_progress: HashMap::new(),
        agent_viewed: HashMap::new(),
        last_foreground_refresh: None,
        pending_web_checkout_ops: HashMap::new(),
        pending_change_base_ops: HashMap::new(),
        pending_web_add_project_ops: HashMap::new(),
        pending_web_pr_lookup_ops: HashMap::new(),
        pending_pr_attach_ops: HashMap::new(),
        pending_recreate_ops: HashMap::new(),
        pending_delete_ops_web: HashMap::new(),
        pending_delete_reports_web: HashMap::new(),
        pending_create_ops: HashMap::new(),
        pending_web_launch_ops: HashMap::new(),
        live_status_keys: Default::default(),
        last_created_op_id: None,
        operations: Default::default(),
        operation_in_dispatch: None,
        created_session_by_op: HashMap::new(),
        removal_coordination: Default::default(),
    };
    // As the real start does: the process registry saved in this database.
    // Its prune is waited for, so a test registering sessions nothing runs in
    // (made-up sids) never has them dropped by a prune that read the process
    // table after they were registered.
    if let Some(prune) = engine
        .process_registry
        .attach_store(&engine.paths.sessions_db_path)
    {
        let _ = prune.join();
    }
    engine
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
        // Deliberately NOT the session id: the slot tab is a stored pointer at a
        // generated id, and a fixture that reused the session id would hide
        // every place still assuming the two are the same string.
        slot_tab_id: format!("{id}-slot"),
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
        // Deliberately NOT the session id: the slot tab is a stored pointer at a
        // generated id, and a fixture that reused the session id would hide
        // every place still assuming the two are the same string.
        slot_tab_id: format!("{id}-slot"),
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

/// Take the next `ChangedFilesReady` off the worker lane, applying whatever
/// else arrives first.
///
/// Arming a watch also launches the directory probe, so a bare `recv_timeout`
/// races the two events. Every drainer in production handles both; a test that
/// wants one of them says which.
pub(crate) fn recv_changed_files_ready(engine: &mut Engine) -> crate::worker::WorkerEvent {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let Ok(event) = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_millis(500))
        else {
            continue;
        };
        if matches!(event, crate::worker::WorkerEvent::ChangedFilesReady { .. }) {
            return event;
        }
        engine.process_worker_event(event);
    }
    panic!("the changed-files refresh never reported");
}

/// What [`delete_through_pipeline`] reports: what happened to the worktree and
/// its branches, and the record the delete vanished.
pub(crate) struct PipelineDelete {
    pub removal: crate::engine::WorktreeRemoval,
    pub finish: crate::engine::FinishDeleteSessionOutcome,
}

/// Delete an agent the way both surfaces do, through the one pipeline:
/// `begin_delete_session`, the record vanished at once, and for a
/// worktree-removing delete the reaper and the removal worker driven until it
/// reports. `Ok(None)` for an unknown agent; a refusal or a removal failure
/// is an `Err` carrying the sentence the surfaces would show.
pub(crate) fn delete_through_pipeline(
    engine: &mut Engine,
    session_id: &str,
    delete_worktree: bool,
    delete_branch: Option<bool>,
) -> anyhow::Result<Option<PipelineDelete>> {
    use crate::engine::BeginDeleteSessionOutcome;
    match engine.begin_delete_session(session_id, delete_worktree, delete_branch) {
        BeginDeleteSessionOutcome::NotFound => Ok(None),
        BeginDeleteSessionOutcome::AlreadyInFlight => {
            anyhow::bail!("a delete of this agent is already in progress")
        }
        BeginDeleteSessionOutcome::TabLaunching => {
            anyhow::bail!("a tab of this agent is still launching")
        }
        BeginDeleteSessionOutcome::Refused { message } => anyhow::bail!("{message}"),
        BeginDeleteSessionOutcome::Inline { removal } => {
            let finish = engine
                .finish_delete_session(session_id)?
                .expect("the agent was there");
            Ok(Some(PipelineDelete { removal, finish }))
        }
        BeginDeleteSessionOutcome::AsyncStarted { .. } => {
            let finish = engine
                .finish_delete_session(session_id)?
                .expect("the agent was there");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                for removal in engine.reap_terminating_ptys().removals {
                    let _ = engine.dispatch_deferred_worktree_removal(removal);
                }
                match engine
                    .worker_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                {
                    Ok(crate::worker::WorkerEvent::WorktreeRemoveCompleted {
                        session_id: done,
                        result,
                    }) if done == session_id => {
                        engine.process_worker_event(
                            crate::worker::WorkerEvent::WorktreeRemoveCompleted {
                                session_id: done,
                                result: result.clone(),
                            },
                        );
                        return match result {
                            Ok(branches) => Ok(Some(PipelineDelete {
                                removal: crate::engine::WorktreeRemoval::Performed { branches },
                                finish,
                            })),
                            Err(message) => Err(anyhow::anyhow!(message)),
                        };
                    }
                    Ok(event) => {
                        let _ = engine.process_worker_event(event);
                    }
                    Err(_) => assert!(
                        std::time::Instant::now() < deadline,
                        "the removal never reported"
                    ),
                }
            }
        }
    }
}
