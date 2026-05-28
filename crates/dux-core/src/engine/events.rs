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
                                    let warning = engine_branch_warning_kind(
                                        Path::new(&existing.path),
                                        &existing.current_branch,
                                    );
                                    engine_branch_status_from_warning(warning.as_ref())
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

// Local copies of the branch-warning helpers from `src/app/mod.rs`. We
// duplicate the logic here (a few lines) instead of moving the App helpers
// into dux-core so that the `PullCompleted Project` arm can compute branch
// status without touching App code.
fn engine_branch_warning_kind(path: &Path, branch: &str) -> Option<BranchWarningKind> {
    match crate::git::remote_default_branch(path) {
        Some(default) if default != branch => Some(BranchWarningKind::Known {
            default_branch: default,
        }),
        Some(_) => None,
        None if branch != "main" && branch != "master" => Some(BranchWarningKind::Heuristic),
        None => None,
    }
}

fn engine_branch_status_from_warning(
    warning_kind: Option<&BranchWarningKind>,
) -> ProjectBranchStatus {
    match warning_kind {
        Some(_) => ProjectBranchStatus::NotLeading,
        None => ProjectBranchStatus::Leading,
    }
}
