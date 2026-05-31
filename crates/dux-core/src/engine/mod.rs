//! The headless `Engine`: the single owner of dux's domain state. Surfaces (the
//! TUI `App` today, the web server later) embed/drive it. In E2 it is a passive
//! state container; domain operations and workers move into `Engine` methods in E3.

pub mod command;
mod events;

pub use command::Command;
pub use events::{
    AgentLaunchFailedOutcome, AgentLaunchReadyOutcome, AgentLaunchReadyView,
    BeginDeleteSessionOutcome, BeginDeleteSessionView, DeleteTerminalView, DetachedSession,
    DispatchAgentLaunchView, DoDeleteSessionOutcome, DoDeleteSessionView, EventReaction,
    FinishDeleteSessionOutcome, FinishDeleteSessionView, ProjectPersistenceOutcome,
    ProjectPersistenceView, StatusUpdate,
};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::{Config, DuxPaths, ProjectConfig};
use crate::lockfile::SingleInstanceLock;
use crate::model::{
    AgentSession, ChangedFile, CompanionTerminal, GhStatus, PrInfo, Project, ProviderKind,
    SessionStatus,
};
use crate::pty::PtyClient;
use crate::storage::SessionStore;
use crate::worker::{BranchSyncEntry, PrSyncEntry, ProjectPersistenceAction, WorkerEvent};

pub struct Engine {
    pub config: Config,
    pub paths: DuxPaths,
    pub session_store: SessionStore,
    pub projects: Vec<Project>,
    pub sessions: Vec<AgentSession>,
    pub staged_files: Vec<ChangedFile>,
    pub unstaged_files: Vec<ChangedFile>,
    pub terminal_counter: usize,
    pub github_integration_enabled: bool,
    pub single_instance_lock: SingleInstanceLock,

    // Batch B fields
    pub worker_tx: Sender<WorkerEvent>,
    pub worker_rx: Receiver<WorkerEvent>,
    pub providers: HashMap<String, PtyClient>,
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub running_provider_pins: HashMap<String, ProviderKind>,
    pub companion_terminals: HashMap<String, CompanionTerminal>,
    pub gh_status: GhStatus,
    pub pr_statuses: HashMap<String, PrInfo>,
    pub branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub pr_sync_enabled: Arc<AtomicBool>,
    /// File-system watcher for `.git/refs/heads/` directories. `None` if the
    /// watcher could not be created (graceful fallback to poll-only).
    pub refs_watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Maps watched worktree paths back to session IDs so the refs watcher
    /// can route change events.
    pub refs_watch_paths: HashMap<PathBuf, String>,
    /// Session IDs spawned with resume args and the wall-clock time the resume
    /// attempt began. Used for one-shot fallbacks when resume exits quickly or
    /// hangs without rendering visible output.
    pub resume_fallback_candidates: HashMap<String, Instant>,
    /// Session IDs whose worktree is currently being removed by a background
    /// worker. Prevents duplicate delete requests from spawning a second
    /// worker while the first is still running; also drives the dimmed
    /// visual cue on the left pane row so the user can see the in-flight
    /// state.
    pub pending_deletions: HashSet<String>,
    /// Maps session IDs to the exact Busy message set by
    /// `begin_delete_session`. Used by the worker event handler to decide
    /// whether the current status-line content was set by this deletion (and
    /// should be cleared) or by an unrelated operation (and should be left
    /// alone). Cleared per-session when the worker event arrives.
    pub deletion_busy_messages: HashMap<String, String>,
    pub watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub has_active_processes: Arc<AtomicBool>,
    pub create_agent_in_flight: bool,
    pub agent_launches_in_flight: HashSet<String>,
    pub pulls_in_flight: HashSet<String>,
    pub resource_stats_in_flight: bool,
    /// Last-checked timestamps for the one-shot PR-check rate-limiter.
    /// Keyed by `session_id`; written by `process_worker_event`'s
    /// `PrStatusReady` arm and read by `spawn_pr_check_for_session` to
    /// skip checks made within the last 10 seconds.
    pub pr_last_checked: HashMap<String, Instant>,
}

