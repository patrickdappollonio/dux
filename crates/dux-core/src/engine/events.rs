//! Engine-side dispatch for `WorkerEvent`s. `Engine::process_worker_event`
//! performs the domain-state mutation for an event and returns an
//! `EventReaction` that tells the App caller what view follow-up to perform.
//!
//! The Engine MUST NOT touch view state (status line, prompt, focus, derived
//! caches like `left_items_cache` or `files_index`). Anything view-side is
//! described by an `EventReaction` variant; the App's `apply_reaction`
//! translates each variant back into concrete view mutations.

use std::path::Path;
use std::time::Instant;

use chrono::Utc;

use crate::config::Config;
use crate::engine::Engine;
use crate::logger;
use crate::model::{
    AgentSession, GhStatus, PrState, Project, ProjectBranchStatus, ProviderKind, SessionStatus,
};
use crate::startup::StartupCommandLatestLog;
use crate::statusline::StatusTone;
use crate::storage::StoredPr;
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchKind, AgentLaunchReadyData, BranchWarningKind, BrowserEntry,
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

    // -- Agent launch (T2b: Engine performs all domain-state mutations and
    //    returns a typed view-only outcome the App applies). --
    AgentLaunchReadyView(Box<AgentLaunchReadyOutcome>),
    AgentLaunchFailedView(Box<AgentLaunchFailedOutcome>),

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

    // -- PR / refs follow-ups. --
    OpenNewAgentPromptForPr(Box<ResolvedPullRequest>),

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

    // -- Deletion `Command` view follow-ups (E4a). --
    FinishDeleteSessionView(Box<FinishDeleteSessionView>),
    DoDeleteSessionView(Box<DoDeleteSessionView>),
    BeginDeleteSessionView(Box<BeginDeleteSessionView>),

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

    // -- Project persistence (App applies view follow-up; Engine performed mutations). --
    ProjectPersistenceOutcome(Box<ProjectPersistenceOutcome>),

    // -- Startup command / log viewer (App formats key + opens overlay). --
    StartupCommandSucceeded {
        project_name: String,
    },
    StartupLogArrived {
        scope_label: String,
        log: StartupCommandLatestLog,
    },
}

/// Result of `Engine::detach_conflicting_worktree_session` — the App caller
/// uses `id` to clear `last_pty_activity` and `label` for status messages.
#[derive(Clone, Debug)]
pub struct DetachedSession {
    pub id: String,
    pub label: String,
}

/// View-only follow-up for `WorkerEvent::AgentLaunchReady`. The Engine has
/// already performed all domain-state mutations (in_flight maps, sessions,
/// providers, session_store, mark_session_* helpers, resume_fallback_*,
/// update_branch_sync_sessions, and the pure-engine portion of
/// detach_conflicting_worktree_session). The App applies `last_pty_size`,
/// clears `last_pty_activity` for any `detached_session_id`, runs view
/// rebuilds, sets surfaces/overlays/status.
pub struct AgentLaunchReadyOutcome {
    pub session: AgentSession,
    pub pty_size: (u16, u16),
    pub detached_session_id: Option<String>,
    pub view: AgentLaunchReadyView,
}

pub enum AgentLaunchReadyView {
    /// Create-kind launch: `session_store.upsert_session` failed before the
    /// session could be committed. App surfaces the error; no view rebuild.
    CreatePersistFailed { error: String },
    /// Create-kind launch committed. App rebuilds left items, selects the
    /// new session, reloads changed files, shows the agent surface, and
    /// surfaces either the startup-command error or the create status.
    CreateCommitted {
        status_message: String,
        startup_result_error: Option<String>,
    },
    /// Non-Create launch found the session vanished. App does nothing
    /// (Engine has already logged the "dropping launched PTY" line).
    SessionMissing,
    /// Reconnect / ForceReconnect: App shows the agent surface + sets info.
    Reconnect { status_message: String },
    /// ResumeFallback: App shows the agent surface only if `session_id` is
    /// the currently selected session, and always sets info.
    ResumeFallback {
        session_id: String,
        status_message: String,
    },
    /// StartupAutoReopen: App does nothing.
    StartupAutoReopen,
}

/// View-only follow-up for `WorkerEvent::AgentLaunchFailed`. Engine has
/// already cleared `agent_launches_in_flight`, flipped
/// `create_agent_in_flight` for Create-kind failures, logged the
/// ResumeFallback / StartupAutoReopen cases, and marked ResumeFallback
/// sessions Detached. The App only formats the status message.
pub enum AgentLaunchFailedOutcome {
    Create {
        message: String,
    },
    Reconnect {
        branch_name: String,
        message: String,
    },
    ForceReconnect {
        branch_name: String,
        message: String,
    },
    /// Engine logged + marked Detached; App has nothing to do.
    ResumeFallback,
    StartupAutoReopen {
        branch_name: String,
        message: String,
    },
}

