//! Engine-side dispatch for `WorkerEvent`s. `Engine::process_worker_event`
//! performs the domain-state mutation for an event and returns an
//! `EventReaction` that tells the App caller what view follow-up to perform.
//!
//! The Engine MUST NOT touch view state (status line, prompt, focus, derived
//! caches like `left_items_cache` or `files_index`). Anything view-side is
//! described by an `EventReaction` variant; the App's `apply_reaction`
//! translates each variant back into concrete view mutations.

use std::path::Path;

use chrono::Utc;

use crate::config::Config;
use crate::engine::Engine;
use crate::logger;
use crate::model::{GhStatus, PrState, Project, ProjectBranchStatus};
use crate::startup::StartupCommandLatestLog;
use crate::statusline::StatusTone;
use crate::storage::StoredPr;
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchReadyData, BranchWarningKind, BrowserEntry,
    CreateAgentBranchInspection, NonDefaultBranchAction, ProjectPersistenceAction,
    ProjectWorktreeEntry, PullTarget, ResolvedPullRequest, ResourceStats, WorkerEvent,
};

/// Status-line update returned from the Engine for the App to apply.
#[derive(Clone, Debug)]
pub struct StatusUpdate {
    pub tone: StatusTone,
    pub message: String,
}

impl StatusUpdate {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Info,
            message: message.into(),
        }
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Busy,
            message: message.into(),
        }
    }
    #[allow(dead_code)]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Warning,
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Error,
            message: message.into(),
        }
    }
}

/// What the App must do after the Engine processed a `WorkerEvent`. The Engine
/// handles all domain-state mutations (sessions, projects, providers,
/// session_store, sync entries, in-flight maps, env, etc.); anything that
/// touches view state (status line, prompt, focus, input_target, derived caches
/// like `left_items_cache` and `files_index`) is described here.
pub enum EventReaction {
    /// Engine fully handled the event; no view follow-up needed.
    Nothing,
    /// Set the status line.
    Status(StatusUpdate),
    /// Perform multiple reactions in order.
    Multi(Vec<EventReaction>),

    // -- View-sync triggers (the App's derived cache lives in App state). --
    RebuildLeftItems,
    ReloadChangedFiles,
    ClampFilesCursor,

    // -- Agent launch (T2a: pass-through; T2b will swap to typed outcome). --
    AgentLaunchReady(Box<AgentLaunchReadyData>),
    AgentLaunchFailed(Box<AgentLaunchFailedData>),

    // -- Commit message overlay (App formats final status with bindings). --
    CommitMessageGenerated(String),
    CommitMessageFailed(String),