impl Engine {
    pub fn spawn_project_persistence(&self, action: ProjectPersistenceAction) {
        let db_path = self.paths.sessions_db_path.clone();
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let store = SessionStore::open(&db_path)?;
                match &action {
                    ProjectPersistenceAction::Add { project, .. } => {
                        store.upsert_project(&ProjectConfig {
                            id: project.id.clone(),
                            path: project.path.clone(),
                            name: Some(project.name.clone()),
                            default_provider: project
                                .explicit_default_provider
                                .as_ref()
                                .map(|provider| provider.as_str().to_string()),
                            leading_branch: project.leading_branch.clone(),
                            auto_reopen_agents: project.auto_reopen_agents,
                            startup_command: project.startup_command.clone(),
                            env: project.env.clone(),
                        })?;
                    }
                    ProjectPersistenceAction::Remove { project_id, .. }
                    | ProjectPersistenceAction::Delete { project_id, .. } => {
                        store.delete_project(project_id)?;
                    }
                    ProjectPersistenceAction::UpdateDefaultProvider {
                        project_id,
                        provider,
                        ..
                    } => {
                        store.update_project_default_provider(
                            project_id,
                            provider.as_ref().map(|provider| provider.as_str()),
                        )?;
                    }
                    ProjectPersistenceAction::UpdateAutoReopen {
                        project_id,
                        auto_reopen_agents,
                        ..
                    } => {
                        store.update_project_auto_reopen(project_id, *auto_reopen_agents)?;
                    }
                    ProjectPersistenceAction::UpdateStartupCommand {
                        project_id,
                        startup_command,
                        ..
                    } => {
                        store.update_project_startup_command(
                            project_id,
                            startup_command.as_deref(),
                        )?;
                    }
                    ProjectPersistenceAction::UpdateEnv {
                        project_id, env, ..
                    } => {
                        store.update_project_env(project_id, env)?;
                    }
                }
                Ok(())
            })()
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ProjectPersistenceCompleted { action, result });
        });
    }

    /// Validate a raw path string before registering it as a project. Checks
    /// that the path exists, is a git repository, and is not already
    /// registered. Returns the canonicalized path on success or a
    /// user-facing error string on failure.
    pub fn validate_project_add_path(
        &self,
        raw_path: &str,
    ) -> std::result::Result<PathBuf, String> {
        let trimmed = raw_path.trim();
        let path = PathBuf::from(trimmed)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(trimmed));
        if !path.exists() || !crate::git::is_git_repo(&path) {
            crate::logger::error(&format!("add project rejected for {}", path.display()));
            return Err(format!("\"{}\" is not a git repository.", path.display()));
        }
        if self.projects.iter().any(|project| {
            PathBuf::from(&project.path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&project.path))
                == path
        }) {
            return Err(format!(
                "\"{}\" is already registered as a project.",
                path.display()
            ));
        }
        Ok(path)
    }

    /// Re-resolve the in-memory `default_provider` for each project against
    /// the current config. Projects with an explicit `default_provider` keep
    /// their override; projects without one pick up the new global default.
    pub fn refresh_project_defaults(&mut self) {
        let fallback = self.config.default_provider();
        for project in self.projects.iter_mut() {
            project.default_provider = project
                .explicit_default_provider
                .clone()
                .unwrap_or_else(|| fallback.clone());
        }
    }

    pub fn spawn_branch_sync_worker(&self) {
        let interval_secs = self.config.ui.branch_sync_interval;
        if interval_secs == 0 {
            return; // disabled by config
        }
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.branch_sync_sessions);
        thread::spawn(move || {
            let interval = Duration::from_secs(u64::from(interval_secs));
            loop {
                thread::sleep(interval);
                let snapshot = match sessions.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => continue,
                };
                let mut updates = Vec::new();
                for entry in &snapshot {
                    if let Ok(actual) = crate::git::current_branch(Path::new(&entry.worktree_path))
                        && actual != entry.branch_name
                    {
                        updates.push((entry.session_id.clone(), actual));
                    }
                }
                if !updates.is_empty() && tx.send(WorkerEvent::BranchSyncReady(updates)).is_err() {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }

    pub fn spawn_refs_watcher(&mut self) {
        use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};

        let tx = self.worker_tx.clone();
        // Build a reverse map of watched paths for event routing.
        let path_to_session: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let path_map = Arc::clone(&path_to_session);
        let debounce_map: Arc<Mutex<HashMap<String, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let debounce = Arc::clone(&debounce_map);

        let watcher_result = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else { return };
                // We only care about data modifications (ref file updates).
                if !event.kind.is_modify() && !event.kind.is_create() {
                    return;
                }
                let map = match path_map.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let mut debounce_guard = match debounce.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for event_path in &event.paths {
                    // Walk up from the event path to find a watched parent dir.
                    for (watched, session_id) in map.iter() {
                        if event_path.starts_with(watched) {
                            // Debounce: skip if we already sent an event within the last 5s.
                            let now = Instant::now();
                            if let Some(last) = debounce_guard.get(session_id)
                                && now.duration_since(*last) < Duration::from_secs(5)
                            {
                                continue;
                            }
                            debounce_guard.insert(session_id.clone(), now);
                            crate::logger::debug(&format!(
                                "[gh-integration] refs watcher: detected change at {}, debouncing for session {}",
                                event_path.display(),
                                session_id,
                            ));
                            let _ = tx.send(WorkerEvent::RefsChanged(session_id.clone()));
                        }
                    }
                }
            },
            NotifyConfig::default(),
        );

        match watcher_result {
            Ok(watcher) => {
                self.refs_watcher = Some(Arc::new(Mutex::new(watcher)));
                self.refs_watch_paths.clear();
                // Populate the path map and start watching existing sessions.
                let mut paths = HashMap::new();
                for session in &self.sessions {
                    let refs_dir = PathBuf::from(&session.worktree_path)
                        .join(".git")
                        .join("refs")
                        .join("heads");
                    if refs_dir.is_dir()
                        && let Some(ref watcher_arc) = self.refs_watcher
                    {
                        match watcher_arc.lock() {
                            Ok(mut w) => match w.watch(&refs_dir, RecursiveMode::NonRecursive) {
                                Ok(()) => {
                                    crate::logger::debug(&format!(
                                        "[gh-integration] refs watcher: watching {} for session {}",
                                        refs_dir.display(),
                                        session.id,
                                    ));
                                    paths.insert(refs_dir.clone(), session.id.clone());
                                }
                                Err(e) => {
                                    crate::logger::debug(&format!(
                                        "[gh-integration] refs watcher: failed to watch {}: {}",
                                        refs_dir.display(),
                                        e,
                                    ));
                                }
                            },
                            Err(poison) => {
                                crate::logger::error(&format!(
                                    "[gh-integration] refs watcher mutex poisoned, will not watch {} for session {} \u{2014} PR updates for this session will not arrive until dux restarts: {}",
                                    refs_dir.display(),
                                    session.id,
                                    poison,
                                ));
                            }
                        }
                    }
                }
                self.refs_watch_paths = paths.clone();
                // Populate the closure's path map so events can route to sessions.
                if let Ok(mut map) = path_to_session.lock() {
                    *map = paths;
                }
                crate::logger::info(&format!(
                    "[gh-integration] refs watcher: initialized, watching {} session(s)",
                    self.refs_watch_paths.len(),
                ));
            }
            Err(e) => {
                crate::logger::warn(&format!(
                    "[gh-integration] refs watcher: failed to create watcher (falling back to poll-only): {}",
                    e,
                ));
            }
        }
    }

    pub fn spawn_gh_status_check(&self) {
        if !self.github_integration_enabled {
            return;
        }
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            use crate::model::GhStatus;
            // Step 1: Is `gh` on PATH?
            let on_path = std::process::Command::new("which")
                .arg("gh")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !on_path {
                crate::logger::info("[gh-integration] gh CLI not found on PATH");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotInstalled));
                return;
            }
            // Step 2: Is `gh` authenticated?
            let authed = std::process::Command::new("gh")
                .args(["auth", "status"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !authed {
                crate::logger::info("[gh-integration] gh CLI found but not authenticated");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotAuthenticated));
                return;
            }
            crate::logger::info("[gh-integration] gh CLI available and authenticated");
            let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::Available));
        });
    }

    pub fn spawn_changed_files_poller(&self) {
        let tx = self.worker_tx.clone();
        let watched = Arc::clone(&self.watched_worktree);
        let has_agent = Arc::clone(&self.has_active_processes);
        thread::spawn(move || {
            loop {
                let interval = if has_agent.load(Ordering::Relaxed) {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                };
                thread::sleep(interval);
                let path = watched.lock().ok().and_then(|guard| guard.clone());
                if let Some(worktree_path) = path
                    && let Ok((staged, unstaged)) = crate::git::changed_files(&worktree_path)
                    && tx
                        .send(WorkerEvent::ChangedFilesReady { staged, unstaged })
                        .is_err()
                {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }

    pub fn spawn_browser_entries(&self, dir: &Path) {
        let tx = self.worker_tx.clone();
        let dir = dir.to_path_buf();
        thread::spawn(move || {
            let entries = crate::project_browser::browser_entries(&dir);
            crate::logger::debug(&format!(
                "browser loaded {} with {} entries",
                dir.display(),
                entries.len()
            ));
            let _ = tx.send(WorkerEvent::BrowserEntriesReady {
                dir: dir.clone(),
                entries,
            });
        });
    }

    pub fn spawn_project_worktrees_worker(&self, project: Project) {
        let tx = self.worker_tx.clone();
        let paths = self.paths.clone();
        let sessions = self.sessions.clone();
        thread::spawn(move || {
            let result = crate::git::list_worktrees(Path::new(&project.path))
                .map(|worktrees| {
                    crate::project_browser::classify_project_worktrees(
                        &project, &paths, &sessions, worktrees,
                    )
                })
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ProjectWorktreesReady {
                project_id: project.id,
                result,
            });
        });
    }

    pub fn spawn_project_branch_status_checks(&self) {
        for project in self.projects.iter().filter(|project| !project.path_missing) {
            let project = project.clone();
            let worker_tx = self.worker_tx.clone();
            thread::spawn(move || {
                crate::project_browser::run_project_branch_status_job(project, worker_tx);
            });
        }
    }

    // -- GitHub PR integration workers --

    pub fn spawn_pr_sync_worker(&self) {
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let enabled = Arc::clone(&self.pr_sync_enabled);
        enabled.store(true, Ordering::Relaxed);
        thread::spawn(move || {
            let interval = Duration::from_secs(45);
            loop {
                thread::sleep(interval);
                if !enabled.load(Ordering::Relaxed) {
                    break;
                }
                let results = crate::gh::run_pr_sync(&sessions);
                if !results.is_empty() && tx.send(WorkerEvent::PrStatusReady(results)).is_err() {
                    break;
                }
            }
        });
    }

    pub fn spawn_initial_pr_refresh(&self) {
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.pr_sync_sessions);
        thread::spawn(move || {
            let results = crate::gh::run_pr_sync(&sessions);
            if !results.is_empty() {
                let _ = tx.send(WorkerEvent::PrStatusReady(results));
            }
        });
    }

    /// Gather the labeled PIDs that the resource monitor should report on.
    /// Each entry is `(label, root_pid)` — the worker will aggregate the
    /// full process tree under each root.
    fn resource_monitor_targets(&self) -> Vec<(String, u32)> {
        let mut targets = Vec::new();
        for session in &self.sessions {
            if let Some(pty) = self.providers.get(&session.id)
                && let Some(pid) = pty.child_process_id()
            {
                let title = session.title.as_deref().unwrap_or(&session.branch_name);
                let provider = session.provider.as_str();
                targets.push((format!("Agent ({provider}): {title}"), pid));
            }
        }
        for terminal in self.companion_terminals.values() {
            if let Some(pid) = terminal.client.child_process_id() {
                let label = match &terminal.foreground_cmd {
                    Some(cmd) => format!("Terminal ({cmd}): {}", terminal.label),
                    None => format!("Terminal: {}", terminal.label),
                };
                targets.push((label, pid));
            }
        }
        targets
    }

    pub fn spawn_resource_stats_worker(&mut self) {
        if self.resource_stats_in_flight {
            return;
        }
        self.resource_stats_in_flight = true;
        let targets = self.resource_monitor_targets();
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let rows = crate::resource_stats::collect_resource_stats(targets);
            let _ = tx.send(WorkerEvent::ResourceStatsReady(rows));
        });
    }

    /// Trigger a one-shot PR check for a single session, unless it was checked
    /// recently (within 10 seconds).
    ///
    /// The timestamp is recorded BEFORE the worker thread is spawned so a burst
    /// of triggers within a single event-loop tick — e.g. several callers each
    /// invoking this for the same session before the first worker's
    /// `PrStatusReady` event has been processed — does not bypass the
    /// rate-limit and spawn N concurrent `gh` subprocesses.
    pub fn spawn_pr_check_for_session(&mut self, session_id: &str) {
        if !self.github_integration_enabled
            || !matches!(self.gh_status, crate::model::GhStatus::Available)
        {
            return;
        }
        // Rate-limit: skip if checked within the last 10 seconds.
        if let Some(last) = self.pr_last_checked.get(session_id)
            && last.elapsed() < Duration::from_secs(10)
        {
            return;
        }
        self.pr_last_checked
            .insert(session_id.to_string(), Instant::now());
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let known_pr = self
            .session_store
            .load_prs(session_id)
            .ok()
            .and_then(|prs| prs.into_iter().next());
        let entry = PrSyncEntry {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            worktree_path: session.worktree_path.clone(),
            known_pr,
            agent_exited: !self.providers.contains_key(session_id),
        };
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let result = crate::gh::check_pr_for_entry(&entry);
            let _ = tx.send(WorkerEvent::PrStatusReady(vec![(entry.session_id, result)]));
        });
    }
}