/// Domain mutations the Engine performed in response to a
/// `ProjectPersistenceCompleted` worker event; carries everything the App
/// needs for view follow-up (rebuild_left_items, persist_config_projects_from_runtime,
/// reload_changed_files for Delete, selected_left adjustment, status).
///
/// The Engine never calls `persist_config_projects_from_runtime` because
/// that helper uses binary-only `RuntimeBindings` / `save_config` — it lives
/// on App until Phase E5 carves dux-tui.
pub struct ProjectPersistenceOutcome {
    pub action: ProjectPersistenceAction,
    pub view: ProjectPersistenceView,
}

pub enum ProjectPersistenceView {
    PersistenceFailed {
        error: String,
    },
    Added {
        project_id: String,
        status_message: String,
    },
    Removed {
        project_name: String,
    },
    Deleted {
        project_name: String,
    },
    DefaultProviderUpdated {
        project_name: String,
        provider: Option<ProviderKind>,
        global_default: ProviderKind,
    },
    AutoReopenUpdated {
        project_name: String,
        auto_reopen_agents: Option<bool>,
    },
    StartupCommandUpdated {
        project_name: String,
        startup_command: Option<String>,
    },
    EnvUpdated {
        project_name: String,
        env_count: usize,
    },
}

/// Result of `Engine::finish_delete_session`. Carries the deleted session
/// and project context the App needs to apply view follow-up
/// (`last_pty_activity` clear, `clear_companion_terminals_for_session`,
/// `rebuild_left_items`, `selected_left` adjustment, `reload_changed_files`)
/// and to format the 4-branch status message.
pub struct FinishDeleteSessionOutcome {
    pub session: AgentSession,
    pub project: Option<Project>,
    pub other_sessions_on_worktree: bool,
    pub project_still_has_sessions: bool,
}

/// Result of `Engine::do_delete_session`. Engine has performed the git
/// worktree removal (if needed) and the full finish-delete-session cascade
/// (store delete + providers/pins/resume_fallback removal + sessions retain
/// + branch-sync refresh); the App still has to apply view follow-up.
pub struct DoDeleteSessionOutcome {
    /// Finish-cascade outcome (same shape T3f-1 introduced).
    pub finish: FinishDeleteSessionOutcome,
    /// `Some(branch_already_deleted)` if Engine ran `git::remove_worktree`;
    /// `None` if the worktree was preserved because a sibling session shares
    /// it or `delete_worktree` was false. Drives the 4-case status formatting
    /// in `apply_finish_delete_session_outcome`.
    pub remove_outcome: Option<bool>,
}

/// Result of `Engine::begin_delete_session`. The four branches mirror the
/// original App method's control flow.
pub enum BeginDeleteSessionOutcome {
    /// `pending_deletions` already contains this session — App emits the
    /// "already in progress" error.
    AlreadyInFlight,
    /// Session or project lookup failed — silent no-op (preserves the
    /// original early-return behaviour).
    NotFound,
    /// Async path: Engine inserted into `pending_deletions`, spawned the
    /// `git::remove_worktree` worker (which posts `WorktreeRemoveCompleted`
    /// back), and stored `busy_message` in `deletion_busy_messages`. App
    /// only needs to set the status line.
    AsyncStarted { busy_message: String },
    /// Inline path: no worktree removal needed (no `delete_worktree` request
    /// or shared with siblings). App should call the existing
    /// `finish_delete_session(session_id, delete_worktree, None, true)`
    /// wrapper to complete cleanup + emit status.
    Inline,
}

/// View follow-up data for a `Command::FinishDeleteSession`. Wraps the
/// engine outcome with the App-context fields needed for status
/// formatting and the contract-violation check.
pub struct FinishDeleteSessionView {
    pub session_id: String,
    pub outcome: FinishDeleteSessionOutcome,
    pub delete_worktree: bool,
    pub remove_outcome: Option<bool>,
    pub update_status: bool,
}

/// View follow-up data for a `Command::DoDeleteSession`. Wraps the engine
/// outcome with the App-context fields needed for status formatting.
pub struct DoDeleteSessionView {
    pub session_id: String,
    pub outcome: DoDeleteSessionOutcome,
    pub delete_worktree: bool,
}

/// View follow-up data for a `Command::BeginDeleteSession`. Wraps the
/// engine outcome with the App-context fields needed for status
/// formatting and the inline cleanup follow-up.
pub struct BeginDeleteSessionView {
    pub session_id: String,
    pub outcome: BeginDeleteSessionOutcome,
    pub delete_worktree: bool,
}

/// Display name for a session — title if present, branch name otherwise.
/// (Engine-internal helper; the binary keeps `App::session_label` for the
/// ~8 view-side callers in `sessions.rs`.)
fn session_label(session: &AgentSession) -> String {
    session
        .title
        .clone()
        .unwrap_or_else(|| session.branch_name.clone())
}