    // -- Picker/browser prompts. --
    BrowserEntriesArrived {
        dir: std::path::PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ProjectWorktreesArrived {
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
    },

    // -- PR / refs follow-ups (App owns `pr_last_checked`). --
    UpdatePrLastChecked(Vec<String>),
    SpawnPrCheckForSession(String),
    OpenNewAgentPromptForPr(Box<ResolvedPullRequest>),
    /// gh just became available with integration enabled: the App must spawn
    /// the PR sync worker + the initial refresh. These spawn helpers depend on
    /// PR-sync helper free functions that still live in the binary, so they
    /// stay App-side.
    SpawnPrSyncWorkers,

    // -- Worktree delete follow-up. --
    WorktreeRemoveSucceeded {
        session_id: String,
        branch_already_deleted: bool,
        our_busy_message: Option<String>,
    },
    WorktreeRemoveFailed {
        session_id: String,
        message: String,
    },

    // -- Resource monitor. --
    ResourceStatsArrived(Vec<ResourceStats>),

    // -- Add-project / branch-checkout follow-ups (App helpers). --
    AddProjectAfterBranchCheckout {
        path: String,
        name: String,
        target_branch: String,
        leading_branch: String,
    },

    // -- Branch inspection follow-ups (App helpers). --
    ContinueCreateAgentAfterInspection {
        project: Project,
        inspection: CreateAgentBranchInspection,
    },
    DispatchProjectDefaultBranchCheckout {
        project: Project,
        default_branch: String,
    },

    // -- Config reload (App helpers). --
    ApplyReloadedConfig(Box<Config>),
    OpenConfigReloadFailedModal(String),

    // -- Project persistence (App helper stays put for T3). --
    ProjectPersistenceCompleted {
        action: ProjectPersistenceAction,
        result: Result<(), String>,
    },

    // -- Startup command / log viewer (App formats key + opens overlay). --
    StartupCommandSucceeded {
        project_name: String,
    },
    StartupLogArrived {
        scope_label: String,
        log: StartupCommandLatestLog,
    },
}

impl Engine {
    /// Process a `WorkerEvent`: perform engine-side mutations and return the
    /// view follow-up the App caller should apply.
    ///
    /// The Engine MUST NOT touch view state. Anything view-side is returned
    /// via `EventReaction` for the App to apply.
    pub fn process_worker_event(&mut self, event: WorkerEvent) -> EventReaction {
        match event {
            WorkerEvent::CreateAgentProgress(message) => {
                EventReaction::Status(StatusUpdate::busy(message))
            }
            WorkerEvent::CreateAgentFailed(message) => {
                self.create_agent_in_flight = false;
                EventReaction::Status(StatusUpdate::error(message))
            }
            WorkerEvent::AgentLaunchReady(boxed) => EventReaction::AgentLaunchReady(boxed),
            WorkerEvent::AgentLaunchFailed(boxed) => EventReaction::AgentLaunchFailed(boxed),
            WorkerEvent::ChangedFilesReady { staged, unstaged } => {
                self.staged_files = staged;
                self.unstaged_files = unstaged;
                EventReaction::ClampFilesCursor
            }
            WorkerEvent::CommitMessageGenerated(msg) => EventReaction::CommitMessageGenerated(msg),
            WorkerEvent::CommitMessageFailed(err) => EventReaction::CommitMessageFailed(err),
            WorkerEvent::PushCompleted(result) => match result {
                Ok(()) => EventReaction::Status(StatusUpdate::info(
                    "Pushed to remote successfully. Your changes are now available to collaborators.",
                )),
                Err(e) => EventReaction::Status(StatusUpdate::error(format!(
                    "Push to remote failed: {e}"
                ))),
            },
            WorkerEvent::PullCompleted {
                repo_path,
                target,
                result,
            } => {
                self.pulls_in_flight.remove(&repo_path);
                match target {
                    PullTarget::Project {
                        project_id,
                        project_name,
                        ..
                    } => match result {
                        Ok(branch_name) => {
                            if let Some(existing) =
                                self.projects.iter_mut().find(|c| c.id == project_id)
                                && let Some(branch_name) = branch_name
                            {
                                existing.current_branch = branch_name;
                                existing.branch_status = if existing.leading_branch.as_deref()
                                    == Some(&existing.current_branch)
                                {
                                    ProjectBranchStatus::Leading
                                } else if existing.leading_branch.is_some() {
                                    ProjectBranchStatus::NotLeading
                                } else {
                                    let warning = crate::git::branch_warning_kind(
                                        Path::new(&existing.path),
                                        &existing.current_branch,
                                    );
                                    crate::git::branch_status_from_warning(warning.as_ref())
                                };
                            }
                            EventReaction::Status(StatusUpdate::info(format!(
                                "Refreshed project \"{}\". Local branch is up to date with remote.",
                                project_name,
                            )))
                        }
                        Err(e) => EventReaction::Status(StatusUpdate::error(format!(
                            "Project refresh failed for \"{}\": {e}",
                            project_name
                        ))),
                    },
                    PullTarget::Session => match result {
                        Ok(_) => EventReaction::Multi(vec![
                            EventReaction::Status(StatusUpdate::info(
                                "Pulled latest changes from remote successfully. Local branch is up to date.",
                            )),
                            EventReaction::ReloadChangedFiles,
                        ]),
                        Err(e) => EventReaction::Status(StatusUpdate::error(format!(
                            "Pull from remote failed: {e}"
                        ))),
                    },
                }
            }
            WorkerEvent::ClipboardCopyCompleted { label, result } => match result {
                Ok(()) => EventReaction::Status(StatusUpdate::info(label)),
                Err(e) => EventReaction::Status(StatusUpdate::error(format!(
                    "Clipboard copy failed: {e}"
                ))),
            },
            WorkerEvent::BranchRenameCompleted {
                session_id,
                new_branch,
                previous_title,
                result,
            } => match result {
                Ok(()) => {
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        session.branch_name = new_branch.clone();
                        session.updated_at = Utc::now();
                        let _ = self.session_store.upsert_session(session);
                    }
                    self.update_branch_sync_sessions();
                    EventReaction::Multi(vec![
                        EventReaction::RebuildLeftItems,
                        EventReaction::Status(StatusUpdate::info(format!(
                            "Renamed agent and branch to \"{new_branch}\"."
                        ))),
                    ])
                }
                Err(e) => {
                    // Revert the title so the session doesn't stay in a
                    // mixed state where the display name changed but the
                    // branch didn't.
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        session.title = previous_title;
                        session.updated_at = Utc::now();
                        let _ = self.session_store.upsert_session(session);
                    }
                    EventReaction::Multi(vec![
                        EventReaction::RebuildLeftItems,
                        EventReaction::Status(StatusUpdate::error(format!(
                            "Branch rename failed, reverted agent name: {e}"
                        ))),
                    ])
                }
            },
            WorkerEvent::BranchSyncReady(updates) => {
                let mut changed = false;
                for (session_id, actual_branch) in updates {
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id)
                        && session.branch_name != actual_branch
                    {
                        logger::info(&format!(
                            "branch sync: session {} branch changed {} -> {}",
                            session_id, session.branch_name, actual_branch,
                        ));
                        session.branch_name = actual_branch;
                        session.updated_at = Utc::now();
                        let _ = self.session_store.upsert_session(session);
                        changed = true;
                    }
                }
                if changed {
                    self.update_branch_sync_sessions();
                    EventReaction::RebuildLeftItems
                } else {
                    EventReaction::Nothing
                }
            }
            WorkerEvent::GhStatusChecked(status) => {
                self.gh_status = status;
                if matches!(status, GhStatus::Available) && self.github_integration_enabled {
                    logger::info("[gh-integration] gh CLI is available and authenticated");
                    self.update_pr_sync_sessions();
                    self.spawn_refs_watcher();
                    EventReaction::SpawnPrSyncWorkers
                } else {
                    logger::info(&format!(
                        "[gh-integration] gh status: {:?}, integration enabled: {}",
                        status, self.github_integration_enabled,
                    ));
                    EventReaction::Nothing
                }
            }
            WorkerEvent::PrStatusReady(results) => {
                let mut changed = false;
                let mut updated_ids = Vec::with_capacity(results.len());
                for (session_id, maybe_pr) in results {
                    updated_ids.push(session_id.clone());
                    match maybe_pr {
                        Some(pr) => {
                            // Persist the PR association (including state) so
                            // it survives restarts and squash-merge branch
                            // deletions.
                            let state_str = match pr.state {
                                PrState::Open => "OPEN",
                                PrState::Merged => "MERGED",
                                PrState::Closed => "CLOSED",
                            };
                            let _ = self.session_store.upsert_pr(&StoredPr {
                                session_id: session_id.clone(),
                                pr_number: pr.number,
                                host: pr.host.clone(),
                                owner_repo: pr.owner_repo.clone(),
                                state: state_str.to_string(),
                                title: pr.title.clone(),
                                url: pr.url.clone(),
                            });
                            self.pr_statuses.insert(session_id, pr);
                            changed = true;
                        }
                        None => {
                            if self.pr_statuses.remove(&session_id).is_some() {
                                changed = true;
                            }
                        }
                    }
                }
                if changed {
                    // Refresh the sync entries so the worker has updated
                    // known_pr data.
                    self.update_pr_sync_sessions();
                    EventReaction::Multi(vec![
                        EventReaction::UpdatePrLastChecked(updated_ids),
                        EventReaction::RebuildLeftItems,
                    ])
                } else {
                    EventReaction::UpdatePrLastChecked(updated_ids)
                }
            }
            WorkerEvent::PullRequestResolved { result } => match result {
                Ok(pr) => EventReaction::OpenNewAgentPromptForPr(Box::new(pr)),
                Err(message) => EventReaction::Status(StatusUpdate::error(message)),
            },
            WorkerEvent::RefsChanged(session_id) => {
                logger::debug(&format!(
                    "[gh-integration] refs watcher: triggering PR check for session {}",
                    session_id,
                ));
                EventReaction::SpawnPrCheckForSession(session_id)
            }
            WorkerEvent::BrowserEntriesReady { dir, entries } => {
                EventReaction::BrowserEntriesArrived { dir, entries }
            }
            WorkerEvent::ProjectWorktreesReady { project_id, result } => {
                EventReaction::ProjectWorktreesArrived { project_id, result }
            }
            WorkerEvent::WorktreeRemoveCompleted { session_id, result } => {
                // Always clear the in-flight guard so the session is
                // interactive again — whether we're about to remove it
                // (Ok path) or leave it in place for retry (Err path).
                self.pending_deletions.remove(&session_id);

                // Retrieve (and remove) the exact Busy message we set when
                // the worker was spawned. The App compares this against the
                // current status-line content rather than checking tone
                // alone, because another operation (push, pull, refresh,
                // concurrent delete) may have since set its own Busy message
                // that must not be clobbered.
                let our_busy_msg = self.deletion_busy_messages.remove(&session_id);

                match result {
                    Ok(branch_already_deleted) => EventReaction::WorktreeRemoveSucceeded {
                        session_id,
                        branch_already_deleted,
                        our_busy_message: our_busy_msg,
                    },
                    Err(msg) => EventReaction::WorktreeRemoveFailed {
                        session_id,
                        message: msg,
                    },
                }
            }
            WorkerEvent::ResourceStatsReady(stats) => {
                self.resource_stats_in_flight = false;
                EventReaction::ResourceStatsArrived(stats)
            }
            WorkerEvent::NonDefaultBranchCheckoutCompleted {
                action,
                target_branch,
                result,
            } => match result {
                Ok(()) => match action {
                    NonDefaultBranchAction::AddProject {
                        path,
                        name,
                        leading_branch,
                    } => EventReaction::AddProjectAfterBranchCheckout {
                        path,
                        name,
                        target_branch,
                        leading_branch,
                    },
                    NonDefaultBranchAction::CheckoutProjectDefault { project } => {
                        if let Some(existing) =
                            self.projects.iter_mut().find(|p| p.id == project.id)
                        {
                            existing.current_branch = target_branch.clone();
                            existing.branch_status = ProjectBranchStatus::Leading;
                        }
                        EventReaction::Status(StatusUpdate::info(format!(
                            "Checked out \"{target_branch}\" for project \"{}\".",
                            project.name
                        )))
                    }
                },
                Err(err) => {
                    // Preserve the full git stderr in the log so debugging
                    // stays possible after the status line summary is
                    // overwritten by the next message.
                    let path = action.repo_path().to_string();
                    logger::error(&format!(
                        "non-default branch checkout failed for {path}: {err}"
                    ));
                    EventReaction::Status(StatusUpdate::error(format!(
                        "Couldn't check out \"{target_branch}\" in {path} — resolve in your terminal and retry."
                    )))
                }
            },
            WorkerEvent::CreateAgentBranchInspected { project, result } => match result {
                Ok(inspection) => {
                    if let Some(existing) = self.projects.iter_mut().find(|p| p.id == project.id) {
                        existing.current_branch = inspection.current_branch.clone();
                        existing.leading_branch = Some(inspection.leading_branch.clone());
                        existing.branch_status =
                            if existing.current_branch == inspection.leading_branch {
                                ProjectBranchStatus::Leading
                            } else {
                                ProjectBranchStatus::NotLeading
                            };
                    }
                    EventReaction::ContinueCreateAgentAfterInspection {
                        project,
                        inspection,
                    }
                }
                Err(err) => EventReaction::Status(StatusUpdate::error(err)),
            },
            WorkerEvent::ProjectBranchStatusReady { project_id, result } => match result {
                Ok((current_branch, branch_status)) => {
                    if let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) {
                        project.current_branch = current_branch;
                        project.branch_status = branch_status;
                    }
                    EventReaction::Nothing
                }
                Err(err) => {
                    logger::debug(&format!(
                        "project branch status inspection failed for {project_id}: {err}"
                    ));
                    EventReaction::Nothing
                }
            },
            WorkerEvent::CheckoutProjectDefaultBranchInspected { project, result } => {
                match result {
                    Ok((current_branch, warning_kind)) => match warning_kind {
                        Some(BranchWarningKind::Known { default_branch }) => {
                            let mut project = project;
                            project.current_branch = current_branch;
                            EventReaction::DispatchProjectDefaultBranchCheckout {
                                project,
                                default_branch,
                            }
                        }
                        Some(BranchWarningKind::Heuristic) => {
                            EventReaction::Status(StatusUpdate::error(format!(
                                "Can't determine the default branch for project \"{}\" while it is on \"{}\". Resolve the default branch in your terminal and retry.",
                                project.name, current_branch
                            )))
                        }
                        None => {
                            if let Some(existing) =
                                self.projects.iter_mut().find(|p| p.id == project.id)
                            {
                                existing.current_branch = current_branch.clone();
                                existing.branch_status = ProjectBranchStatus::Leading;
                            }
                            EventReaction::Status(StatusUpdate::info(format!(
                                "Project \"{}\" is already on the leading branch \"{}\".",
                                project.name, current_branch
                            )))
                        }
                    },
                    Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                        "Couldn't inspect the default branch for project \"{}\": {err}",
                        project.name
                    ))),
                }
            }
            WorkerEvent::ConfigReloadReady(result) => match *result {
                Ok(config) => EventReaction::ApplyReloadedConfig(Box::new(config)),
                Err(message) => EventReaction::OpenConfigReloadFailedModal(message),
            },
            WorkerEvent::ConfigRecoverCompleted(result) => match result {
                Ok(()) => EventReaction::Status(StatusUpdate::info(
                    "Restored the last working configuration to config.toml.",
                )),
                Err(message) => EventReaction::Status(StatusUpdate::error(format!(
                    "Couldn't restore the last working configuration: {message}"
                ))),
            },
            WorkerEvent::ProjectPersistenceCompleted { action, result } => {
                EventReaction::ProjectPersistenceCompleted { action, result }
            }
            WorkerEvent::GlobalEnvPersistenceCompleted { env, result } => match result {
                Ok(()) => {
                    self.config.env = env;
                    if self.config.env.is_empty() {
                        EventReaction::Status(StatusUpdate::info(
                            "Global environment variables cleared.",
                        ))
                    } else {
                        EventReaction::Status(StatusUpdate::info(format!(
                            "Saved {} global environment variable(s). New agents and terminals will receive them unless a project overrides the same key.",
                            self.config.env.len()
                        )))
                    }
                }
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Could not save global environment variables to config.toml: {err}"
                ))),
            },
            WorkerEvent::StartupCommandRerunCompleted(result) => match result.status {
                Ok(()) => EventReaction::StartupCommandSucceeded {
                    project_name: result.project_name,
                },
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Startup command failed for project \"{}\": {err}. Run read-startup-command-logs for details.",
                    result.project_name
                ))),
            },
            WorkerEvent::StartupCommandLogsLoaded {
                scope_label,
                result,
            } => match result {
                Ok(log) => EventReaction::StartupLogArrived { scope_label, log },
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Could not read startup command logs for {scope_label}: {err}"
                ))),
            },
            WorkerEvent::OpenPathCompleted { target, result } => match result {
                Ok(()) => EventReaction::Status(StatusUpdate::info(format!("Opened {target}."))),
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Could not open {target}: {err}"
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuxPaths;
    use crate::lockfile::SingleInstanceLock;
    use crate::model::{
        AgentSession, GhStatus, PrInfo, PrState, Project, ProjectBranchStatus, ProviderKind,
        SessionStatus,
    };
    use crate::storage::SessionStore;
    use crate::worker::{
        AgentLaunchFailedData, AgentLaunchKind, AgentLaunchRequest, PullTarget, WorkerEvent,
    };
    use chrono::Utc;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// Construct a minimally-wired `Engine` for tests, alongside the `TempDir`
    /// that backs its on-disk state (sqlite, lockfile). Keep the `TempDir`
    /// alive for the lifetime of the test so it is cleaned up afterwards.
    fn test_engine() -> (Engine, TempDir) {
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
            create_agent_in_flight: false,
            agent_launches_in_flight: HashSet::new(),
            pulls_in_flight: HashSet::new(),
            resource_stats_in_flight: false,
        };
        (engine, tmp)
    }

    fn sample_project(id: &str, path: &str) -> Project {
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
        }
    }

    fn sample_session(id: &str, project_id: &str, branch: &str) -> AgentSession {
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

    fn unwrap_status(reaction: EventReaction) -> StatusUpdate {
        match reaction {
            EventReaction::Status(s) => s,
            other => panic!("expected Status reaction, got {:?}", reaction_kind(&other)),
        }
    }

    fn reaction_kind(r: &EventReaction) -> &'static str {
        match r {
            EventReaction::Nothing => "Nothing",
            EventReaction::Status(_) => "Status",
            EventReaction::Multi(_) => "Multi",
            EventReaction::RebuildLeftItems => "RebuildLeftItems",
            EventReaction::ReloadChangedFiles => "ReloadChangedFiles",
            EventReaction::ClampFilesCursor => "ClampFilesCursor",
            EventReaction::AgentLaunchReady(_) => "AgentLaunchReady",
            EventReaction::AgentLaunchFailed(_) => "AgentLaunchFailed",
            EventReaction::CommitMessageGenerated(_) => "CommitMessageGenerated",
            EventReaction::CommitMessageFailed(_) => "CommitMessageFailed",
            EventReaction::BrowserEntriesArrived { .. } => "BrowserEntriesArrived",
            EventReaction::ProjectWorktreesArrived { .. } => "ProjectWorktreesArrived",
            EventReaction::UpdatePrLastChecked(_) => "UpdatePrLastChecked",
            EventReaction::SpawnPrCheckForSession(_) => "SpawnPrCheckForSession",
            EventReaction::OpenNewAgentPromptForPr(_) => "OpenNewAgentPromptForPr",
            EventReaction::SpawnPrSyncWorkers => "SpawnPrSyncWorkers",
            EventReaction::WorktreeRemoveSucceeded { .. } => "WorktreeRemoveSucceeded",
            EventReaction::WorktreeRemoveFailed { .. } => "WorktreeRemoveFailed",
            EventReaction::ResourceStatsArrived(_) => "ResourceStatsArrived",
            EventReaction::AddProjectAfterBranchCheckout { .. } => "AddProjectAfterBranchCheckout",
            EventReaction::ContinueCreateAgentAfterInspection { .. } => {
                "ContinueCreateAgentAfterInspection"
            }
            EventReaction::DispatchProjectDefaultBranchCheckout { .. } => {
                "DispatchProjectDefaultBranchCheckout"
            }
            EventReaction::ApplyReloadedConfig(_) => "ApplyReloadedConfig",
            EventReaction::OpenConfigReloadFailedModal(_) => "OpenConfigReloadFailedModal",
            EventReaction::ProjectPersistenceCompleted { .. } => "ProjectPersistenceCompleted",
            EventReaction::StartupCommandSucceeded { .. } => "StartupCommandSucceeded",
            EventReaction::StartupLogArrived { .. } => "StartupLogArrived",
        }
    }

    // ── PullCompleted (Project) ──────────────────────────────────────────

    #[test]
    fn pull_completed_project_ok_updates_branch_and_clears_inflight() {
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        engine.projects.push(project);
        let repo_path = "/tmp/p1".to_string();
        engine.pulls_in_flight.insert(repo_path.clone());

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: Some("main".to_string()),
            },
            result: Ok(Some("feature-x".to_string())),
        });

        // In-flight entry is cleared regardless of result.
        assert!(!engine.pulls_in_flight.contains(&repo_path));

        // Project's current branch is updated; status is NotLeading because
        // leading_branch is Some("main") and current_branch is "feature-x".
        let p = &engine.projects[0];
        assert_eq!(p.current_branch, "feature-x");
        assert_eq!(p.branch_status, ProjectBranchStatus::NotLeading);

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            status.message,
            "Refreshed project \"p1-name\". Local branch is up to date with remote."
        );
    }

    #[test]
    fn pull_completed_project_err_still_clears_inflight() {
        let (mut engine, _tmp) = test_engine();
        let repo_path = "/tmp/p1".to_string();
        engine.pulls_in_flight.insert(repo_path.clone());

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: None,
            },
            result: Err("network down".to_string()),
        });

        assert!(!engine.pulls_in_flight.contains(&repo_path));
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(
            status.message,
            "Project refresh failed for \"p1-name\": network down"
        );
    }

    // ── BranchSyncReady ──────────────────────────────────────────────────

    #[test]
    fn branch_sync_ready_changed_branch_returns_rebuild() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "old"));
        let before_updated_at = engine.sessions[0].updated_at;

        let reaction = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "new".to_string(),
        )]));

        assert!(matches!(reaction, EventReaction::RebuildLeftItems));
        let s = &engine.sessions[0];
        assert_eq!(s.branch_name, "new");
        assert!(s.updated_at >= before_updated_at);

        // Verify the upsert hit the session store.
        let loaded = engine.session_store.load_sessions().expect("load");
        let stored = loaded.iter().find(|s| s.id == "s1").expect("stored s1");
        assert_eq!(stored.branch_name, "new");
    }

    #[test]
    fn branch_sync_ready_no_change_returns_nothing() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "same"));

        let reaction = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "same".to_string(),
        )]));

        assert!(matches!(reaction, EventReaction::Nothing));
        // Session store should not contain "s1" since no upsert happened.
        let loaded = engine.session_store.load_sessions().expect("load");
        assert!(loaded.iter().all(|s| s.id != "s1"));
    }

    // ── PrStatusReady ────────────────────────────────────────────────────

    #[test]
    fn pr_status_ready_with_pr_upserts_and_returns_all_ids() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        // Persist the session row first so the session_prs foreign key
        // constraint on session_id is satisfied when upsert_pr fires from
        // the dispatcher.
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);

        let pr = PrInfo {
            number: 42,
            state: PrState::Open,
            title: "Add feature".to_string(),
            host: "github.com".to_string(),
            owner_repo: "octo/repo".to_string(),
            url: "https://github.com/octo/repo/pull/42".to_string(),
        };
        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "s1".to_string(),
            Some(pr.clone()),
        )]));

        // changed -> outer is Multi(UpdatePrLastChecked, RebuildLeftItems).
        let parts = match reaction {
            EventReaction::Multi(v) => v,
            other => panic!("expected Multi, got {}", reaction_kind(&other)),
        };
        assert_eq!(parts.len(), 2);
        match &parts[0] {
            EventReaction::UpdatePrLastChecked(ids) => {
                assert_eq!(ids, &vec!["s1".to_string()]);
            }
            other => panic!("expected UpdatePrLastChecked, got {}", reaction_kind(other)),
        }
        assert!(matches!(parts[1], EventReaction::RebuildLeftItems));

        // pr_statuses populated; sqlite has the row.
        assert!(engine.pr_statuses.contains_key("s1"));
        let stored = engine
            .session_store
            .load_all_latest_prs()
            .expect("load prs");
        let row = stored.iter().find(|p| p.session_id == "s1").expect("row");
        assert_eq!(row.pr_number, 42);
        assert_eq!(row.state, "OPEN");
        assert_eq!(row.title, "Add feature");
    }

    #[test]
    fn pr_status_ready_none_removes_and_returns_ids_even_when_unchanged() {
        let (mut engine, _tmp) = test_engine();
        // Pre-seed pr_statuses for s1 so the None path actually removes
        // something (and flips `changed`). s2 has no PR — None for s2 leaves
        // `changed` alone for s2 but its id must still appear in the
        // UpdatePrLastChecked list.
        let pr = PrInfo {
            number: 1,
            state: PrState::Open,
            title: "x".into(),
            host: "github.com".into(),
            owner_repo: "o/r".into(),
            url: "https://example".into(),
        };
        engine.pr_statuses.insert("s1".to_string(), pr);

        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![
            ("s1".to_string(), None),
            ("s2".to_string(), None),
        ]));

        // s1 was removed -> changed -> Multi
        let parts = match reaction {
            EventReaction::Multi(v) => v,
            other => panic!("expected Multi, got {}", reaction_kind(&other)),
        };
        match &parts[0] {
            EventReaction::UpdatePrLastChecked(ids) => {
                assert_eq!(
                    ids,
                    &vec!["s1".to_string(), "s2".to_string()],
                    "every session id from results must appear, even those with no state change"
                );
            }
            other => panic!("expected UpdatePrLastChecked, got {}", reaction_kind(other)),
        }
        assert!(matches!(parts[1], EventReaction::RebuildLeftItems));
        assert!(!engine.pr_statuses.contains_key("s1"));
    }

    #[test]
    fn pr_status_ready_unchanged_only_returns_update_pr_last_checked() {
        let (mut engine, _tmp) = test_engine();
        // No pre-seeded pr_statuses; sending None for s1 leaves changed=false.
        let reaction =
            engine.process_worker_event(WorkerEvent::PrStatusReady(vec![("s1".to_string(), None)]));

        match reaction {
            EventReaction::UpdatePrLastChecked(ids) => {
                assert_eq!(ids, vec!["s1".to_string()]);
            }
            other => panic!(
                "expected bare UpdatePrLastChecked (no Multi wrapping), got {}",
                reaction_kind(&other)
            ),
        }
    }

    // ── WorktreeRemoveCompleted ──────────────────────────────────────────

    #[test]
    fn worktree_remove_completed_ok_clears_state_and_returns_busy_message() {
        let (mut engine, _tmp) = test_engine();
        engine.pending_deletions.insert("s1".to_string());
        engine
            .deletion_busy_messages
            .insert("s1".to_string(), "Deleting agent \"s1\"…".to_string());

        let reaction = engine.process_worker_event(WorkerEvent::WorktreeRemoveCompleted {
            session_id: "s1".to_string(),
            result: Ok(true),
        });

        assert!(!engine.pending_deletions.contains("s1"));
        assert!(!engine.deletion_busy_messages.contains_key("s1"));

        match reaction {
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branch_already_deleted,
                our_busy_message,
            } => {
                assert_eq!(session_id, "s1");
                assert!(branch_already_deleted);
                assert_eq!(our_busy_message.as_deref(), Some("Deleting agent \"s1\"…"));
            }
            other => panic!(
                "expected WorktreeRemoveSucceeded, got {}",
                reaction_kind(&other)
            ),
        }
    }

    #[test]
    fn worktree_remove_completed_err_still_clears_state() {
        let (mut engine, _tmp) = test_engine();
        engine.pending_deletions.insert("s1".to_string());
        engine
            .deletion_busy_messages
            .insert("s1".to_string(), "busy".to_string());

        let reaction = engine.process_worker_event(WorkerEvent::WorktreeRemoveCompleted {
            session_id: "s1".to_string(),
            result: Err("git failed".to_string()),
        });

        // Even on Err, both maps must be cleaned up.
        assert!(!engine.pending_deletions.contains("s1"));
        assert!(!engine.deletion_busy_messages.contains_key("s1"));

        match reaction {
            EventReaction::WorktreeRemoveFailed {
                session_id,
                message,
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(message, "git failed");
            }
            other => panic!(
                "expected WorktreeRemoveFailed, got {}",
                reaction_kind(&other)
            ),
        }
    }

    // ── CreateAgentFailed ────────────────────────────────────────────────

    #[test]
    fn create_agent_failed_flips_inflight_and_returns_error_status() {
        let (mut engine, _tmp) = test_engine();
        engine.create_agent_in_flight = true;

        let reaction =
            engine.process_worker_event(WorkerEvent::CreateAgentFailed("nope".to_string()));

        assert!(!engine.create_agent_in_flight);
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.message, "nope");
    }

    // ── GlobalEnvPersistenceCompleted ────────────────────────────────────

    #[test]
    fn global_env_persistence_completed_ok_empty_clears_and_reports() {
        let (mut engine, _tmp) = test_engine();
        // Seed a non-empty env to prove it gets replaced by the empty map.
        let mut prior = BTreeMap::new();
        prior.insert("OLD".to_string(), "1".to_string());
        engine.config.env = prior;

        let reaction = engine.process_worker_event(WorkerEvent::GlobalEnvPersistenceCompleted {
            env: BTreeMap::new(),
            result: Ok(()),
        });

        assert!(engine.config.env.is_empty());
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(status.message, "Global environment variables cleared.");
    }

    #[test]
    fn global_env_persistence_completed_ok_nonempty_replaces_env() {
        let (mut engine, _tmp) = test_engine();
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

        let reaction = engine.process_worker_event(WorkerEvent::GlobalEnvPersistenceCompleted {
            env: env.clone(),
            result: Ok(()),
        });

        assert_eq!(engine.config.env, env);
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            status.message,
            "Saved 2 global environment variable(s). New agents and terminals will receive them unless a project overrides the same key."
        );
    }

    // Sanity: the unused-import linter won't catch AgentLaunchFailedData
    // because we reference it via a no-op assertion to prove the test module
    // compiles against the same shape the dispatcher uses.
    #[allow(dead_code)]
    fn _agent_launch_failed_shape_compiles(req: AgentLaunchRequest, msg: String) {
        let _boxed = Box::new(AgentLaunchFailedData {
            request: req,
            message: msg,
        });
        let _kind = AgentLaunchKind::StartupAutoReopen;
    }
}