impl Engine {
    pub fn mark_session_status(&mut self, session_id: &str, status: SessionStatus) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        {
            if session.status == status {
                return;
            }
            session.status = status;
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
        }
    }

    pub fn mark_session_desired_running(&mut self, session_id: &str, desired: bool) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        {
            if session.desired_running == desired {
                return;
            }
            session.desired_running = desired;
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
        } else {
            let _ = self.session_store.set_desired_running(session_id, desired);
        }
    }

    pub fn mark_session_provider_started(&mut self, session_id: &str) {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        else {
            return;
        };

        let provider = session.provider.clone();
        if !session.mark_provider_started(&provider) {
            return;
        }

        session.updated_at = Utc::now();
        let _ = self.session_store.upsert_session(session);
    }

    /// Refreshes the shared session snapshot used by the branch-sync background
    /// worker.
    pub fn update_branch_sync_sessions(&self) {
        if let Ok(mut guard) = self.branch_sync_sessions.lock() {
            *guard = self
                .sessions
                .iter()
                .map(|s| BranchSyncEntry {
                    session_id: s.id.clone(),
                    worktree_path: s.worktree_path.clone(),
                    branch_name: s.branch_name.clone(),
                })
                .collect();
        }
    }

    /// Refreshes the shared session snapshot used by the PR-sync background
    /// worker. Includes the latest known PR per session so the worker can use
    /// `gh pr view` for sessions that already have a persisted PR association.
    pub fn update_pr_sync_sessions(&self) {
        let known_prs = self.session_store.load_all_latest_prs().unwrap_or_default();
        let known_map: HashMap<String, crate::storage::StoredPr> = known_prs
            .into_iter()
            .map(|pr| (pr.session_id.clone(), pr))
            .collect();

        if let Ok(mut guard) = self.pr_sync_sessions.lock() {
            *guard = self
                .sessions
                .iter()
                .map(|s| PrSyncEntry {
                    session_id: s.id.clone(),
                    branch_name: s.branch_name.clone(),
                    worktree_path: s.worktree_path.clone(),
                    known_pr: known_map.get(&s.id).cloned(),
                    agent_exited: !self.providers.contains_key(&s.id),
                })
                .collect();
        }
    }
}

impl Engine {
    pub fn project_explicit_default_provider(&self, project_id: &str) -> Option<ProviderKind> {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.explicit_default_provider.clone())
    }

    pub fn project_uses_explicit_default_provider(&self, project_id: &str) -> bool {
        self.project_explicit_default_provider(project_id).is_some()
    }

    pub fn project_allows_auto_reopen(&self, project_id: &str) -> bool {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.auto_reopen_agents)
            .unwrap_or(true)
    }

    pub fn project_name_for_session(&self, session: &AgentSession) -> String {
        self.projects
            .iter()
            .find(|p| p.id == session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Provider currently driving the session's live PTY, if any. After an
    /// in-place provider swap while the agent is still running, this returns
    /// the *original* provider until the user exits and relaunches — so the
    /// pane title doesn't lie about what's actually on screen.
    pub fn running_provider_for(&self, session: &AgentSession) -> ProviderKind {
        self.running_provider_pins
            .get(&session.id)
            .cloned()
            .unwrap_or_else(|| session.provider.clone())
    }

    pub fn should_resume_session(&self, session: &AgentSession) -> bool {
        let cfg = crate::config::provider_config(&self.config, &session.provider);
        cfg.supports_session_resume() && session.has_started_provider(&session.provider)
    }
}