impl Engine {
    /// Find any other session that owns `worktree_path` and currently has a
    /// running provider, and detach it so the incoming launch can take over.
    /// Returns the detached session's id + label so the App caller can clear
    /// `last_pty_activity` and surface the label in status messages.
    ///
    /// The App's `detach_conflicting_worktree_session` is a thin wrapper that
    /// also drops `last_pty_activity` for the returned id.
    pub fn detach_conflicting_worktree_session(
        &mut self,
        worktree_path: &str,
        exclude_id: &str,
    ) -> Option<DetachedSession> {
        let conflicting = self
            .sessions
            .iter()
            .find(|s| {
                s.id != exclude_id
                    && s.worktree_path == worktree_path
                    && self.providers.contains_key(&s.id)
            })
            .cloned()?;

        let label = session_label(&conflicting);
        let provider = conflicting.provider.as_str().to_string();
        self.providers.remove(&conflicting.id);
        self.running_provider_pins.remove(&conflicting.id);
        self.resume_fallback_candidates.remove(&conflicting.id);
        self.mark_session_status(&conflicting.id, SessionStatus::Detached);

        logger::info(&format!(
            "auto-detached {} agent \"{}\" to avoid worktree conflict",
            provider, label,
        ));
        Some(DetachedSession {
            id: conflicting.id,
            label,
        })
    }
}

impl Engine {
    pub fn process_agent_launch_ready(
        &mut self,
        data: AgentLaunchReadyData,
    ) -> AgentLaunchReadyOutcome {
        let AgentLaunchReadyData { request, client } = data;
        let session = request.session.clone();
        let pty_size = request.pty_size;
        self.agent_launches_in_flight.remove(&session.id);

        if matches!(request.kind, AgentLaunchKind::Create { .. }) {
            self.create_agent_in_flight = false;
            if let Err(err) = self.session_store.upsert_session(&session) {
                logger::error(&format!(
                    "session store upsert failed for {}: {err}",
                    session.id,
                ));
                return AgentLaunchReadyOutcome {
                    session,
                    pty_size,
                    detached_session_id: None,
                    view: AgentLaunchReadyView::CreatePersistFailed {
                        error: err.to_string(),
                    },
                };
            }
            let detached =
                self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);
            self.providers.insert(session.id.clone(), client);
            self.sessions.insert(0, session.clone());
            self.mark_session_provider_started(&session.id);
            if request.resume {
                self.resume_fallback_candidates
                    .insert(session.id.clone(), Instant::now());
            }
            self.update_branch_sync_sessions();

            // Extract Create-kind payload for the view outcome.
            let AgentLaunchKind::Create {
                status_message,
                startup_result,
                ..
            } = request.kind
            else {
                unreachable!("matched AgentLaunchKind::Create above")
            };
            let startup_result_error = startup_result.and_then(|r| r.status.err());

            return AgentLaunchReadyOutcome {
                session,
                pty_size,
                detached_session_id: detached.map(|d| d.id),
                view: AgentLaunchReadyView::CreateCommitted {
                    status_message,
                    startup_result_error,
                },
            };
        }

        // Non-Create branches share the "drop on missing session" guard.
        if !self.sessions.iter().any(|s| s.id == session.id) {
            logger::info(&format!(
                "dropping launched PTY for missing session {}",
                session.id,
            ));
            return AgentLaunchReadyOutcome {
                session,
                pty_size,
                detached_session_id: None,
                view: AgentLaunchReadyView::SessionMissing,
            };
        }

        let detached =
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);
        self.providers.insert(session.id.clone(), client);
        if request.resume {
            self.resume_fallback_candidates
                .insert(session.id.clone(), Instant::now());
        }
        self.mark_session_desired_running(&session.id, true);
        self.mark_session_status(&session.id, SessionStatus::Active);
        self.mark_session_provider_started(&session.id);

        let view = match request.kind {
            AgentLaunchKind::Reconnect { status_message }
            | AgentLaunchKind::ForceReconnect { status_message } => {
                AgentLaunchReadyView::Reconnect { status_message }
            }
            AgentLaunchKind::ResumeFallback { status_message } => {
                AgentLaunchReadyView::ResumeFallback {
                    session_id: session.id.clone(),
                    status_message,
                }
            }
            AgentLaunchKind::StartupAutoReopen => AgentLaunchReadyView::StartupAutoReopen,
            AgentLaunchKind::Create { .. } => unreachable!("create launch handled above"),
        };

        AgentLaunchReadyOutcome {
            session,
            pty_size,
            detached_session_id: detached.map(|d| d.id),
            view,
        }
    }

    pub fn process_project_persistence_completed(
        &mut self,
        action: ProjectPersistenceAction,
        result: Result<(), String>,
    ) -> ProjectPersistenceOutcome {
        if let Err(error) = result {
            return ProjectPersistenceOutcome {
                action,
                view: ProjectPersistenceView::PersistenceFailed { error },
            };
        }

        let view = match &action {
            ProjectPersistenceAction::Add {
                project,
                status_message,
            } => {
                let project_id = project.id.clone();
                self.projects.push(project.clone());
                ProjectPersistenceView::Added {
                    project_id,
                    status_message: status_message.clone(),
                }
            }
            ProjectPersistenceAction::Remove {
                project_id,
                project_name,
            } => {
                self.projects.retain(|p| p.id != *project_id);
                ProjectPersistenceView::Removed {
                    project_name: project_name.clone(),
                }
            }
            ProjectPersistenceAction::Delete {
                project_id,
                project_name,
            } => {
                self.projects.retain(|p| p.id != *project_id);
                ProjectPersistenceView::Deleted {
                    project_name: project_name.clone(),
                }
            }
            ProjectPersistenceAction::UpdateDefaultProvider {
                project_id,
                project_name,
                provider,
                global_default,
            } => {
                if let Some(project) = self.projects.iter_mut().find(|p| p.id == *project_id) {
                    project.explicit_default_provider = provider.clone();
                }
                self.refresh_project_defaults();
                ProjectPersistenceView::DefaultProviderUpdated {
                    project_name: project_name.clone(),
                    provider: provider.clone(),
                    global_default: global_default.clone(),
                }
            }
            ProjectPersistenceAction::UpdateAutoReopen {
                project_id,
                project_name,
                auto_reopen_agents,
            } => {
                if let Some(project) = self.projects.iter_mut().find(|p| p.id == *project_id) {
                    project.auto_reopen_agents = *auto_reopen_agents;
                }
                ProjectPersistenceView::AutoReopenUpdated {
                    project_name: project_name.clone(),
                    auto_reopen_agents: *auto_reopen_agents,
                }
            }
            ProjectPersistenceAction::UpdateStartupCommand {
                project_id,
                project_name,
                startup_command,
            } => {
                if let Some(project) = self.projects.iter_mut().find(|p| p.id == *project_id) {
                    project.startup_command = startup_command.clone();
                }
                ProjectPersistenceView::StartupCommandUpdated {
                    project_name: project_name.clone(),
                    startup_command: startup_command.clone(),
                }
            }
            ProjectPersistenceAction::UpdateEnv {
                project_id,
                project_name,
                env,
            } => {
                if let Some(project) = self.projects.iter_mut().find(|p| p.id == *project_id) {
                    project.env = env.clone();
                }
                let env_count = env.len();
                ProjectPersistenceView::EnvUpdated {
                    project_name: project_name.clone(),
                    env_count,
                }
            }
        };

        ProjectPersistenceOutcome { action, view }
    }

    /// Engine half of the session-deletion cascade. Removes the session from
    /// the store + providers + runtime maps + the sessions vector; refreshes
    /// branch-sync entries; spawns the startup-log deletion worker. Returns
    /// `Ok(None)` if the session was already gone; `Ok(Some(outcome))` with
    /// the context the App needs for its view-side follow-up; `Err` on a
    /// store failure (in-memory state untouched in that case so the UI keeps
    /// showing the session).
    pub fn finish_delete_session(
        &mut self,
        session_id: &str,
    ) -> anyhow::Result<Option<FinishDeleteSessionOutcome>> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return Ok(None);
        };
        let project = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned();
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);

        // Persist the deletion FIRST so a DB failure leaves in-memory state
        // untouched and the session remains visible in the UI. If we cleared
        // in-memory state first and the DB call then failed, the session
        // would vanish from the UI but reappear on restart.
        self.session_store.delete_session(&session.id)?;
        crate::startup::spawn_delete_startup_command_logs(
            self.paths.clone(),
            session.project_id.clone(),
            session.id.clone(),
        );

        self.providers.remove(&session.id);
        self.running_provider_pins.remove(&session.id);
        self.resume_fallback_candidates.remove(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.update_branch_sync_sessions();

        let project_still_has_sessions = self
            .sessions
            .iter()
            .any(|candidate| candidate.project_id == session.project_id);

        Ok(Some(FinishDeleteSessionOutcome {
            session,
            project,
            other_sessions_on_worktree,
            project_still_has_sessions,
        }))
    }

    /// Synchronous engine half of "delete this session" — looks up the session
    /// and project, optionally calls `git::remove_worktree`, then runs the full
    /// `finish_delete_session` cascade.
    ///
    /// Returns `Ok(None)` if the session was already gone or the project record
    /// is missing; `Ok(Some(outcome))` otherwise; `Err` if
    /// `git::remove_worktree` or `session_store.delete_session` fails.
    ///
    /// Callers must ensure no async worker is already removing this worktree
    /// (`pending_deletions` should not contain `session_id`). The debug_assert
    /// surfaces the violation in debug builds; release builds race silently.
    pub fn do_delete_session(
        &mut self,
        session_id: &str,
        delete_worktree: bool,
    ) -> anyhow::Result<Option<DoDeleteSessionOutcome>> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return Ok(None);
        };
        logger::info(&format!(
            "deleting session {} at {} (delete_worktree={}, sync)",
            session.id, session.worktree_path, delete_worktree
        ));
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned()
        else {
            return Ok(None);
        };
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);

        let should_remove_worktree = delete_worktree && !other_sessions_on_worktree;

        debug_assert!(
            !self.pending_deletions.contains(session_id),
            "do_delete_session called while an async delete worker is in-flight for {}",
            session_id,
        );
        let remove_outcome = if should_remove_worktree {
            let result = crate::git::remove_worktree(
                std::path::Path::new(&project.path),
                std::path::Path::new(&session.worktree_path),
                &session.branch_name,
            )?;
            Some(result.branch_already_deleted)
        } else {
            None
        };

        let Some(finish) = self.finish_delete_session(session_id)? else {
            // Should be unreachable — we just confirmed the session exists
            // above — but if a concurrent path removed it, treat as no-op.
            return Ok(None);
        };
        Ok(Some(DoDeleteSessionOutcome {
            finish,
            remove_outcome,
        }))
    }

    /// Engine half of the modal "begin delete" action. Branches between the
    /// async path (spawns `git::remove_worktree` worker, posts
    /// `WorktreeRemoveCompleted` back to `worker_tx`) and the inline path
    /// (lets the App caller invoke `finish_delete_session` synchronously).
    /// Never returns `Err` — failures route through the worker callback.
    pub fn begin_delete_session(
        &mut self,
        session_id: &str,
        delete_worktree: bool,
    ) -> BeginDeleteSessionOutcome {
        if self.pending_deletions.contains(session_id) {
            return BeginDeleteSessionOutcome::AlreadyInFlight;
        }

        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return BeginDeleteSessionOutcome::NotFound;
        };
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned()
        else {
            return BeginDeleteSessionOutcome::NotFound;
        };
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);
        let should_remove_worktree = delete_worktree && !other_sessions_on_worktree;

        if should_remove_worktree {
            logger::info(&format!(
                "deleting session {} at {} (delete_worktree=true, async)",
                session.id, session.worktree_path
            ));
            // Mark in-flight BEFORE spawning so a fast follow-up action from
            // the same event loop tick can see the guard. The worker event
            // handler clears the entry on completion (Ok or Err).
            self.pending_deletions.insert(session.id.clone());
            let sid = session.id.clone();
            let project_path = project.path.clone();
            let worktree_path = session.worktree_path.clone();
            let branch_name = session.branch_name.clone();
            let tx = self.worker_tx.clone();
            std::thread::spawn(move || {
                let result = crate::git::remove_worktree(
                    std::path::Path::new(&project_path),
                    std::path::Path::new(&worktree_path),
                    &branch_name,
                )
                .map(|r| r.branch_already_deleted)
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(crate::worker::WorkerEvent::WorktreeRemoveCompleted {
                    session_id: sid,
                    result,
                });
            });
            let busy_message = format!(
                "Removing worktree for agent \"{}\"\u{2026}",
                session.branch_name
            );
            self.deletion_busy_messages
                .insert(session.id.clone(), busy_message.clone());
            BeginDeleteSessionOutcome::AsyncStarted { busy_message }
        } else {
            logger::info(&format!(
                "deleting session {} at {} (delete_worktree={}, inline)",
                session.id, session.worktree_path, delete_worktree
            ));
            BeginDeleteSessionOutcome::Inline
        }
    }

    pub fn process_agent_launch_failed(
        &mut self,
        data: AgentLaunchFailedData,
    ) -> AgentLaunchFailedOutcome {
        let AgentLaunchFailedData { request, message } = data;
        let session = request.session;
        self.agent_launches_in_flight.remove(&session.id);

        match request.kind {
            AgentLaunchKind::Create { .. } => {
                self.create_agent_in_flight = false;
                AgentLaunchFailedOutcome::Create { message }
            }
            AgentLaunchKind::Reconnect { .. } => AgentLaunchFailedOutcome::Reconnect {
                branch_name: session.branch_name,
                message,
            },
            AgentLaunchKind::ForceReconnect { .. } => AgentLaunchFailedOutcome::ForceReconnect {
                branch_name: session.branch_name,
                message,
            },
            AgentLaunchKind::ResumeFallback { .. } => {
                logger::error(&format!(
                    "fallback PTY spawn failed for {}: {}",
                    session.id, message,
                ));
                self.mark_session_status(&session.id, SessionStatus::Detached);
                AgentLaunchFailedOutcome::ResumeFallback
            }
            AgentLaunchKind::StartupAutoReopen => {
                logger::error(&format!(
                    "startup auto-reopen failed for agent \"{}\": {}",
                    session.branch_name, message,
                ));
                AgentLaunchFailedOutcome::StartupAutoReopen {
                    branch_name: session.branch_name,
                    message,
                }
            }
        }
    }

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
            WorkerEvent::AgentLaunchReady(boxed) => {
                let outcome = self.process_agent_launch_ready(*boxed);
                EventReaction::AgentLaunchReadyView(Box::new(outcome))
            }
            WorkerEvent::AgentLaunchFailed(boxed) => {
                let outcome = self.process_agent_launch_failed(*boxed);
                EventReaction::AgentLaunchFailedView(Box::new(outcome))
            }
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
                    self.spawn_pr_sync_worker();
                    self.spawn_initial_pr_refresh();
                } else {
                    logger::info(&format!(
                        "[gh-integration] gh status: {:?}, integration enabled: {}",
                        status, self.github_integration_enabled,
                    ));
                }
                EventReaction::Nothing
            }
            WorkerEvent::PrStatusReady(results) => {
                let now = Instant::now();
                let mut changed = false;
                for (session_id, maybe_pr) in results {
                    self.pr_last_checked.insert(session_id.clone(), now);
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
                    EventReaction::RebuildLeftItems
                } else {
                    EventReaction::Nothing
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
                self.spawn_pr_check_for_session(&session_id);
                EventReaction::Nothing
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
                let outcome = self.process_project_persistence_completed(action, result);
                EventReaction::ProjectPersistenceOutcome(Box::new(outcome))
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
    use crate::config::{DuxPaths, ProviderCommandConfig};
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
            pr_last_checked: HashMap::new(),
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

    #[test]
    fn finish_delete_session_unknown_id_returns_none() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.finish_delete_session("missing").unwrap().is_none());
    }

    #[test]
    fn finish_delete_session_removes_session_and_returns_outcome() {
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        engine.projects.push(project.clone());
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session.clone());

        let outcome = engine
            .finish_delete_session("s1")
            .unwrap()
            .expect("outcome");
        assert!(engine.sessions.is_empty());
        assert!(!engine.providers.contains_key("s1"));
        assert_eq!(outcome.session.id, "s1");
        assert_eq!(outcome.project.as_ref().map(|p| p.id.as_str()), Some("p1"));
        assert!(!outcome.other_sessions_on_worktree);
        assert!(!outcome.project_still_has_sessions);
    }

    #[test]
    fn finish_delete_session_detects_sibling_on_same_worktree() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let mut sibling = sample_session("s1", "p1", "feat/x");
        let mut deleted = sample_session("s2", "p1", "feat/y");
        // Force both to share a worktree path.
        sibling.worktree_path = "/tmp/wt/shared".to_string();
        deleted.worktree_path = "/tmp/wt/shared".to_string();
        engine.session_store.upsert_session(&sibling).unwrap();
        engine.session_store.upsert_session(&deleted).unwrap();
        engine.sessions.push(sibling);
        engine.sessions.push(deleted);

        let outcome = engine
            .finish_delete_session("s2")
            .unwrap()
            .expect("outcome");
        assert!(outcome.other_sessions_on_worktree);
        assert!(outcome.project_still_has_sessions);
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
            EventReaction::AgentLaunchReadyView(_) => "AgentLaunchReadyView",
            EventReaction::AgentLaunchFailedView(_) => "AgentLaunchFailedView",
            EventReaction::CommitMessageGenerated(_) => "CommitMessageGenerated",
            EventReaction::CommitMessageFailed(_) => "CommitMessageFailed",
            EventReaction::BrowserEntriesArrived { .. } => "BrowserEntriesArrived",
            EventReaction::ProjectWorktreesArrived { .. } => "ProjectWorktreesArrived",
            EventReaction::OpenNewAgentPromptForPr(_) => "OpenNewAgentPromptForPr",
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
            EventReaction::ProjectPersistenceOutcome(_) => "ProjectPersistenceOutcome",
            EventReaction::StartupCommandSucceeded { .. } => "StartupCommandSucceeded",
            EventReaction::StartupLogArrived { .. } => "StartupLogArrived",
            EventReaction::FinishDeleteSessionView(_) => "FinishDeleteSessionView",
            EventReaction::DoDeleteSessionView(_) => "DoDeleteSessionView",
            EventReaction::BeginDeleteSessionView(_) => "BeginDeleteSessionView",
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
    fn pr_status_ready_with_pr_upserts_and_records_timestamp() {
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

        // changed -> RebuildLeftItems (engine writes the timestamp directly).
        assert!(
            matches!(reaction, EventReaction::RebuildLeftItems),
            "expected RebuildLeftItems, got {}",
            reaction_kind(&reaction),
        );
        assert!(engine.pr_last_checked.contains_key("s1"));

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
    fn pr_status_ready_none_removes_existing_and_records_timestamps() {
        let (mut engine, _tmp) = test_engine();
        // Pre-seed pr_statuses for s1 so the None path actually removes
        // something (and flips `changed`). s2 has no PR — None for s2 leaves
        // `changed` alone for s2 but its id must still get a timestamp in
        // `pr_last_checked`.
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

        // s1 was removed -> changed -> RebuildLeftItems.
        assert!(
            matches!(reaction, EventReaction::RebuildLeftItems),
            "expected RebuildLeftItems, got {}",
            reaction_kind(&reaction),
        );
        assert!(!engine.pr_statuses.contains_key("s1"));
        // Both ids must get a timestamp in pr_last_checked even though only
        // s1 caused a state change.
        assert!(engine.pr_last_checked.contains_key("s1"));
        assert!(engine.pr_last_checked.contains_key("s2"));
    }

    #[test]
    fn pr_status_ready_unchanged_writes_timestamp_and_returns_nothing() {
        let (mut engine, _tmp) = test_engine();
        // No pre-seeded pr_statuses; sending None for s1 leaves changed=false.
        let reaction =
            engine.process_worker_event(WorkerEvent::PrStatusReady(vec![("s1".to_string(), None)]));

        assert!(
            matches!(reaction, EventReaction::Nothing),
            "expected Nothing, got {}",
            reaction_kind(&reaction),
        );
        assert!(engine.pr_last_checked.contains_key("s1"));
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

    // ── process_agent_launch_failed + detach_conflicting_worktree_session ──

    fn make_failed_data(
        session_id: &str,
        branch: &str,
        kind: AgentLaunchKind,
        message: &str,
    ) -> AgentLaunchFailedData {
        AgentLaunchFailedData {
            request: AgentLaunchRequest {
                session: sample_session(session_id, "project-1", branch),
                provider_config: ProviderCommandConfig::default(),
                env: Vec::new(),
                resume: false,
                pty_size: (24, 80),
                scrollback_lines: 1000,
                kind,
            },
            message: message.to_string(),
        }
    }

    #[test]
    fn process_agent_launch_failed_create_clears_in_flight_and_returns_message() {
        let (mut engine, _tmp) = test_engine();
        engine.agent_launches_in_flight.insert("s1".to_string());
        engine.create_agent_in_flight = true;
        let data = make_failed_data(
            "s1",
            "feat/x",
            AgentLaunchKind::Create {
                status_message: String::new(),
                repo_path: String::from("/tmp/wt"),
                owns_worktree: true,
                startup_result: None,
            },
            "boom",
        );
        let outcome = engine.process_agent_launch_failed(data);
        assert!(!engine.agent_launches_in_flight.contains("s1"));
        assert!(!engine.create_agent_in_flight);
        assert!(
            matches!(outcome, AgentLaunchFailedOutcome::Create { message } if message == "boom")
        );
    }

    #[test]
    fn process_agent_launch_failed_resume_fallback_marks_detached() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "project-1", "feat/x");
        let _ = engine.session_store.upsert_session(&session);
        engine.sessions.push(session);
        engine.agent_launches_in_flight.insert("s1".to_string());

        let data = make_failed_data(
            "s1",
            "feat/x",
            AgentLaunchKind::ResumeFallback {
                status_message: String::new(),
            },
            "boom",
        );
        let outcome = engine.process_agent_launch_failed(data);
        assert!(matches!(outcome, AgentLaunchFailedOutcome::ResumeFallback));
        assert!(!engine.agent_launches_in_flight.contains("s1"));
        assert_eq!(engine.sessions[0].status, SessionStatus::Detached);
    }

    #[test]
    fn process_agent_launch_failed_reconnect_returns_branch_name() {
        // Verifies that the Reconnect arm carries branch_name to the App for
        // the "Reconnect failed for agent \"…\": …" status format.
        let (mut engine, _tmp) = test_engine();
        let data = make_failed_data(
            "s1",
            "feat/x",
            AgentLaunchKind::Reconnect {
                status_message: String::new(),
            },
            "boom",
        );
        let outcome = engine.process_agent_launch_failed(data);
        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::Reconnect { branch_name, message }
                if branch_name == "feat/x" && message == "boom"
        ));
    }

    #[test]
    fn process_agent_launch_failed_startup_auto_reopen_returns_branch_and_message() {
        let (mut engine, _tmp) = test_engine();
        let data = make_failed_data("s1", "feat/x", AgentLaunchKind::StartupAutoReopen, "boom");
        let outcome = engine.process_agent_launch_failed(data);
        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::StartupAutoReopen { branch_name, message }
                if branch_name == "feat/x" && message == "boom"
        ));
    }

    #[test]
    fn detach_conflicting_worktree_session_returns_none_with_no_conflict() {
        let (mut engine, _tmp) = test_engine();
        let s1 = sample_session("s1", "project-1", "feat/x");
        engine.sessions.push(s1);
        let detached = engine.detach_conflicting_worktree_session("/tmp/wt/a", "s1");
        assert!(detached.is_none());
    }

    // ── process_project_persistence_completed ────────────────────────────

    #[test]
    fn process_project_persistence_completed_add_pushes_project_and_returns_added() {
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        let action = ProjectPersistenceAction::Add {
            project: project.clone(),
            status_message: "Added project \"p1\" to workspace.".to_string(),
        };
        let outcome = engine.process_project_persistence_completed(action, Ok(()));
        assert_eq!(engine.projects.len(), 1);
        assert_eq!(engine.projects[0].id, "p1");
        assert!(matches!(
            outcome.view,
            ProjectPersistenceView::Added { ref project_id, ref status_message }
                if project_id == "p1" && status_message == "Added project \"p1\" to workspace."
        ));
    }

    #[test]
    fn process_project_persistence_completed_remove_drops_project_and_returns_removed() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let action = ProjectPersistenceAction::Remove {
            project_id: "p1".to_string(),
            project_name: "Pee One".to_string(),
        };
        let outcome = engine.process_project_persistence_completed(action, Ok(()));
        assert!(engine.projects.is_empty());
        assert!(matches!(
            outcome.view,
            ProjectPersistenceView::Removed { ref project_name } if project_name == "Pee One"
        ));
    }

    #[test]
    fn process_project_persistence_completed_update_default_provider_mutates_project() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let action = ProjectPersistenceAction::UpdateDefaultProvider {
            project_id: "p1".to_string(),
            project_name: "Pee One".to_string(),
            provider: Some(ProviderKind::from_str("claude")),
            global_default: ProviderKind::from_str("codex"),
        };
        let outcome = engine.process_project_persistence_completed(action, Ok(()));
        assert_eq!(
            engine.projects[0]
                .explicit_default_provider
                .as_ref()
                .map(|p| p.as_str()),
            Some("claude"),
        );
        assert!(matches!(
            outcome.view,
            ProjectPersistenceView::DefaultProviderUpdated { ref project_name, .. }
                if project_name == "Pee One"
        ));
    }

    #[test]
    fn process_project_persistence_completed_err_returns_persistence_failed() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let action = ProjectPersistenceAction::Remove {
            project_id: "p1".to_string(),
            project_name: "Pee One".to_string(),
        };
        let outcome =
            engine.process_project_persistence_completed(action, Err("disk full".to_string()));
        // Engine did NOT mutate state on error.
        assert_eq!(engine.projects.len(), 1);
        assert!(matches!(
            outcome.view,
            ProjectPersistenceView::PersistenceFailed { ref error } if error == "disk full"
        ));
    }

    // ── Engine::do_delete_session + Engine::begin_delete_session ────────────

    #[test]
    fn begin_delete_session_already_in_flight_returns_already_in_flight() {
        let (mut engine, _tmp) = test_engine();
        engine.pending_deletions.insert("s1".to_string());
        let outcome = engine.begin_delete_session("s1", true);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AlreadyInFlight
        ));
    }

    #[test]
    fn begin_delete_session_unknown_id_returns_not_found() {
        let (mut engine, _tmp) = test_engine();
        let outcome = engine.begin_delete_session("missing", true);
        assert!(matches!(outcome, BeginDeleteSessionOutcome::NotFound));
    }

    #[test]
    fn begin_delete_session_inline_when_no_worktree_removal_needed() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        // delete_worktree=false → no git work needed → inline path
        let outcome = engine.begin_delete_session("s1", false);
        assert!(matches!(outcome, BeginDeleteSessionOutcome::Inline));
        assert!(!engine.pending_deletions.contains("s1"));
    }

    #[test]
    fn do_delete_session_unknown_id_returns_none() {
        let (mut engine, _tmp) = test_engine();
        assert!(
            engine
                .do_delete_session("missing", false)
                .unwrap()
                .is_none()
        );
    }

    // ── Engine::apply on the deletion family (E4a) ───────────────────────

    #[test]
    fn apply_begin_delete_session_returns_already_in_flight_when_pending() {
        let (mut engine, _tmp) = test_engine();
        engine.pending_deletions.insert("s1".to_string());
        let reaction = engine
            .apply(crate::engine::Command::BeginDeleteSession {
                session_id: "s1".to_string(),
                delete_worktree: true,
            })
            .unwrap();
        assert!(matches!(
            reaction,
            EventReaction::BeginDeleteSessionView(view)
                if matches!(view.outcome, BeginDeleteSessionOutcome::AlreadyInFlight)
        ));
    }

    #[test]
    fn apply_begin_delete_session_returns_not_found_for_unknown_id() {
        let (mut engine, _tmp) = test_engine();
        let reaction = engine
            .apply(crate::engine::Command::BeginDeleteSession {
                session_id: "missing".to_string(),
                delete_worktree: false,
            })
            .unwrap();
        assert!(matches!(
            reaction,
            EventReaction::BeginDeleteSessionView(view)
                if matches!(view.outcome, BeginDeleteSessionOutcome::NotFound)
        ));
    }

    #[test]
    fn apply_do_delete_session_returns_nothing_for_unknown_id() {
        let (mut engine, _tmp) = test_engine();
        let reaction = engine
            .apply(crate::engine::Command::DoDeleteSession {
                session_id: "missing".to_string(),
                delete_worktree: false,
            })
            .unwrap();
        assert!(matches!(reaction, EventReaction::Nothing));
    }

    #[test]
    fn apply_finish_delete_session_returns_nothing_for_unknown_id() {
        let (mut engine, _tmp) = test_engine();
        let reaction = engine
            .apply(crate::engine::Command::FinishDeleteSession {
                session_id: "missing".to_string(),
                delete_worktree: false,
                remove_outcome: None,
                update_status: true,
            })
            .unwrap();
        assert!(matches!(reaction, EventReaction::Nothing));
    }
}
