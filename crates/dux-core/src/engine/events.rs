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
use crate::engine::{Engine, InFlightKey};
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
    /// Optional correlation key. `None` = an unkeyed transient. `Some` = a
    /// keyed op whose later success/error/clear carries the same key so both
    /// surfaces can correlate the pair. Ignored by the TUI today; copied into
    /// `WireStatus::key` by `WireStatus::from_update` so the web layer can
    /// dismiss the matching toast when the final status arrives.
    pub key: Option<String>,
}

impl StatusUpdate {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Info,
            message: message.into(),
            key: None,
        }
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Busy,
            message: message.into(),
            key: None,
        }
    }
    #[allow(dead_code)]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Warning,
            message: message.into(),
            key: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Error,
            message: message.into(),
            key: None,
        }
    }

    /// Construct a keyed status update. Both the busy and the final
    /// (info/error) for the same operation should carry the same key so
    /// `WireStatus::from_update` can propagate it and the web layer can
    /// dismiss the correct toast.
    pub fn keyed(key: impl Into<String>, tone: StatusTone, message: impl Into<String>) -> Self {
        Self {
            tone,
            message: message.into(),
            key: Some(key.into()),
        }
    }

    /// Attach a correlation key to this update (builder form). Lets callers
    /// start from one of the tone helpers and then chain `.with_key(k)`.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
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
    /// Dismiss a keyed status with no replacement message (the `Final::Clear`
    /// outcome of a `StatusOp`). The TUI removes the keyed entry; the web emits
    /// a `StatusCleared` frame for the key.
    ClearStatus(String),
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

    // -- Commit message overlay (App formats final status with bindings). The
    //    session id scopes the result so a web client routes it to the matching
    //    commit dialog (the TUI has a single dialog and ignores it). --
    CommitMessageGenerated {
        session_id: String,
        message: String,
    },
    CommitMessageFailed {
        session_id: String,
        error: String,
    },

    // -- Picker/browser prompts. --
    BrowserEntriesArrived {
        dir: std::path::PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ProjectWorktreesArrived {
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in
        /// the completion handler. `None` for the web/wire path.
        status_op_id: Option<String>,
    },

    // -- PR / refs follow-ups. --
    OpenNewAgentPromptForPr {
        pr: Box<ResolvedPullRequest>,
        /// Correlation id for a web PR-lookup `HandlerStatusOp`. `Some` on the web
        /// handoff path (the followup clears this op's busy once the create
        /// dispatch takes over); `None` for the TUI, which opens a name prompt.
        status_op_id: Option<String>,
    },

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
        /// Correlation id for a web add-project `HandlerStatusOp`. `Some` on the
        /// web path, resolved in `drive_add_project_followup` after the inline
        /// add; `None` for the TUI.
        status_op_id: Option<String>,
    },

    // -- Branch inspection follow-ups (App helpers). --
    ContinueCreateAgentAfterInspection {
        project: Project,
        inspection: CreateAgentBranchInspection,
    },
    DispatchProjectDefaultBranchCheckout {
        project: Project,
        default_branch: String,
        /// Correlation id for a web checkout `HandlerStatusOp`, forwarded by
        /// `drive_checkout_followup` into worker 2 so the eventual
        /// `NonDefaultBranchCheckoutCompleted` resolves the right op. `None` for
        /// the TUI.
        status_op_id: Option<String>,
    },

    // -- Config reload (App helpers). --
    ApplyReloadedConfig(Box<Config>),
    OpenConfigReloadFailedModal(String),

    // -- Project persistence (App applies view follow-up; Engine performed mutations). --
    ProjectPersistenceOutcome(Box<ProjectPersistenceOutcome>),

    // -- Web-UI login-user add (App resolves the HandlerStatusOp). --
    /// A web-UI login-user add finished: the bcrypt hash ran off-thread and the
    /// engine performed the fallible post-worker config write. The Engine already
    /// adopted (or rolled back) `config.auth.users`; the App only surfaces the
    /// final. When `status_op_id` is `Some`, the App pops the matching
    /// `HandlerStatusOp<AuthUserFinalOutcome>` and resolves it into the keyed
    /// final; when `None` (web/wire, engine internals) the App/wire builds the
    /// `StatusUpdate` directly via `AuthUserFinalOutcome::into_status`.
    AuthUsersOutcome {
        outcome: AuthUserFinalOutcome,
        status_op_id: Option<String>,
    },

    // -- Startup command / log viewer (App formats key + opens overlay). --
    StartupLogArrived {
        scope_label: String,
        log: StartupCommandLatestLog,
    },

    // -- Agent-creation dispatch (E4c). --
    /// View follow-up for `Command::DispatchAgentLaunch`. The Engine performs
    /// the in-flight check + spawn; the App caller uses `launched` to decide
    /// site-specific follow-up (busy messages, status updates, fallback
    /// branches). `status` is `Some(StatusUpdate::info(…))` only on the
    /// already-in-flight path.
    DispatchAgentLaunchView(Box<DispatchAgentLaunchView>),

    // -- Companion terminal deletion (E4e). --
    /// View follow-up for `Command::DeleteTerminal`. The Engine has dropped
    /// the `PtyClient` (killing the child); the App clears
    /// `active_terminal_id` if it matches and clamps the terminal cursor.
    DeleteTerminalView(Box<DeleteTerminalView>),

    // -- Web-server flip pre-flight (App owns the listeners + flip state). --
    /// The worker that ran Tailscale detection + bound the LOCAL MODE listeners
    /// finished. The Engine has no domain state to mutate here — the listeners
    /// and flip are TUI concerns — so this passes straight through to the App,
    /// which stashes `pending_server_flip` (on `Ok`) or surfaces the error, and
    /// shows the non-fatal `warning` when present.
    ServerFlipPreflightReady {
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        warning: Option<String>,
    },
}

/// Result of `Engine::detach_conflicting_worktree_session` — the App caller
/// uses `id` to clear the engine's `pty_activity` entry and `label` for status
/// messages.
#[derive(Clone, Debug)]
pub struct DetachedSession {
    pub id: String,
    pub label: String,
}

/// View-only follow-up for `WorkerEvent::AgentLaunchReady`. The Engine has
/// already performed all domain-state mutations (the `in_flight` set,
/// sessions, providers, session_store, mark_session_* helpers,
/// resume_fallback_*, update_branch_sync_sessions, and the pure-engine
/// portion of detach_conflicting_worktree_session). The App applies
/// `last_pty_size`, clears the engine's `pty_activity` entry for any
/// `detached_session_id`, runs view rebuilds, sets surfaces/overlays/status.
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
/// already cleared `InFlightKey::AgentLaunch(session_id)`, cleared
/// `InFlightKey::CreateAgent` for Create-kind failures, logged the
/// ResumeFallback / StartupAutoReopen cases, and marked ResumeFallback
/// sessions Detached. The App only formats the status message.
pub enum AgentLaunchFailedOutcome {
    Create {
        project_id: String,
        message: String,
    },
    /// Reconnect-family failure. `session_id` is the pre-existing session that
    /// was being relaunched — used by the wire layer to key the failure status
    /// so it replaces the corresponding "launching…" busy toast.
    Reconnect {
        session_id: String,
        branch_name: String,
        message: String,
    },
    /// Force-reconnect failure. `session_id` carries the pre-existing session
    /// id for the same keying purpose as `Reconnect`.
    ForceReconnect {
        session_id: String,
        branch_name: String,
        message: String,
    },
    /// Engine logged + marked Detached; App has nothing to do.
    ResumeFallback,
    /// Startup-auto-reopen failure. `session_id` carries the pre-existing
    /// session id for the same keying purpose as `Reconnect`.
    StartupAutoReopen {
        session_id: String,
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
    /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in the
    /// completion handler (the post-worker config write is fallible, producing a
    /// third outcome the worker never sees). `None` for callers that don't drive
    /// a handler-resolved status (web/wire, engine internals).
    pub status_op_id: Option<String>,
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

/// Handler-computed outcome for a web-UI login-user add (see
/// [`crate::worker::WorkerEvent::AuthUsersPersisted`]). The bcrypt hash runs on a
/// background thread; the engine handler then performs the fallible post-worker
/// `config.toml` write (and a removal-empties-the-list warn flag), producing one
/// of these results the worker never sees. A TUI `HandlerStatusOp` resolver
/// (declared at the add dispatch site) maps this to the final user message; the
/// `None`-correlation-id callers (web/wire, the engine's own tests) instead build
/// the `StatusUpdate` directly via [`Self::into_status`].
///
/// The message and `warn` tone are decided where they always were — the worker
/// builds the success line, the engine builds the single error template — and are
/// carried here verbatim so both surfaces stay byte-identical to the pre-op code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthUserFinalOutcome {
    /// The hash succeeded and the post-worker config write succeeded. `message`
    /// is the verbose, already-formatted status line; `warn` requests a
    /// warning-tone status (set when a change empties the valid-user set, which
    /// disables the login gate — an add never empties it, so the add path always
    /// passes `false`).
    Saved { warn: bool, message: String },
    /// The hash/DB worker result was `Err`, OR the post-worker config write
    /// failed. Both currently surface the same error template, so they fold into
    /// one variant carrying the formatted error detail.
    Errored(String),
}

impl AuthUserFinalOutcome {
    /// Build the keyed-or-unkeyed final [`StatusUpdate`] for the callers that do
    /// NOT drive a TUI `HandlerStatusOp` (web/wire and the engine's own
    /// internals): the same tones and byte-identical messages a resolver would
    /// produce. `key` correlates the status to a pending busy when one exists.
    pub fn into_status(self, key: Option<String>) -> StatusUpdate {
        let update = match self {
            AuthUserFinalOutcome::Saved {
                warn: true,
                message,
            } => StatusUpdate::warning(message),
            AuthUserFinalOutcome::Saved {
                warn: false,
                message,
            } => StatusUpdate::info(message),
            AuthUserFinalOutcome::Errored(err) => StatusUpdate::error(format!(
                "Could not save web UI login users to config.toml: {err}"
            )),
        };
        match key {
            Some(key) => update.with_key(key),
            None => update,
        }
    }
}

/// What happened to the session's worktree during deletion. Each variant maps
/// 1:1 to a user-facing status message; the illegal "delete requested, no
/// siblings, but no result" state has no representation. Replaces the former
/// `(delete_worktree: bool, remove_outcome: Option<bool>)` pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeRemoval {
    /// Deletion NOT requested; worktree shared with sibling sessions.
    PreservedShared,
    /// Deletion NOT requested; no siblings — worktree left at its path.
    PreservedOrphan,
    /// Deletion requested but skipped because siblings still use the worktree.
    SkippedForSiblings,
    /// Worktree removed. `branch_already_deleted` mirrors git's report.
    Performed { branch_already_deleted: bool },
}

impl WorktreeRemoval {
    /// Derive the removal outcome for a synchronous (inline / `do_delete`)
    /// decision, given user intent and whether siblings share the worktree.
    /// `performed` is `Some(branch_already_deleted)` when git actually removed
    /// the worktree, `None` when it was not run. The caller guarantees
    /// `performed.is_some()` exactly when `delete_worktree && !other_sessions`.
    fn from_decision(
        delete_worktree: bool,
        other_sessions_on_worktree: bool,
        performed: Option<bool>,
    ) -> Self {
        match (delete_worktree, other_sessions_on_worktree, performed) {
            (_, _, Some(branch_already_deleted)) => WorktreeRemoval::Performed {
                branch_already_deleted,
            },
            (true, true, None) => WorktreeRemoval::SkippedForSiblings,
            (false, true, None) => WorktreeRemoval::PreservedShared,
            (false, false, None) => WorktreeRemoval::PreservedOrphan,
            // delete requested, no siblings, but git did not run: impossible by
            // the caller's contract. Default to the most truthful preserved
            // state rather than panicking.
            (true, false, None) => WorktreeRemoval::PreservedOrphan,
        }
    }
}

/// Result of `Engine::finish_delete_session`. Carries the deleted session
/// and project context the App needs to apply view follow-up
/// (`pty_activity` clear, `clear_companion_terminals_for_session`,
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
    /// What happened to the worktree. Drives status formatting in
    /// `apply_finish_delete_session_outcome`.
    pub removal: WorktreeRemoval,
}

/// Result of `Engine::begin_delete_session`. The four branches mirror the
/// original App method's control flow.
#[derive(Debug)]
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
    /// `finish_delete_session` wrapper to complete cleanup + emit status.
    Inline { removal: WorktreeRemoval },
}

/// View follow-up data for a `Command::FinishDeleteSession`. Wraps the
/// engine outcome with the App-context fields needed for status formatting.
pub struct FinishDeleteSessionView {
    pub session_id: String,
    pub outcome: FinishDeleteSessionOutcome,
    pub removal: WorktreeRemoval,
    pub update_status: bool,
}

/// View follow-up data for a `Command::DoDeleteSession`. Wraps the engine
/// outcome with the App-context fields needed for status formatting.
pub struct DoDeleteSessionView {
    pub session_id: String,
    pub outcome: DoDeleteSessionOutcome,
}

/// View follow-up data for a `Command::BeginDeleteSession`. Wraps the
/// engine outcome with the App-context fields needed for status
/// formatting and the inline cleanup follow-up.
pub struct BeginDeleteSessionView {
    pub session_id: String,
    pub outcome: BeginDeleteSessionOutcome,
}

/// View follow-up for `Command::DispatchAgentLaunch`. The Engine performs
/// the in-flight check + spawn; the App caller uses `launched` to decide
/// site-specific follow-up (busy messages, status updates, fallback
/// branches). `status` is `Some(StatusUpdate::info(…))` only on the
/// already-in-flight path. `session_id` is the id of the session whose
/// launch was attempted, populated on both branches so downstream
/// observers (e.g. the future web layer) can correlate the dispatch
/// with its session without re-deriving it from the request.
pub struct DispatchAgentLaunchView {
    pub session_id: String,
    pub launched: bool,
    pub status: Option<StatusUpdate>,
}

/// View follow-up for `Command::DeleteTerminal`. `label` is `Some(label)`
/// if the terminal existed; `None` if it was already gone. The App caller
/// clears `active_terminal_id` if it matches and clamps the terminal
/// cursor.
pub struct DeleteTerminalView {
    pub terminal_id: String,
    pub label: Option<String>,
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
    /// the engine's `pty_activity`/`pty_input` entries and surface the label in
    /// status messages.
    ///
    /// The App's `detach_conflicting_worktree_session` is a thin wrapper that
    /// also drops the `pty_activity` and `pty_input` entries for the returned id.
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
        self.clear_in_flight(&InFlightKey::AgentLaunch(session.id.clone()));

        if matches!(request.kind, AgentLaunchKind::Create { .. }) {
            self.clear_in_flight(&InFlightKey::CreateAgent);
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
        status_op_id: Option<String>,
    ) -> ProjectPersistenceOutcome {
        if let Err(error) = result {
            return ProjectPersistenceOutcome {
                action,
                view: ProjectPersistenceView::PersistenceFailed { error },
                status_op_id,
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

        ProjectPersistenceOutcome {
            action,
            view,
            status_op_id,
        }
    }

    /// Engine half of the session-deletion cascade. Removes the session from
    /// the store + providers + runtime maps + the sessions vector; refreshes
    /// branch-sync entries; spawns the startup-log deletion worker. Returns
    /// `Ok(None)` if the session was already gone; `Ok(Some(outcome))` with
    /// the context the App needs for its view-side follow-up; `Err` on a
    /// store failure (in-memory state untouched in that case so the UI keeps
    /// showing the session).
    ///
    /// **Ordering invariant for engine-side helpers**: this method performs
    /// all engine-state cleanup (providers, running_provider_pins,
    /// resume_fallback_candidates, pty_activity, sessions.retain,
    /// update_branch_sync_sessions) before returning. The caller is then
    /// responsible for view-side cleanup (e.g. companion-terminal view
    /// teardown). During the gap between this method returning and the
    /// App-side applier running, those view-only maps still hold stale
    /// entries for the deleted session_id. Engine helpers invoked from inside
    /// this method MUST NOT read those view-only maps for the deleted
    /// session_id — they will see stale data. If a future helper needs to
    /// observe view state during deletion, the deletion sequence must be
    /// re-architected to invert the engine/view ordering.
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
        self.pty_activity.remove(&session.id);
        self.pty_input.remove(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.companion_terminals
            .retain(|_, t| t.session_id != session.id);
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
    /// Returns `Ok(None)` if the session was already gone or an async delete
    /// worker is already in flight for this session; `Ok(Some(outcome))`
    /// otherwise; `Err` if `git::remove_worktree` or
    /// `session_store.delete_session` fails. A missing project record does NOT
    /// abort the deletion — the session record is still removed, but its worktree
    /// is kept (we cannot run `git worktree remove` without the project repo).
    ///
    /// Callers must ensure no async worker is already removing this worktree
    /// (`pending_deletions` should not contain `session_id`). If a caller
    /// bypasses that contract, this method soft-returns `Ok(None)` and logs an
    /// error rather than racing `git::remove_worktree` against the in-flight
    /// async deletion — debug-only checks would not catch this in release
    /// builds, and the path is destructive (worktrees are user data).
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
        // The project may be ABSENT for an orphaned session; we can still delete
        // the record but cannot remove its worktree without the project repo, so
        // an orphan always keeps its worktree.
        let project = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned();
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);

        let should_remove_worktree =
            delete_worktree && !other_sessions_on_worktree && project.is_some();

        if self.pending_deletions.contains(session_id) {
            crate::logger::error(&format!(
                "do_delete_session called while an async delete worker is in-flight for {session_id} \u{2014} refusing to proceed to avoid racing git::remove_worktree",
            ));
            return Ok(None);
        }
        let remove_outcome = if should_remove_worktree {
            let project = project
                .as_ref()
                .expect("should_remove_worktree implies a project");
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
            removal: WorktreeRemoval::from_decision(
                delete_worktree,
                finish.other_sessions_on_worktree,
                remove_outcome,
            ),
            finish,
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
        // The project may be ABSENT for an orphaned session (its project was
        // removed but the session record outlived it). We can still delete the
        // session record; we just cannot run `git worktree remove` without the
        // project repo, so an orphan keeps its worktree and takes the inline path.
        let project = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned();
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);
        let should_remove_worktree =
            delete_worktree && !other_sessions_on_worktree && project.is_some();

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
            let project_path = project
                .as_ref()
                .expect("should_remove_worktree implies a project")
                .path
                .clone();
            let worktree_path = session.worktree_path.clone();
            let branch_name = session.branch_name.clone();
            let tx = self.worker_tx.clone();
            std::thread::spawn(move || {
                use std::panic::AssertUnwindSafe;
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    crate::git::remove_worktree(
                        std::path::Path::new(&project_path),
                        std::path::Path::new(&worktree_path),
                        &branch_name,
                    )
                    .map(|r| r.branch_already_deleted)
                    .map_err(|e| format!("{e:#}"))
                }))
                .unwrap_or_else(|payload| {
                    let reason = crate::engine::spawn_worker::format_panic_payload(payload);
                    crate::logger::error(&format!(
                        "worktree-remove worker panicked for session {sid}: {reason}"
                    ));
                    Err(format!("Worker panicked: {reason}"))
                });
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
            BeginDeleteSessionOutcome::Inline {
                removal: WorktreeRemoval::from_decision(
                    delete_worktree,
                    other_sessions_on_worktree,
                    None,
                ),
            }
        }
    }

    pub fn process_agent_launch_failed(
        &mut self,
        data: AgentLaunchFailedData,
    ) -> AgentLaunchFailedOutcome {
        let AgentLaunchFailedData { request, message } = data;
        let session = request.session;
        self.clear_in_flight(&InFlightKey::AgentLaunch(session.id.clone()));

        match request.kind {
            AgentLaunchKind::Create { .. } => {
                self.clear_in_flight(&InFlightKey::CreateAgent);
                AgentLaunchFailedOutcome::Create {
                    project_id: session.project_id.clone(),
                    message,
                }
            }
            AgentLaunchKind::Reconnect { .. } => AgentLaunchFailedOutcome::Reconnect {
                session_id: session.id,
                branch_name: session.branch_name,
                message,
            },
            AgentLaunchKind::ForceReconnect { .. } => AgentLaunchFailedOutcome::ForceReconnect {
                session_id: session.id,
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
                    session_id: session.id,
                    branch_name: session.branch_name,
                    message,
                }
            }
        }
    }

    /// Close the reload barrier opened by `Command::ReloadConfig` and drive the
    /// follow-up: drop the writer quiesce guard, clear `reloading`, and drain
    /// any commands that were deferred while the reload was in flight.
    ///
    /// Ordering matters (see F1): on success the engine first applies the
    /// reloaded config to its own state, then clears the barrier flags, then
    /// drains the deferred commands — each of which re-mutates the now-current
    /// config and eager-writes. The deferred command's write is therefore the
    /// LAST write to disk, and the config it carries (reloaded + the deferred
    /// change) is the final on-disk state.
    ///
    /// To keep in-memory state in lockstep with disk, the `ApplyReloadedConfig`
    /// reaction carries the FINAL config (reloaded + drained) whenever deferred
    /// commands ran. The surface's richer apply (theme/keybindings/projects, and
    /// for the web the auth-gate rebuild) then re-applies that same final config,
    /// so it never reverts a deferred change back to the pre-deferral snapshot.
    ///
    /// In the common no-deferral case the engine leaves `self.config` untouched
    /// and returns the bare reloaded config: the surface's apply does the swap
    /// and must see the still-running (pre-swap) config so it can diff old vs new
    /// (the web actor's "restart to apply server settings" detection). The one
    /// tradeoff: when a deferral coincides with a [server] change in the same
    /// reload, the engine pre-swaps before the actor diffs, so that advisory
    /// restart warning is suppressed — the auth-gate rebuild, which reads the
    /// final users, still takes effect. This is acceptable for that rare overlap.
    ///
    /// On failure (the reload could not be parsed, OR it parsed but could not be
    /// applied to engine state) the in-memory config is unchanged (still current),
    /// so the deferred user commands are re-applied against it rather than dropped
    /// (F6). The reload-failed reaction is placed LAST in the returned `Multi` so
    /// its error status wins the surface's status line instead of being overwritten
    /// by a deferred save's success message.
    fn process_config_reload_ready(&mut self, result: Result<Config, String>) -> EventReaction {
        let deferred = std::mem::take(&mut self.deferred_commands);
        let has_deferred = !deferred.is_empty();
        // Pre-swap `self.config` to the reloaded config (rather than leaving the
        // surface to do the swap) whenever we must base a follow-up write on the
        // reloaded config: a deferred command drain, OR an auth-users replay.
        // Without this, the auth replay would write/surface the OLD config (the
        // no-deferral path leaves `self.config` untouched), reverting both the
        // reloaded file on disk and the in-flight user change in memory + the live
        // auth gate. The cost is the same rare tradeoff documented above: the
        // surface's old-vs-new diff ("restart to apply server settings") is
        // suppressed when an auth change coincides with a [server] change.
        let must_preswap = has_deferred || self.pending_auth_users.is_some();

        // Step 1: compute the primary reaction and, on success, apply the reloaded
        // config to engine state BEFORE clearing the barrier — but only when we
        // must drain deferred commands (they re-mutate and re-save the config, so
        // they need the reloaded config as their base). With no deferral the engine
        // leaves `self.config` alone and lets the surface do the swap (so the
        // surface can still diff old vs new). `failure` carries a reload-failed
        // reaction when the reload could not be applied — including the case where
        // the parse succeeded but applying it to engine state failed: that is a
        // genuine reload failure, not a silent success on a stale config.
        let mut failure: Option<EventReaction> = None;
        let bare_apply: Option<EventReaction> = match result {
            Ok(config) => {
                if must_preswap {
                    // Apply the reloaded config so the deferred drain / auth replay
                    // re-mutates it (and the surfaced config carries those edits).
                    // If applying it FAILS, do not pretend the reload worked: open
                    // the reload-failed modal and leave `self.config` as-is (the
                    // deferred commands below still re-apply against the current
                    // config, so they are never dropped — F6).
                    if let Err(err) = self.apply_reloaded_config(config) {
                        failure = Some(EventReaction::OpenConfigReloadFailedModal(format!(
                            "Config validated but could not be applied: {err:#}"
                        )));
                    }
                    // On success the FINAL config (reloaded + deferred) is surfaced
                    // after the drain below, so there is no bare reaction here.
                    None
                } else {
                    Some(EventReaction::ApplyReloadedConfig(Box::new(config)))
                }
            }
            Err(message) => {
                failure = Some(EventReaction::OpenConfigReloadFailedModal(message));
                None
            }
        };

        // Step 2: clear the barrier — resume the writer and stop deferring. Done
        // AFTER applying the reloaded config (so deferred re-applies write the
        // reloaded-plus-change config) and BEFORE the drain (so the re-applied
        // commands take the normal, non-deferred path).
        self.reload_guard = None;
        self.reloading = false;

        // Step 2b: replay any auth-users update that arrived while the barrier
        // was open. The barrier is now cleared so the eager writer will accept
        // the write. The user-initiated update wins over the on-disk reloaded
        // value: we write AFTER apply_reloaded_config has swapped in the
        // reloaded config, so the final on-disk state is reloaded-config +
        // in-flight user change. Multiple arrivals folded into one by the
        // stash: last one wins.
        let pending_auth_reaction =
            self.pending_auth_users
                .take()
                .map(|(users, message, warn, status_op_id)| {
                    let previous = self.config.auth.users.clone();
                    self.config.auth.users = users;
                    // Same tone/message decision as the immediate handler, carried
                    // back as an outcome so the App resolves the deferred op (it
                    // stayed stashed across the reload barrier) against it, keyed by
                    // the id that rode in on AuthUsersPersisted.
                    let outcome = match self.config_writer.save_eager(self.config.clone()) {
                        Ok(()) => AuthUserFinalOutcome::Saved { warn, message },
                        Err(err) => {
                            self.config.auth.users = previous;
                            AuthUserFinalOutcome::Errored(err.to_string())
                        }
                    };
                    EventReaction::AuthUsersOutcome {
                        outcome,
                        status_op_id,
                    }
                });

        if !must_preswap {
            // No pre-swap needed (no deferral, no auth replay): the bare reloaded
            // config is surfaced for the surface to swap. Exactly one of
            // `bare_apply` (success) / `failure` (parse error) is set; fall back to
            // Nothing. (`pending_auth_reaction` is always None here.)
            return bare_apply.or(failure).unwrap_or(EventReaction::Nothing);
        }

        // Step 3: re-apply each deferred command now that the barrier is closed.
        // Each re-mutates the current config and eager-writes — the deferred write
        // is therefore the LAST write to disk. On a failed reload the config is
        // unchanged/current, so re-applying against it is still correct (F6:
        // deferred commands are never dropped). Collect status reactions so the
        // surface still reports each save's success/failure.
        let mut deferred_reactions = Vec::new();
        for command in deferred {
            match self.apply(command) {
                Ok(EventReaction::Nothing) => {}
                Ok(reaction) => deferred_reactions.push(reaction),
                Err(err) => deferred_reactions.push(EventReaction::Status(StatusUpdate::error(
                    format!("A deferred config change failed after reload: {err:#}"),
                ))),
            }
        }

        // Step 4: assemble the final reaction list.
        let mut reactions = Vec::new();
        if failure.is_none() {
            // Success: surface the FINAL config (reloaded + the deferred changes
            // that JUST landed) FIRST so the surface's config swap matches the
            // engine + disk state and never reverts a deferred change. Snapshot
            // `self.config` AFTER the drain above so it carries the deferred edits.
            reactions.push(EventReaction::ApplyReloadedConfig(Box::new(
                self.config.clone(),
            )));
        }
        reactions.extend(deferred_reactions);
        if let Some(auth_reaction) = pending_auth_reaction {
            // Auth-users replay: append after deferred command reactions so its
            // status (info or warning) is the last thing shown to the user.
            // Appended before failure so a reload error still wins the headline.
            reactions.push(auth_reaction);
        }
        if let Some(failure) = failure {
            // Failure: append the reload-failed modal/error LAST so its error
            // status wins the surface's status line instead of being overwritten by
            // a deferred save's success message (the deferred saves did land against
            // the still-current config, but the headline state the user needs is
            // "reload failed — review the modal").
            reactions.push(failure);
        }

        EventReaction::Multi(reactions)
    }

    /// Process a `WorkerEvent`: perform engine-side mutations and return the
    /// view follow-up the App caller should apply.
    ///
    /// The Engine MUST NOT touch view state. Anything view-side is returned
    /// via `EventReaction` for the App to apply.
    pub fn process_worker_event(&mut self, event: WorkerEvent) -> EventReaction {
        match event {
            WorkerEvent::CommandWorkerStarted(status) => EventReaction::Status(status),
            WorkerEvent::CreateAgentProgress { key, message } => {
                EventReaction::Status(StatusUpdate::busy(message).with_key(key))
            }
            WorkerEvent::CreateAgentFailed { key, message } => {
                self.clear_in_flight(&InFlightKey::CreateAgent);
                EventReaction::Status(StatusUpdate::error(message).with_key(key))
            }
            WorkerEvent::AgentLaunchReady(boxed) => {
                let outcome = self.process_agent_launch_ready(*boxed);
                EventReaction::AgentLaunchReadyView(Box::new(outcome))
            }
            WorkerEvent::AgentLaunchFailed(boxed) => {
                let outcome = self.process_agent_launch_failed(*boxed);
                EventReaction::AgentLaunchFailedView(Box::new(outcome))
            }
            WorkerEvent::ChangedFilesReady {
                staged,
                unstaged,
                worktree,
            } => {
                // Stale-poll race / CF1 watched_session_id invariant: the poller
                // snapshots the watched worktree, releases the lock, then computes
                // `git::changed_files` off-thread. If the watch moved to a
                // different session (or was cleared) while this poll was in
                // flight, applying these lists would leave the ViewModel showing
                // another worktree's files under the current `watched_session_id`
                // — which CF1's cross-tab guard would then wrongly accept. Only
                // apply when the event's worktree still matches the watch; drop
                // it otherwise.
                let still_watched = self
                    .watched_worktree
                    .lock()
                    .ok()
                    .and_then(|guard| guard.clone())
                    .is_some_and(|current| current == worktree);
                if still_watched {
                    self.staged_files = staged;
                    self.unstaged_files = unstaged;
                    EventReaction::ClampFilesCursor
                } else {
                    EventReaction::Nothing
                }
            }
            WorkerEvent::CommitMessageGenerated {
                session_id,
                message,
            } => EventReaction::CommitMessageGenerated {
                session_id,
                message,
            },
            WorkerEvent::CommitMessageFailed { session_id, error } => {
                EventReaction::CommitMessageFailed { session_id, error }
            }
            WorkerEvent::StatusOpCompleted { resolved } => resolved.into_reaction(),
            WorkerEvent::PullCompleted {
                repo_path,
                target,
                result,
                status,
            } => {
                self.clear_in_flight(&InFlightKey::Pull(repo_path.clone()));
                // Domain mutation: a successful project refresh updates the
                // project's current branch and re-derives its leading status.
                // The user-facing message was resolved at dispatch by the
                // StatusOp and rides in `status`.
                if let PullTarget::Project { project_id, .. } = &target
                    && let Ok(Some(branch_name)) = &result
                    && let Some(existing) = self.projects.iter_mut().find(|c| c.id == *project_id)
                {
                    existing.current_branch = branch_name.clone();
                    existing.branch_status =
                        if existing.leading_branch.as_deref() == Some(&existing.current_branch) {
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
                let final_reaction = status.into_reaction();
                // A successful session pull also reloads the changed-files view.
                if matches!(target, PullTarget::Session) && result.is_ok() {
                    EventReaction::Multi(vec![final_reaction, EventReaction::ReloadChangedFiles])
                } else {
                    final_reaction
                }
            }
            WorkerEvent::ClipboardCopyCompleted {
                label: _,
                result: _,
                status,
            } => {
                // The user-facing message was resolved at the call site by the
                // clipboard StatusOp and rides in `status`.
                status.into_reaction()
            }
            WorkerEvent::BranchRenameCompleted {
                session_id,
                new_branch,
                previous_title,
                result,
                status,
            } => {
                // Domain work depends on the outcome; the user-facing message was
                // resolved at dispatch by the StatusOp and rides in `status`.
                match &result {
                    Ok(()) => {
                        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            session.branch_name = new_branch.clone();
                            session.updated_at = Utc::now();
                            if let Err(err) = self.session_store.upsert_session(session) {
                                logger::error(&format!(
                                    "failed to persist branch rename for {} (new branch: {}): {err}",
                                    session.id, new_branch,
                                ));
                            }
                        }
                        self.update_branch_sync_sessions();
                    }
                    Err(_) => {
                        // Revert the title so the session doesn't stay in a mixed
                        // state where the display name changed but the branch
                        // didn't.
                        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            session.title = previous_title;
                            session.updated_at = Utc::now();
                            if let Err(err) = self.session_store.upsert_session(session) {
                                logger::error(&format!(
                                    "failed to persist branch-rename revert for {}: {err}",
                                    session.id,
                                ));
                            }
                        }
                    }
                }
                EventReaction::Multi(vec![
                    EventReaction::RebuildLeftItems,
                    status.into_reaction(),
                ])
            }
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
                        if let Err(err) = self.session_store.upsert_session(session) {
                            logger::error(&format!(
                                "failed to persist branch-sync update for {} (new branch: {}): {err}",
                                session.id, session.branch_name,
                            ));
                        }
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
                            let pr_number = pr.number;
                            if let Err(err) = self.session_store.upsert_pr(&StoredPr {
                                session_id: session_id.clone(),
                                pr_number,
                                host: pr.host.clone(),
                                owner_repo: pr.owner_repo.clone(),
                                state: state_str.to_string(),
                                title: pr.title.clone(),
                                url: pr.url.clone(),
                            }) {
                                logger::error(&format!(
                                    "failed to persist PR status for {session_id} (PR #{pr_number}): {err}",
                                ));
                            }
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
            WorkerEvent::PullRequestResolved {
                result,
                status_op_id,
            } => match result {
                Ok(pr) => EventReaction::OpenNewAgentPromptForPr {
                    pr: Box::new(pr),
                    status_op_id,
                },
                Err(message) => {
                    // Web path: resolve the PR-lookup op into a keyed error so the
                    // busy is replaced (closes the previously-documented gap where
                    // a failed lookup stranded its busy to the timeout warning).
                    // TUI path (`status_op_id == None`) keeps the unkeyed error.
                    if let Some(id) = status_op_id
                        && let Some(op) = self.pending_web_pr_lookup_ops.remove(&id)
                    {
                        op.resolve(&crate::engine::WebPrLookupOutcome::Failed { message })
                            .into_reaction()
                    } else {
                        EventReaction::Status(StatusUpdate::error(message))
                    }
                }
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
            WorkerEvent::ProjectWorktreesReady {
                project_id,
                result,
                status_op_id,
            } => EventReaction::ProjectWorktreesArrived {
                project_id,
                result,
                status_op_id,
            },
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
                self.clear_in_flight(&InFlightKey::ResourceStats);
                EventReaction::ResourceStatsArrived(stats)
            }
            WorkerEvent::NonDefaultBranchCheckoutCompleted {
                action,
                target_branch,
                result,
                status_op_id,
            } => {
                match result {
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
                            // The SUCCESS message is built in `drive_add_project_followup`
                            // after the inline add, so the op is resolved there, not here.
                            status_op_id,
                        },
                        NonDefaultBranchAction::CheckoutProjectDefault { project } => {
                            if let Some(existing) =
                                self.projects.iter_mut().find(|p| p.id == project.id)
                            {
                                existing.current_branch = target_branch.clone();
                                existing.branch_status = ProjectBranchStatus::Leading;
                            }
                            // Web path: resolve the checkout op into its keyed info
                            // final (same message). TUI path keeps the unkeyed Status.
                            if let Some(id) = status_op_id
                                && let Some(op) = self.pending_web_checkout_ops.remove(&id)
                            {
                                op.resolve(&crate::engine::WebCheckoutOutcome::Ok { target_branch })
                                    .into_reaction()
                            } else {
                                EventReaction::Status(StatusUpdate::info(format!(
                                    "Checked out \"{target_branch}\" for project \"{}\".",
                                    project.name
                                )))
                            }
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
                        // Web path: resolve the matching op into a keyed error (same
                        // message). The op kind depends on the action: a checkout-default
                        // failure resolves the checkout op, an add-project switch failure
                        // resolves the add-project op. TUI path keeps the unkeyed Status.
                        if let Some(id) = status_op_id {
                            match action {
                                NonDefaultBranchAction::CheckoutProjectDefault { .. } => {
                                    if let Some(op) = self.pending_web_checkout_ops.remove(&id) {
                                        return op
                                            .resolve(&crate::engine::WebCheckoutOutcome::Failed {
                                                target_branch,
                                                repo_path: path,
                                            })
                                            .into_reaction();
                                    }
                                }
                                NonDefaultBranchAction::AddProject { .. } => {
                                    if let Some(op) = self.pending_web_add_project_ops.remove(&id) {
                                        return op
                                        .resolve(&crate::engine::WebAddProjectOutcome::SwitchFailed {
                                            target_branch,
                                            repo_path: path,
                                        })
                                        .into_reaction();
                                    }
                                }
                            }
                        }
                        EventReaction::Status(StatusUpdate::error(format!(
                            "Couldn't check out \"{target_branch}\" in {path} — resolve in your terminal and retry."
                        )))
                    }
                }
            }
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
            WorkerEvent::CheckoutProjectDefaultBranchInspected {
                project,
                result,
                status_op_id,
            } => {
                // Web path: every terminal outcome of worker 1 must resolve the
                // checkout op's busy. The Known case forwards the id to worker 2
                // (which resolves later); the short-circuit cases (already-leading,
                // heuristic, inspect-failed) resolve it here. Helper closure pops
                // the op (if any) so the byte-identical message can be re-emitted
                // either keyed (web) or unkeyed (TUI).
                match result {
                    Ok((current_branch, warning_kind)) => match warning_kind {
                        Some(BranchWarningKind::Known { default_branch }) => {
                            let mut project = project;
                            project.current_branch = current_branch;
                            EventReaction::DispatchProjectDefaultBranchCheckout {
                                project,
                                default_branch,
                                status_op_id,
                            }
                        }
                        Some(BranchWarningKind::Heuristic) => {
                            if let Some(id) = status_op_id
                                && let Some(op) = self.pending_web_checkout_ops.remove(&id)
                            {
                                op.resolve(&crate::engine::WebCheckoutOutcome::Heuristic {
                                    current_branch,
                                })
                                .into_reaction()
                            } else {
                                EventReaction::Status(StatusUpdate::error(format!(
                                    "Can't determine the default branch for project \"{}\" while it is on \"{}\". Resolve the default branch in your terminal and retry.",
                                    project.name, current_branch
                                )))
                            }
                        }
                        None => {
                            if let Some(existing) =
                                self.projects.iter_mut().find(|p| p.id == project.id)
                            {
                                existing.current_branch = current_branch.clone();
                                existing.branch_status = ProjectBranchStatus::Leading;
                            }
                            if let Some(id) = status_op_id
                                && let Some(op) = self.pending_web_checkout_ops.remove(&id)
                            {
                                op.resolve(&crate::engine::WebCheckoutOutcome::AlreadyLeading {
                                    current_branch,
                                })
                                .into_reaction()
                            } else {
                                EventReaction::Status(StatusUpdate::info(format!(
                                    "Project \"{}\" is already on the leading branch \"{}\".",
                                    project.name, current_branch
                                )))
                            }
                        }
                    },
                    Err(err) => {
                        if let Some(id) = status_op_id
                            && let Some(op) = self.pending_web_checkout_ops.remove(&id)
                        {
                            op.resolve(&crate::engine::WebCheckoutOutcome::InspectFailed {
                                error: err,
                            })
                            .into_reaction()
                        } else {
                            EventReaction::Status(StatusUpdate::error(format!(
                                "Couldn't inspect the default branch for project \"{}\": {err}",
                                project.name
                            )))
                        }
                    }
                }
            }
            WorkerEvent::ConfigReloadReady(result) => self.process_config_reload_ready(*result),
            WorkerEvent::ProjectPersistenceCompleted {
                action,
                result,
                status_op_id,
            } => {
                let outcome =
                    self.process_project_persistence_completed(action, result, status_op_id);
                EventReaction::ProjectPersistenceOutcome(Box::new(outcome))
            }
            WorkerEvent::AuthUsersPersisted {
                users,
                message,
                warn,
                result,
                status_op_id,
            } => {
                // Clear the single-flight guard on BOTH outcomes so the next
                // add/remove can start; opening either prompt while this was set
                // was refused (see App::open_server_add_user/open_server_remove_user).
                self.clear_in_flight(&InFlightKey::AuthUsers);
                match result {
                    Ok(()) => {
                        // If a config reload is in progress, the eager writer is
                        // quiesced and would reject this write. Stash the payload
                        // (and its op id) and replay it when `ConfigReloadReady`
                        // closes the barrier (after the reloaded config is
                        // applied), so the user-initiated change is never silently
                        // lost. The op stays stashed on the App side until the
                        // replay emits its final. Multiple arrivals within one
                        // reload window fold: the last one wins.
                        if self.reloading {
                            self.pending_auth_users = Some((users, message, warn, status_op_id));
                            return EventReaction::Nothing;
                        }
                        // Adopt the new user list and write it to config via the
                        // eager queue; roll back in-memory state on write failure.
                        // The tone/message decision is carried back as an outcome:
                        // the App resolves its HandlerStatusOp against it, or (the
                        // None callers) builds the StatusUpdate via into_status.
                        let previous = self.config.auth.users.clone();
                        self.config.auth.users = users;
                        let outcome = match self.config_writer.save_eager(self.config.clone()) {
                            Ok(()) => AuthUserFinalOutcome::Saved { warn, message },
                            Err(err) => {
                                self.config.auth.users = previous;
                                AuthUserFinalOutcome::Errored(err.to_string())
                            }
                        };
                        EventReaction::AuthUsersOutcome {
                            outcome,
                            status_op_id,
                        }
                    }
                    Err(err) => EventReaction::AuthUsersOutcome {
                        outcome: AuthUserFinalOutcome::Errored(err),
                        status_op_id,
                    },
                }
            }
            WorkerEvent::StartupCommandLogsLoaded {
                scope_label,
                result,
            } => match result {
                Ok(log) => EventReaction::StartupLogArrived { scope_label, log },
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Could not read startup command logs for {scope_label}: {err}"
                ))),
            },
            WorkerEvent::ServerFlipPreflightReady { result, warning } => {
                // No engine domain state to mutate — the listeners and the flip
                // are TUI concerns. Hand them straight to the App.
                EventReaction::ServerFlipPreflightReady { result, warning }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderCommandConfig;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::model::{
        GhStatus, PrInfo, PrState, ProjectBranchStatus, ProviderKind, SessionStatus,
    };
    use crate::worker::{
        AgentLaunchFailedData, AgentLaunchKind, AgentLaunchRequest, CreateAgentRequest, PullTarget,
        WorkerEvent,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

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
    fn finish_delete_session_removes_companion_terminals() {
        let (mut engine, _tmp) = test_engine();

        // A real worktree directory the companion PTY can `cwd` into.
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat/x");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // `cat` is always on PATH and simply echoes — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");
        assert!(
            engine
                .companion_terminals
                .values()
                .any(|t| t.session_id == "s1")
        );

        engine
            .finish_delete_session("s1")
            .unwrap()
            .expect("outcome");

        assert!(
            !engine
                .companion_terminals
                .values()
                .any(|t| t.session_id == "s1"),
            "deleted session's companion terminals should be removed"
        );
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
            EventReaction::ClearStatus(_) => "ClearStatus",
            EventReaction::Multi(_) => "Multi",
            EventReaction::RebuildLeftItems => "RebuildLeftItems",
            EventReaction::ReloadChangedFiles => "ReloadChangedFiles",
            EventReaction::ClampFilesCursor => "ClampFilesCursor",
            EventReaction::AgentLaunchReadyView(_) => "AgentLaunchReadyView",
            EventReaction::AgentLaunchFailedView(_) => "AgentLaunchFailedView",
            EventReaction::CommitMessageGenerated { .. } => "CommitMessageGenerated",
            EventReaction::CommitMessageFailed { .. } => "CommitMessageFailed",
            EventReaction::BrowserEntriesArrived { .. } => "BrowserEntriesArrived",
            EventReaction::ProjectWorktreesArrived { .. } => "ProjectWorktreesArrived",
            EventReaction::OpenNewAgentPromptForPr { .. } => "OpenNewAgentPromptForPr",
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
            EventReaction::AuthUsersOutcome { .. } => "AuthUsersOutcome",
            EventReaction::StartupLogArrived { .. } => "StartupLogArrived",
            EventReaction::FinishDeleteSessionView(_) => "FinishDeleteSessionView",
            EventReaction::DoDeleteSessionView(_) => "DoDeleteSessionView",
            EventReaction::BeginDeleteSessionView(_) => "BeginDeleteSessionView",
            EventReaction::DispatchAgentLaunchView(_) => "DispatchAgentLaunchView",
            EventReaction::DeleteTerminalView(_) => "DeleteTerminalView",
            EventReaction::ServerFlipPreflightReady { .. } => "ServerFlipPreflightReady",
        }
    }

    // ── PullCompleted (Project) ──────────────────────────────────────────

    #[test]
    fn pull_completed_project_ok_updates_branch_and_clears_inflight() {
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        engine.projects.push(project);
        let repo_path = "/tmp/p1".to_string();
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: Some("main".to_string()),
            },
            result: Ok(Some("feature-x".to_string())),
            status: crate::engine::ResolvedFinal::new(
                "pull-project:p1",
                crate::engine::Final::info(
                    "Refreshed project \"p1-name\". Local branch is up to date with remote.",
                ),
            ),
        });

        // In-flight entry is cleared regardless of result.
        assert!(!engine.is_in_flight(&InFlightKey::Pull(repo_path.clone())));

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
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: None,
            },
            result: Err("network down".to_string()),
            status: crate::engine::ResolvedFinal::new(
                "pull-project:p1",
                crate::engine::Final::error("Project refresh failed for \"p1-name\": network down"),
            ),
        });

        assert!(!engine.is_in_flight(&InFlightKey::Pull(repo_path.clone())));
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

    // ── ChangedFilesReady (stale-poll race / CF1 invariant) ──────────────

    fn sample_changed_file(path: &str) -> crate::model::ChangedFile {
        crate::model::ChangedFile {
            status: "M".to_string(),
            path: path.to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
        }
    }

    #[test]
    fn changed_files_ready_matching_worktree_applies_and_clamps() {
        let (mut engine, _tmp) = test_engine();
        let worktree = PathBuf::from("/tmp/wt-current");
        *engine.watched_worktree.lock().unwrap() = Some(worktree.clone());

        let reaction = engine.process_worker_event(WorkerEvent::ChangedFilesReady {
            staged: vec![sample_changed_file("staged.txt")],
            unstaged: vec![sample_changed_file("unstaged.txt")],
            worktree,
        });

        // The view follow-up that repaints the TUI's changed-files pane.
        assert!(matches!(reaction, EventReaction::ClampFilesCursor));
        assert_eq!(engine.staged_files.len(), 1);
        assert_eq!(engine.staged_files[0].path, "staged.txt");
        assert_eq!(engine.unstaged_files.len(), 1);
        assert_eq!(engine.unstaged_files[0].path, "unstaged.txt");
    }

    #[test]
    fn changed_files_ready_stale_worktree_is_dropped() {
        let (mut engine, _tmp) = test_engine();
        // Watch has since moved to a different worktree.
        *engine.watched_worktree.lock().unwrap() = Some(PathBuf::from("/tmp/wt-now"));
        // Seed existing lists so we can prove they are left untouched.
        engine.staged_files = vec![sample_changed_file("keep-staged.txt")];
        engine.unstaged_files = vec![sample_changed_file("keep-unstaged.txt")];

        let reaction = engine.process_worker_event(WorkerEvent::ChangedFilesReady {
            staged: vec![sample_changed_file("stale-staged.txt")],
            unstaged: vec![sample_changed_file("stale-unstaged.txt")],
            // Computed for the worktree we have since stopped watching.
            worktree: PathBuf::from("/tmp/wt-stale"),
        });

        // Dropped: no view follow-up, and engine state is unchanged.
        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.staged_files.len(), 1);
        assert_eq!(engine.staged_files[0].path, "keep-staged.txt");
        assert_eq!(engine.unstaged_files.len(), 1);
        assert_eq!(engine.unstaged_files[0].path, "keep-unstaged.txt");
    }

    #[test]
    fn changed_files_ready_dropped_when_watch_cleared() {
        let (mut engine, _tmp) = test_engine();
        // No worktree watched (the watch was cleared, e.g. no session focused).
        assert!(engine.watched_worktree.lock().unwrap().is_none());
        engine.staged_files = vec![sample_changed_file("keep.txt")];

        let reaction = engine.process_worker_event(WorkerEvent::ChangedFilesReady {
            staged: vec![sample_changed_file("stale.txt")],
            unstaged: vec![sample_changed_file("stale.txt")],
            worktree: PathBuf::from("/tmp/wt-stale"),
        });

        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.staged_files.len(), 1);
        assert_eq!(engine.staged_files[0].path, "keep.txt");
        assert!(engine.unstaged_files.is_empty());
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
        engine.mark_in_flight(InFlightKey::CreateAgent);

        let reaction = engine.process_worker_event(WorkerEvent::CreateAgentFailed {
            key: "create:p1".to_string(),
            message: "nope".to_string(),
        });

        assert!(!engine.is_in_flight(&InFlightKey::CreateAgent));
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.message, "nope");
        // The failure must carry the same create key as the busy so the web
        // clears the "Creating a new agent…" loading toast on completion.
        assert_eq!(status.key.as_deref(), Some("create:p1"));
    }

    #[test]
    fn create_agent_progress_is_keyed_with_the_create_key() {
        // Regression: progress updates used to be emitted unkeyed, landing in the
        // anonymous status slot. That slot is never expired by the busy timeout,
        // and the keyed success/failure (create:{project_id}) could not dismiss
        // it, so a "Launching … in a fresh session…" toast lingered forever on
        // the web. The progress MUST carry the same create key as its final.
        let (mut engine, _tmp) = test_engine();

        let reaction = engine.process_worker_event(WorkerEvent::CreateAgentProgress {
            key: "create:p1".to_string(),
            message: "Launching codex in a fresh session...".to_string(),
        });

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Busy);
        assert_eq!(status.key.as_deref(), Some("create:p1"));
    }

    #[test]
    fn create_progress_busy_is_dismissed_by_the_keyed_failure() {
        // End-to-end on the keyed controller: a busy progress on create:p1
        // followed by a keyed error on the same key replaces it in place, so the
        // controller never strands a busy entry (and the web toast is reused,
        // not duplicated).
        use crate::statusline::{KeyedStatusController, StatusTone as Tone};
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut controller = KeyedStatusController::with_clear_after(Duration::from_secs(6));

        // Simulate the wire projection: progress busy then failure, same key.
        let progress = engine_progress_status();
        controller.set(
            now,
            progress.key.clone(),
            Tone::Busy,
            progress.message.clone(),
        );
        assert_eq!(controller.snapshot().len(), 1);
        assert_eq!(controller.snapshot()[0].tone, "busy");

        controller.set(
            now,
            Some("create:p1".to_string()),
            Tone::Error,
            "Failed to create a new worktree.",
        );
        // Still one entry on the same key — the busy was replaced, not stacked.
        let snap = controller.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].key.as_deref(), Some("create:p1"));
        assert_eq!(snap[0].tone, "error");
    }

    fn engine_progress_status() -> StatusUpdate {
        let (mut engine, _tmp) = test_engine();
        unwrap_status(
            engine.process_worker_event(WorkerEvent::CreateAgentProgress {
                key: "create:p1".to_string(),
                message: "Attaching to existing branch \"x\" for project \"y\"...".to_string(),
            }),
        )
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
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".to_string()));
        engine.mark_in_flight(InFlightKey::CreateAgent);
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
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
        assert!(!engine.is_in_flight(&InFlightKey::CreateAgent));
        assert!(
            matches!(outcome, AgentLaunchFailedOutcome::Create { message, .. } if message == "boom")
        );
    }

    #[test]
    fn process_agent_launch_failed_resume_fallback_marks_detached() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "project-1", "feat/x");
        let _ = engine.session_store.upsert_session(&session);
        engine.sessions.push(session);
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".to_string()));

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
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
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
            AgentLaunchFailedOutcome::Reconnect { session_id, branch_name, message }
                if session_id == "s1" && branch_name == "feat/x" && message == "boom"
        ));
    }

    #[test]
    fn process_agent_launch_failed_startup_auto_reopen_returns_branch_and_message() {
        let (mut engine, _tmp) = test_engine();
        let data = make_failed_data("s1", "feat/x", AgentLaunchKind::StartupAutoReopen, "boom");
        let outcome = engine.process_agent_launch_failed(data);
        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::StartupAutoReopen { session_id, branch_name, message }
                if session_id == "s1" && branch_name == "feat/x" && message == "boom"
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
        let outcome = engine.process_project_persistence_completed(action, Ok(()), None);
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
        let outcome = engine.process_project_persistence_completed(action, Ok(()), None);
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
        let outcome = engine.process_project_persistence_completed(action, Ok(()), None);
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
        let outcome = engine.process_project_persistence_completed(
            action,
            Err("disk full".to_string()),
            None,
        );
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
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::Inline {
                removal: WorktreeRemoval::PreservedOrphan
            }
        ));
        assert!(!engine.pending_deletions.contains("s1"));
    }

    #[test]
    fn begin_delete_orphan_session_returns_inline_not_not_found() {
        // A session whose project record is gone (orphan): no projects.push.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "ghost", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        // Even requesting worktree removal, a missing project takes the inline
        // path (we cannot run git worktree remove without the repo) — NOT NotFound,
        // which would silently no-op the user's delete.
        let outcome = engine.begin_delete_session("s1", true);
        assert!(matches!(outcome, BeginDeleteSessionOutcome::Inline { .. }));
    }

    #[test]
    fn remove_project_command_cascades_sessions_keeping_worktrees() {
        let (mut engine, _tmp) = test_engine();
        // Two projects so the removal must keep the OTHER one untouched, and both
        // exist as real store rows so we can prove the project row itself is gone.
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        engine.projects.push(sample_project("p2", "/tmp/p2"));
        engine
            .session_store
            .upsert_project(&crate::engine::project_to_project_config(
                &engine.projects[0],
            ))
            .unwrap();
        engine
            .session_store
            .upsert_project(&crate::engine::project_to_project_config(
                &engine.projects[1],
            ))
            .unwrap();
        let s1 = sample_session("s1", "p1", "feat/a");
        let s2 = sample_session("s2", "p1", "feat/b");
        let s3 = sample_session("s3", "p2", "feat/c");
        for s in [&s1, &s2, &s3] {
            engine.session_store.upsert_session(s).unwrap();
        }
        engine.sessions.push(s1);
        engine.sessions.push(s2);
        engine.sessions.push(s3);
        // A PR row on a doomed session proves the cascade clears session_prs too
        // (the FK cascade is unenforced, so the engine path must do it explicitly).
        engine
            .session_store
            .upsert_pr(&StoredPr {
                session_id: "s1".to_string(),
                pr_number: 7,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "open".to_string(),
                title: "t".to_string(),
                url: "u".to_string(),
            })
            .unwrap();

        let reaction = engine
            .apply(crate::engine::Command::RemoveProject {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
            })
            .expect("remove project");

        // Only p1 and its sessions are gone — from memory AND the store records,
        // synchronously and atomically (sessions, PR rows, and the project row).
        let session_ids: Vec<String> = engine.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(session_ids, vec!["s3".to_string()]);
        let project_ids: Vec<String> = engine.projects.iter().map(|p| p.id.clone()).collect();
        assert_eq!(project_ids, vec!["p2".to_string()]);
        let stored_sessions: Vec<String> = engine
            .session_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(stored_sessions, vec!["s3".to_string()]);
        let stored_projects: Vec<String> = engine
            .session_store
            .load_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(stored_projects, vec!["p2".to_string()]);
        assert!(
            engine
                .session_store
                .load_all_latest_prs()
                .unwrap()
                .is_empty()
        );
        // A single success status is emitted (no silent removal).
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert!(status.message.contains("Removed project \"p1-name\""));
    }

    #[test]
    fn remove_project_command_refuses_while_an_agent_deletion_is_pending() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let s1 = sample_session("s1", "p1", "feat/a");
        engine.session_store.upsert_session(&s1).unwrap();
        engine.sessions.push(s1);
        // One of the project's agents has an in-flight async worktree removal.
        engine.pending_deletions.insert("s1".to_string());

        let reaction = engine
            .apply(crate::engine::Command::RemoveProject {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
            })
            .expect("remove project");

        // The guard refuses with an error and mutates nothing — the session row,
        // the project, and the in-memory state all survive for a later retry.
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(engine.sessions.len(), 1);
        assert_eq!(engine.projects.len(), 1);
        assert_eq!(engine.session_store.load_sessions().unwrap().len(), 1);
    }

    #[test]
    fn remove_ghost_project_command_clears_orphaned_sessions() {
        let (mut engine, _tmp) = test_engine();
        // Orphaned sessions: a project_id present on sessions with no project row.
        let s1 = sample_session("s1", "ghost", "feat/a");
        engine.session_store.upsert_session(&s1).unwrap();
        engine.sessions.push(s1);

        engine
            .apply(crate::engine::Command::RemoveProject {
                project_id: "ghost".to_string(),
                project_name: "ghost".to_string(),
            })
            .expect("remove ghost project");

        assert!(engine.sessions.is_empty());
        assert!(engine.session_store.load_sessions().unwrap().is_empty());
    }

    #[test]
    fn begin_delete_inline_preserved_orphan_when_no_delete_no_siblings() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine.begin_delete_session("s1", false);
        match outcome {
            BeginDeleteSessionOutcome::Inline { removal } => {
                assert_eq!(removal, WorktreeRemoval::PreservedOrphan);
            }
            other => panic!("expected Inline, got {other:?}"),
        }
    }

    #[test]
    fn begin_delete_inline_preserved_shared_when_no_delete_with_sibling() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let mut a = sample_session("s1", "p1", "feat/x");
        let mut b = sample_session("s2", "p1", "feat/y");
        a.worktree_path = "/tmp/shared".to_string();
        b.worktree_path = "/tmp/shared".to_string();
        engine.session_store.upsert_session(&a).unwrap();
        engine.session_store.upsert_session(&b).unwrap();
        engine.sessions.push(a);
        engine.sessions.push(b);

        let outcome = engine.begin_delete_session("s1", false);
        match outcome {
            BeginDeleteSessionOutcome::Inline { removal } => {
                assert_eq!(removal, WorktreeRemoval::PreservedShared);
            }
            other => panic!("expected Inline, got {other:?}"),
        }
    }

    #[test]
    fn begin_delete_inline_skipped_for_siblings_when_delete_with_sibling() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let mut a = sample_session("s1", "p1", "feat/x");
        let mut b = sample_session("s2", "p1", "feat/y");
        a.worktree_path = "/tmp/shared".to_string();
        b.worktree_path = "/tmp/shared".to_string();
        engine.session_store.upsert_session(&a).unwrap();
        engine.session_store.upsert_session(&b).unwrap();
        engine.sessions.push(a);
        engine.sessions.push(b);

        // delete_worktree=true but a sibling shares the worktree → skipped,
        // so this stays on the inline path (no git removal needed).
        let outcome = engine.begin_delete_session("s1", true);
        match outcome {
            BeginDeleteSessionOutcome::Inline { removal } => {
                assert_eq!(removal, WorktreeRemoval::SkippedForSiblings);
            }
            other => panic!("expected Inline, got {other:?}"),
        }
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

    #[test]
    fn do_delete_session_soft_returns_when_async_worker_in_flight() {
        // Fix #9: the in-flight guard must hold in release builds. If an
        // async delete worker is already running for this session, the
        // synchronous path must NOT proceed to `git::remove_worktree` or
        // touch in-memory state — otherwise the two paths would race on
        // the worktree.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine.pending_deletions.insert("s1".to_string());

        let outcome = engine
            .do_delete_session("s1", true)
            .expect("soft-return does not error");
        assert!(
            outcome.is_none(),
            "do_delete_session must soft-return Ok(None) when an async worker is in-flight",
        );
        // The session must still be present — we soft-returned, did not delete.
        assert!(
            engine.sessions.iter().any(|s| s.id == "s1"),
            "session should be untouched when the in-flight guard fires",
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
                removal: crate::engine::WorktreeRemoval::PreservedOrphan,
                update_status: true,
            })
            .unwrap();
        assert!(matches!(reaction, EventReaction::Nothing));
    }

    #[test]
    fn apply_persist_project_add_writes_config_and_returns_outcome() {
        use crate::engine::events::ProjectPersistenceView;
        use crate::worker::ProjectPersistenceAction;
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        let action = ProjectPersistenceAction::Add {
            project: project.clone(),
            status_message: "added".to_string(),
        };
        let reaction = engine
            .apply(crate::engine::Command::PersistProject {
                action: Box::new(action),
                status_op_id: None,
            })
            .expect("apply succeeds");
        // Add is now inline: returns ProjectPersistenceOutcome directly, not Nothing.
        assert!(
            matches!(
                reaction,
                EventReaction::ProjectPersistenceOutcome(ref o)
                if matches!(o.view, ProjectPersistenceView::Added { ref project_id, .. } if project_id == "p1")
            ),
            "expected Added outcome for p1"
        );
        // The project must be in the in-memory list.
        assert!(engine.projects.iter().any(|p| p.id == "p1"));
        // The worker channel must be empty — no background worker was dispatched.
        assert!(
            engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "Add no longer dispatches a background worker"
        );
    }

    // ── Engine::apply on the agent-creation dispatch family (E4c) ───────────

    #[test]
    fn apply_dispatch_create_agent_request_returns_error_when_in_flight() {
        let (mut engine, _tmp) = test_engine();
        engine.mark_in_flight(InFlightKey::CreateAgent);
        let project = sample_project("p1", "/tmp/p1");
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: None,
            use_existing_branch: false,
            pull_before_create: false,
        };
        let reaction = engine
            .apply(crate::engine::Command::DispatchCreateAgentRequest {
                request: Box::new(request),
                busy_message: "busy".to_string(),
                term_size: (24, 80),
            })
            .expect("apply succeeds");
        assert!(matches!(
            reaction,
            EventReaction::Status(StatusUpdate {
                tone: StatusTone::Error,
                ..
            })
        ));
        // Engine state should be unchanged on the already-in-flight path.
        assert!(engine.is_in_flight(&InFlightKey::CreateAgent));
    }

    #[test]
    fn apply_dispatch_agent_launch_returns_already_launching_when_pending() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".to_string()));
        let request = AgentLaunchRequest {
            session,
            provider_config: ProviderCommandConfig::default(),
            env: Vec::new(),
            resume: false,
            pty_size: (24, 80),
            scrollback_lines: 1000,
            kind: AgentLaunchKind::Reconnect {
                status_message: String::new(),
            },
        };
        let reaction = engine
            .apply(crate::engine::Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply succeeds");
        let view = match reaction {
            EventReaction::DispatchAgentLaunchView(v) => *v,
            other => panic!(
                "expected DispatchAgentLaunchView, got {}",
                reaction_kind(&other)
            ),
        };
        assert!(!view.launched);
        assert!(view.status.is_some());
    }

    #[test]
    fn apply_stage_file_propagates_git_error_for_missing_worktree() {
        let (mut engine, _tmp) = test_engine();
        let result = engine.apply(crate::engine::Command::StageFile {
            worktree_path: PathBuf::from("/nonexistent/worktree"),
            path: "missing.rs".to_string(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn apply_pull_rejects_concurrent_pulls_for_same_repo() {
        let (mut engine, _tmp) = test_engine();
        let repo_path = PathBuf::from("/tmp/dummy-repo");
        engine.mark_in_flight(InFlightKey::Pull(repo_path.to_string_lossy().into_owned()));
        let reaction = engine
            .apply(crate::engine::Command::Pull {
                repo_path: repo_path.clone(),
                target: PullTarget::Session,
                busy_message: "busy".to_string(),
                already_running_message: "Pull already in progress".to_string(),
            })
            .expect("apply succeeds");
        assert!(matches!(
            reaction,
            EventReaction::Status(StatusUpdate {
                tone: StatusTone::Warning,
                ..
            })
        ));
    }

    // ── E4e: OpenPath, ToggleAgentAutoReopen, DeleteTerminal ────────────

    #[test]
    fn apply_toggle_agent_auto_reopen_updates_session_and_returns_status() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat/x");
        session.auto_reopen_enabled = false;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let reaction = engine
            .apply(crate::engine::Command::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                branch_name: "feat/x".to_string(),
                new_enabled: true,
            })
            .expect("apply succeeds");
        assert!(engine.sessions[0].auto_reopen_enabled);
        assert!(matches!(
            reaction,
            EventReaction::Status(StatusUpdate {
                tone: StatusTone::Info,
                ..
            })
        ));
    }

    #[test]
    fn apply_toggle_agent_auto_reopen_keeps_in_memory_state_when_db_write_fails() {
        // DB-first invariant: if the upsert fails, the in-memory session
        // must still hold the prior auto_reopen_enabled value so the UI
        // and the database stay consistent. Otherwise the user sees the
        // toggle "succeed" visually but silently revert on next restart.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat/x");
        session.auto_reopen_enabled = false;
        engine.session_store.upsert_session(&session).unwrap();
        let previous_updated_at = session.updated_at;
        engine.sessions.push(session);

        // Force the next upsert_session call to fail by dropping the
        // backing table out from under the engine.
        engine
            .session_store
            .break_sessions_table_for_test()
            .expect("break sessions table");

        let result = engine.apply(crate::engine::Command::ToggleAgentAutoReopen {
            session_id: "s1".to_string(),
            branch_name: "feat/x".to_string(),
            new_enabled: true,
        });

        assert!(result.is_err(), "expected toggle to surface the DB error");
        assert!(
            !engine.sessions[0].auto_reopen_enabled,
            "in-memory auto_reopen_enabled must not flip when the DB write fails",
        );
        assert_eq!(
            engine.sessions[0].updated_at, previous_updated_at,
            "updated_at must not advance when the DB write fails",
        );
    }

    #[test]
    fn apply_delete_terminal_returns_view_with_none_label_when_terminal_missing() {
        let (mut engine, _tmp) = test_engine();
        // Without a real PtyClient we can't construct a CompanionTerminal —
        // exercise only the "not present" path here. The label-present
        // path is covered by existing App-level tests (do_delete_terminal
        // is called from the confirm-delete-terminal flow).
        let reaction = engine
            .apply(crate::engine::Command::DeleteTerminal {
                terminal_id: "missing".to_string(),
            })
            .expect("apply succeeds");
        let view = match reaction {
            EventReaction::DeleteTerminalView(v) => *v,
            other => panic!("expected DeleteTerminalView, got {}", reaction_kind(&other)),
        };
        assert_eq!(view.terminal_id, "missing");
        assert!(view.label.is_none());
    }

    // Note: `Command::OpenPath` is intentionally NOT unit-tested here. The
    // apply arm spawns a detached thread that calls
    // `crate::startup::open_path` (which shells out to xdg-open / `open`),
    // and even though we only care about the synchronous Status reaction,
    // the spawned thread still fires the real system handler — producing a
    // desktop notification on dev machines and a flaky failure in CI. The
    // status-message formatting is trivial and exercised end-to-end by the
    // App-level startup-command-log open flow.

    // ── spawn_pr_check_for_session rate-limit (fix #1) ─────────────────────

    #[test]
    fn spawn_pr_check_for_session_skips_when_recently_checked() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.sessions.push(sample_session("s1", "p1", "feat/x"));
        // Pre-populate the rate-limit map with a fresh timestamp so the
        // 10-second guard short-circuits before any worker thread spawns.
        engine
            .pr_last_checked
            .insert("s1".to_string(), Instant::now());

        engine.spawn_pr_check_for_session("s1");

        // No worker was spawned, so nothing should have been posted to the
        // channel. A short timeout keeps the test responsive while still
        // proving the rate-limit short-circuit fired.
        assert!(
            engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "expected no worker event when rate-limit suppresses the check",
        );
    }

    #[test]
    fn spawn_pr_check_for_session_records_timestamp_before_spawning() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.sessions.push(sample_session("s1", "p1", "feat/x"));
        assert!(!engine.pr_last_checked.contains_key("s1"));

        let before = Instant::now();
        engine.spawn_pr_check_for_session("s1");

        // The timestamp must be recorded synchronously — before the worker
        // thread is spawned — so a burst of triggers within one tick cannot
        // all bypass the rate-limit. The exact Instant value isn't observable
        // across threads cleanly, so just verify an entry now exists and
        // that it is no older than the call site.
        let recorded = engine
            .pr_last_checked
            .get("s1")
            .copied()
            .expect("pr_last_checked entry should be recorded synchronously");
        assert!(
            recorded >= before,
            "recorded instant should be at or after the call site instant",
        );
        assert!(
            recorded.elapsed() < std::time::Duration::from_secs(1),
            "recorded instant should be very recent",
        );
    }

    // ── Command::PersistGlobalEnv / ReloadConfig / RecoverConfig ────────────
    //
    // PersistGlobalEnv now eager-saves through the engine's config writer;
    // ReloadConfig opens the reload barrier and drives the surface's reload;
    // RecoverConfig renders via the surface and writes synchronously.

    /// A recording `ConfigSurface` used by the dispatch tests below. It logs
    /// which method was called into a shared `Vec<String>` so the test can
    /// assert on dispatch, and posts `ConfigReloadReady` on reload.
    #[derive(Clone)]
    struct RecordingConfigSurface(Arc<Mutex<Vec<String>>>);

    impl crate::engine::ConfigSurface for RecordingConfigSurface {
        fn reload(
            &self,
            _paths: crate::config::DuxPaths,
            worker_tx: std::sync::mpsc::Sender<crate::worker::WorkerEvent>,
        ) {
            self.0.lock().unwrap().push("reload".into());
            crate::engine::ReloadCompletionGuard::new(worker_tx)
                .complete(Ok(crate::config::Config::default()));
        }

        fn recover_render(&self, _config: &crate::config::Config) -> String {
            self.0.lock().unwrap().push("recover_render".into());
            "# recovered\n".to_string()
        }
    }

    #[test]
    fn apply_persist_global_env_writes_through_queue() {
        let (mut engine, _tmp) = test_engine();
        let mut env = BTreeMap::new();
        env.insert("FOO".into(), "bar".into());
        let reaction = engine
            .apply(crate::engine::Command::PersistGlobalEnv { env })
            .expect("apply PersistGlobalEnv");
        // Eager save returns a synchronous Info status.
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        engine.config_writer.flush();
        assert!(
            std::fs::read_to_string(&engine.paths.config_path)
                .unwrap()
                .contains("FOO = \"bar\"")
        );
    }

    #[test]
    fn apply_reload_config_opens_barrier_and_invokes_surface() {
        let (mut engine, _tmp) = test_engine();
        let recorder = Arc::new(Mutex::new(Vec::new()));
        engine.surface = Box::new(RecordingConfigSurface(recorder.clone()));

        let reaction = engine
            .apply(crate::engine::Command::ReloadConfig)
            .expect("apply ReloadConfig");
        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(*recorder.lock().unwrap(), vec!["reload".to_string()]);
        // The barrier is open until ConfigReloadReady lands.
        assert!(engine.reloading);
        assert!(engine.reload_guard.is_some());
    }

    #[test]
    fn apply_recover_config_renders_via_surface_and_writes() {
        let (mut engine, _tmp) = test_engine();
        let recorder = Arc::new(Mutex::new(Vec::new()));
        engine.surface = Box::new(RecordingConfigSurface(recorder.clone()));

        let reaction = engine
            .apply(crate::engine::Command::RecoverConfig)
            .expect("apply RecoverConfig");
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            *recorder.lock().unwrap(),
            vec!["recover_render".to_string()]
        );
        // The rendered body was written to disk.
        assert_eq!(
            std::fs::read_to_string(&engine.paths.config_path).unwrap(),
            "# recovered\n"
        );
    }

    // ── spawn_command_worker primitive ────────────────────────────────────

    /// Drain a single `WorkerEvent` from `engine.worker_rx`, polling with a
    /// bounded sleep so a slow CI runner still gets a chance to deliver the
    /// background thread's event. Returns `None` if the budget is exhausted.
    fn try_recv_worker_event(engine: &Engine) -> Option<WorkerEvent> {
        for _ in 0..200 {
            if let Ok(event) = engine.worker_rx.try_recv() {
                return Some(event);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn command_worker_already_in_flight_returns_status() {
        use crate::engine::CommandWorkerSpec;

        let (mut engine, _tmp) = test_engine();
        engine.mark_in_flight(InFlightKey::CreateAgent);
        let reaction = engine.spawn_command_worker(
            CommandWorkerSpec {
                label: "create-agent".into(),
                in_flight_key: Some(InFlightKey::CreateAgent),
                busy_status: Some(StatusUpdate::busy("starting")),
                already_running_status: Some(StatusUpdate::error("already")),
                panic_event: None,
            },
            |_tx| panic!("job must not run when already in flight"),
        );
        match reaction {
            EventReaction::Status(status) => assert_eq!(status.message, "already"),
            other => panic!("expected Status, got {}", reaction_kind(&other)),
        }
        // The pre-existing in-flight key must still be present — the
        // primitive's guard does not clear keys it did not insert.
        assert!(engine.is_in_flight(&InFlightKey::CreateAgent));
        // No worker event should arrive — the job was never spawned.
        assert!(engine.worker_rx.try_recv().is_err());
    }

    #[test]
    fn command_worker_busy_status_arrives_before_completion() {
        use crate::engine::CommandWorkerSpec;

        let (mut engine, _tmp) = test_engine();
        let reaction = engine.spawn_command_worker(
            CommandWorkerSpec {
                label: "fifo-test".into(),
                in_flight_key: None,
                busy_status: Some(StatusUpdate::busy("starting")),
                already_running_status: None,
                panic_event: None,
            },
            |tx| {
                // The job's only side-effect is delivering a second event,
                // which lets the test assert FIFO ordering against the busy
                // status the primitive enqueued synchronously.
                let _ = tx.send(WorkerEvent::CommandWorkerStarted(StatusUpdate::info(
                    "done",
                )));
            },
        );
        assert!(matches!(reaction, EventReaction::Nothing));

        let first = engine
            .worker_rx
            .try_recv()
            .expect("busy status must be enqueued synchronously before the worker thread starts");
        match first {
            WorkerEvent::CommandWorkerStarted(status) => {
                assert_eq!(status.message, "starting");
            }
            other => panic!(
                "expected CommandWorkerStarted(starting), got {other:?}",
                other = std::any::type_name_of_val(&other)
            ),
        }

        let second = try_recv_worker_event(&engine).expect("worker completion event missing");
        match second {
            WorkerEvent::CommandWorkerStarted(status) => {
                assert_eq!(status.message, "done");
            }
            other => panic!(
                "expected CommandWorkerStarted(done), got {other:?}",
                other = std::any::type_name_of_val(&other)
            ),
        }
    }

    #[test]
    fn command_worker_clears_in_flight_on_panic() {
        use crate::engine::CommandWorkerSpec;

        let (mut engine, _tmp) = test_engine();
        let reaction = engine.spawn_command_worker(
            CommandWorkerSpec {
                label: "panic-test".into(),
                in_flight_key: Some(InFlightKey::CreateAgent),
                busy_status: None,
                already_running_status: None,
                panic_event: Some(Box::new(|reason| WorkerEvent::CreateAgentFailed {
                    key: "create:test".to_string(),
                    message: format!("panic: {reason}"),
                })),
            },
            |_tx| panic!("boom"),
        );
        assert!(matches!(reaction, EventReaction::Nothing));
        // The primitive marked the key synchronously; the worker is still
        // running, so the key is present until the synthesised failure
        // event is processed.
        assert!(engine.is_in_flight(&InFlightKey::CreateAgent));

        let event = try_recv_worker_event(&engine)
            .expect("synthesised CreateAgentFailed event must arrive after the panic");
        let message_contains_panic = matches!(&event, WorkerEvent::CreateAgentFailed { message: m, .. } if m.contains("boom"));
        assert!(
            message_contains_panic,
            "expected the synthesised failure event to carry the panic message",
        );

        // Routing through the normal completion-event handler is what
        // actually clears the in-flight key — the primitive does not
        // double-up on the cleanup path.
        let _ = engine.process_worker_event(event);
        assert!(!engine.is_in_flight(&InFlightKey::CreateAgent));
    }

    #[test]
    fn command_worker_no_busy_status_emits_no_started_event() {
        use crate::engine::CommandWorkerSpec;

        // Documents the silent-spawn path used by `spawn_resource_stats_worker`
        // and `Command::DispatchAgentLaunch`: when `busy_status` is `None`,
        // the primitive does not enqueue a `CommandWorkerStarted` event,
        // so the only thing on the channel is whatever the job itself sends.
        let (mut engine, _tmp) = test_engine();
        let reaction = engine.spawn_command_worker(
            CommandWorkerSpec {
                label: "silent".into(),
                in_flight_key: None,
                busy_status: None,
                already_running_status: None,
                panic_event: None,
            },
            |tx| {
                let _ = tx.send(WorkerEvent::ResourceStatsReady(Vec::new()));
            },
        );
        assert!(matches!(reaction, EventReaction::Nothing));

        let first = try_recv_worker_event(&engine).expect("job must produce a single event");
        assert!(
            matches!(first, WorkerEvent::ResourceStatsReady(ref rows) if rows.is_empty()),
            "expected ResourceStatsReady(empty), the silent-spawn path must not synthesise a CommandWorkerStarted event",
        );
        // No further events should be queued.
        assert!(engine.worker_rx.try_recv().is_err());
    }

    // ── spawn_background_worker primitive ─────────────────────────────────

    #[test]
    fn background_worker_logs_panic_without_event_when_panic_event_none() {
        use crate::engine::BackgroundWorkerSpec;

        // Documents the log-only panic path used by background workers whose
        // completion event has no failure variant (e.g. the PR-refresh
        // workers and `spawn_project_branch_status_checks`). The worker
        // panics; the primitive must not synthesise an event onto the
        // worker channel.
        let (mut engine, _tmp) = test_engine();
        engine.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "panic-no-event".into(),
                in_flight_key: None,
                panic_event: None,
            },
            |_tx| panic!("boom"),
        );

        // Wait long enough for the spawned thread to run and panic. The
        // primitive's catch_unwind catches the unwinding and logs; with
        // `panic_event: None` nothing is sent on the channel.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            engine.worker_rx.try_recv().is_err(),
            "no worker event should arrive when panic_event is None",
        );
    }

    // ── spawn_loop_worker primitive ───────────────────────────────────────

    // ── Keyed status pairs (Task 9) ──────────────────────────────────────

    #[test]
    fn pull_completed_project_ok_carries_keyed_status() {
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        engine.projects.push(project);
        let repo_path = "/tmp/p1".to_string();
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: None,
            },
            result: Ok(None),
            status: crate::engine::ResolvedFinal::new(
                "pull-project:p1",
                crate::engine::Final::info(
                    "Refreshed project \"p1-name\". Local branch is up to date with remote.",
                ),
            ),
        });

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            status.key.as_deref(),
            Some("pull-project:p1"),
            "pull-project completion must carry the keyed correlation key"
        );
    }

    #[test]
    fn pull_completed_project_err_carries_keyed_status() {
        let (mut engine, _tmp) = test_engine();
        let repo_path = "/tmp/p1".to_string();
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path,
            target: PullTarget::Project {
                project_id: "p1".to_string(),
                project_name: "p1-name".to_string(),
                leading_branch: None,
            },
            result: Err("network error".to_string()),
            status: crate::engine::ResolvedFinal::new(
                "pull-project:p1",
                crate::engine::Final::error(
                    "Project refresh failed for \"p1-name\": network error",
                ),
            ),
        });

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(
            status.key.as_deref(),
            Some("pull-project:p1"),
            "pull-project failure must carry the same key as the busy"
        );
    }

    #[test]
    fn pull_completed_session_ok_carries_keyed_status() {
        let (mut engine, _tmp) = test_engine();
        let repo_path = "/tmp/wt-session".to_string();
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Session,
            result: Ok(None),
            status: crate::engine::ResolvedFinal::new(
                "pull-session:/tmp/wt-session",
                crate::engine::Final::info(
                    "Pulled latest changes from remote successfully. Local branch is up to date.",
                ),
            ),
        });

        // Session pull success returns Multi([Status, ReloadChangedFiles]).
        let status = match reaction {
            EventReaction::Multi(ref items) => items
                .iter()
                .find_map(|r| {
                    if let EventReaction::Status(s) = r {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .expect("expected a Status inside Multi"),
            other => panic!("expected Multi, got {}", reaction_kind(&other)),
        };
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            status.key.as_deref(),
            Some("pull-session:/tmp/wt-session"),
            "pull-session completion must carry the keyed correlation key"
        );
    }

    #[test]
    fn pull_completed_session_err_carries_keyed_status() {
        let (mut engine, _tmp) = test_engine();
        let repo_path = "/tmp/wt-session".to_string();
        engine.mark_in_flight(InFlightKey::Pull(repo_path.clone()));

        let reaction = engine.process_worker_event(WorkerEvent::PullCompleted {
            repo_path: repo_path.clone(),
            target: PullTarget::Session,
            result: Err("no remote".to_string()),
            status: crate::engine::ResolvedFinal::new(
                "pull-session:/tmp/wt-session",
                crate::engine::Final::error("Pull from remote failed: no remote"),
            ),
        });

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(
            status.key.as_deref(),
            Some("pull-session:/tmp/wt-session"),
            "pull-session failure must carry the same key as the busy"
        );
    }

    #[test]
    fn status_update_with_key_builder_roundtrips() {
        let s = StatusUpdate::info("hello").with_key("my-key");
        assert_eq!(s.tone, StatusTone::Info);
        assert_eq!(s.message, "hello");
        assert_eq!(s.key.as_deref(), Some("my-key"));
    }

    #[test]
    fn status_update_keyed_constructor() {
        let s = StatusUpdate::keyed("op-key", StatusTone::Busy, "working…");
        assert_eq!(s.tone, StatusTone::Busy);
        assert_eq!(s.message, "working\u{2026}");
        assert_eq!(s.key.as_deref(), Some("op-key"));
    }

    #[test]
    fn status_update_helpers_default_to_no_key() {
        assert!(StatusUpdate::info("x").key.is_none());
        assert!(StatusUpdate::busy("x").key.is_none());
        assert!(StatusUpdate::warning("x").key.is_none());
        assert!(StatusUpdate::error("x").key.is_none());
    }

    #[test]
    fn loop_worker_continues_after_iteration_panic() {
        use crate::engine::{LoopControl, LoopWorkerSpec};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Documents the behaviour that distinguishes the loop primitive from
        // the one-shot ones: a panicking iteration must NOT kill the
        // long-running watcher. The body panics on iteration 0, returns
        // `Break` on iteration 1, and would return `Continue` thereafter.
        // The test passes if iteration 1 runs at all — that is only possible
        // if the panic on iteration 0 was caught and the loop continued.
        let (engine, _tmp) = test_engine();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_body = Arc::clone(&counter);
        engine.spawn_loop_worker(
            LoopWorkerSpec {
                label: "panic-loop-test".into(),
            },
            move |_tx| {
                let n = counter_for_body.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    panic!("first iteration panics");
                }
                if n == 1 {
                    LoopControl::Break
                } else {
                    LoopControl::Continue
                }
            },
        );

        // Wait until the second iteration has run.
        for _ in 0..200 {
            if counter.load(Ordering::Relaxed) >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            counter.load(Ordering::Relaxed) >= 2,
            "loop did not continue past panic; counter = {}",
            counter.load(Ordering::Relaxed),
        );
    }

    // ── AuthUsersPersisted reload-barrier deferral ───────────────────────

    /// Deliver AuthUsersPersisted while the reload barrier is open. The write
    /// must be deferred (Nothing returned, no config mutation), the in-flight
    /// guard must be cleared, and the payload stashed in pending_auth_users.
    #[test]
    fn auth_users_persisted_deferred_while_reloading() {
        let (mut engine, _tmp) = test_engine();
        // Open the reload barrier manually (mirrors what ReloadConfig does).
        engine.reloading = true;
        engine.mark_in_flight(InFlightKey::AuthUsers);

        let users = vec!["alice:hash".to_string()];
        let reaction = engine.process_worker_event(WorkerEvent::AuthUsersPersisted {
            users: users.clone(),
            message: "Added user alice.".to_string(),
            warn: false,
            result: Ok(()),
            status_op_id: Some("op-auth".to_string()),
        });

        // The reaction must be Nothing — not an error, not a status. The op stays
        // stashed on the App side; the replay emits the final later.
        assert!(
            matches!(reaction, EventReaction::Nothing),
            "expected Nothing during reload, got {}",
            reaction_kind(&reaction),
        );
        // In-flight guard is cleared regardless of deferral.
        assert!(!engine.is_in_flight(&InFlightKey::AuthUsers));
        // The payload is stashed, not applied to config yet. The op id rides
        // along so the later replay resolves the right op.
        assert!(
            matches!(
                engine.pending_auth_users.as_ref(),
                Some((_, _, _, Some(id))) if id == "op-auth"
            ),
            "payload (with its op id) must be stashed"
        );
        assert!(
            engine.config.auth.users.is_empty(),
            "config must not be mutated before barrier closes"
        );
    }

    /// When ConfigReloadReady closes the barrier, a stashed auth-users payload
    /// must be replayed: config.auth.users updated + config written to disk.
    #[test]
    fn auth_users_replayed_on_config_reload_ready() {
        let (mut engine, _tmp) = test_engine();

        // Stash a pending update as if AuthUsersPersisted arrived during reload.
        let users = vec!["bob:hash".to_string()];
        engine.pending_auth_users = Some((
            users.clone(),
            "Added user bob.".to_string(),
            false,
            Some("op-auth".to_string()),
        ));

        // Deliver a successful ConfigReloadReady with a config that has a
        // different (or empty) auth.users — the in-flight update must win.
        let reloaded = Config::default(); // auth.users is empty
        let reaction =
            engine.process_worker_event(WorkerEvent::ConfigReloadReady(Box::new(Ok(reloaded))));

        // pending_auth_users must be consumed.
        assert!(
            engine.pending_auth_users.is_none(),
            "pending_auth_users must be cleared after replay"
        );
        // Reproduce the SURFACE step: the surface (web actor / TUI) applies the
        // returned ApplyReloadedConfig back onto the engine and rebuilds the web
        // auth gate from it. The in-flight users must SURVIVE that re-apply. The
        // previous version of this test skipped this step and asserted only the
        // transient pre-surface `self.config`, masking a bug where the surfaced
        // config carried the OLD (reloaded, empty) user list — so the surface
        // reverted the change in memory and at the live auth gate.
        let surfaced = match &reaction {
            EventReaction::ApplyReloadedConfig(c) => Some((**c).clone()),
            EventReaction::Multi(v) => v.iter().find_map(|r| match r {
                EventReaction::ApplyReloadedConfig(c) => Some((**c).clone()),
                _ => None,
            }),
            _ => None,
        }
        .expect("a successful reload must surface an ApplyReloadedConfig for the surface to swap");
        engine
            .apply_reloaded_config(surfaced)
            .expect("surface re-apply of the reloaded config must succeed");

        // After the surface re-apply, the in-flight user update must still win over
        // the reloaded (empty) list.
        assert_eq!(
            engine.config.auth.users, users,
            "in-flight user update must survive the surface's config re-apply"
        );
        // The reaction must include the auth-users outcome, carrying the saved
        // message and the op id that rode in on the stash, so the App resolves
        // the right deferred op into the final.
        fn finds_auth_bob(r: &EventReaction) -> bool {
            match r {
                EventReaction::AuthUsersOutcome {
                    outcome: AuthUserFinalOutcome::Saved { message, .. },
                    status_op_id,
                } => {
                    message.contains("Added user bob") && status_op_id.as_deref() == Some("op-auth")
                }
                EventReaction::Multi(v) => v.iter().any(finds_auth_bob),
                _ => false,
            }
        }
        assert!(
            finds_auth_bob(&reaction),
            "reaction must surface the auth-users outcome with its op id; got {}",
            reaction_kind(&reaction),
        );
    }

    /// An AuthUsersPersisted failure (bcrypt or DB) arriving during a reload
    /// must NOT be stashed — the error is surfaced immediately and the
    /// in-flight guard is cleared.
    #[test]
    fn auth_users_error_during_reload_surfaces_immediately() {
        let (mut engine, _tmp) = test_engine();
        engine.reloading = true;
        engine.mark_in_flight(InFlightKey::AuthUsers);

        let reaction = engine.process_worker_event(WorkerEvent::AuthUsersPersisted {
            users: vec![],
            message: String::new(),
            warn: false,
            result: Err("bcrypt failed".to_string()),
            status_op_id: Some("op-auth".to_string()),
        });

        // Error is surfaced immediately, not deferred. The outcome carries the
        // op id so the App resolves the matching op to the error final, and
        // `into_status` reproduces the byte-identical error message.
        let (outcome, status_op_id) = match reaction {
            EventReaction::AuthUsersOutcome {
                outcome,
                status_op_id,
            } => (outcome, status_op_id),
            other => panic!(
                "expected AuthUsersOutcome reaction, got {}",
                reaction_kind(&other)
            ),
        };
        assert_eq!(status_op_id.as_deref(), Some("op-auth"));
        assert!(
            matches!(&outcome, AuthUserFinalOutcome::Errored(e) if e.contains("bcrypt failed")),
            "error detail must be carried; got: {outcome:?}"
        );
        let status = outcome.into_status(status_op_id);
        assert_eq!(status.tone, StatusTone::Error);
        assert!(
            status.message.contains("bcrypt failed"),
            "error message must be surfaced; got: {}",
            status.message,
        );
        // Nothing stashed.
        assert!(engine.pending_auth_users.is_none());
        // In-flight guard cleared.
        assert!(!engine.is_in_flight(&InFlightKey::AuthUsers));
    }

    /// When multiple AuthUsersPersisted events arrive during a single reload,
    /// only the last one is replayed (last-write-wins fold).
    #[test]
    fn auth_users_last_write_wins_within_one_reload() {
        let (mut engine, _tmp) = test_engine();
        engine.reloading = true;

        // First event.
        let _ = engine.process_worker_event(WorkerEvent::AuthUsersPersisted {
            users: vec!["alice:hash".to_string()],
            message: "Added alice.".to_string(),
            warn: false,
            result: Ok(()),
            status_op_id: Some("op-alice".to_string()),
        });
        // Second event (arrives before barrier closes).
        let _ = engine.process_worker_event(WorkerEvent::AuthUsersPersisted {
            users: vec!["alice:hash".to_string(), "bob:hash".to_string()],
            message: "Added bob.".to_string(),
            warn: false,
            result: Ok(()),
            status_op_id: Some("op-bob".to_string()),
        });

        // Only the second payload is stashed (its op id too).
        let (stashed_users, stashed_msg, _, stashed_op_id) = engine
            .pending_auth_users
            .take()
            .expect("payload must be stashed");
        assert_eq!(stashed_users.len(), 2, "second (last) event must win");
        assert_eq!(
            stashed_op_id.as_deref(),
            Some("op-bob"),
            "last event's op id must win the fold"
        );
        assert!(
            stashed_msg.contains("bob"),
            "message must be from last event"
        );
    }

    // ── Panic-safety: worktree-remove worker ─────────────────────────────

    /// A panicking worktree-remove worker must still post
    /// `WorktreeRemoveCompleted { result: Err(_) }` so the engine can clear
    /// `pending_deletions` and surface the failure. This test exercises the
    /// `catch_unwind` wrapper added to `begin_delete_session` by spawning an
    /// equivalent thread, triggering a deliberate panic, and asserting that the
    /// synthesised error event arrives on the channel and that
    /// `process_worker_event` then clears the pending state.
    #[test]
    fn worktree_remove_panic_posts_failure_event_and_clears_pending() {
        let (mut engine, _tmp) = test_engine();

        // Pre-load the pending state as `begin_delete_session` would.
        engine.pending_deletions.insert("s1".to_string());
        engine
            .deletion_busy_messages
            .insert("s1".to_string(), "Removing worktree…".to_string());

        // Spawn a thread that mimics the catch_unwind wrapper in
        // `begin_delete_session` but with a deliberately panicking body.
        let tx = engine.worker_tx.clone();
        let sid = "s1".to_string();
        let handle = std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<bool, String> {
                panic!("simulated git failure");
            }))
            .unwrap_or_else(|payload| {
                let reason = crate::engine::format_panic_payload(payload);
                Err(format!("Worker panicked: {reason}"))
            });
            let _ = tx.send(crate::worker::WorkerEvent::WorktreeRemoveCompleted {
                session_id: sid,
                result,
            });
        });
        handle
            .join()
            .expect("thread should not panic at outer level");

        // The event must have arrived.
        let event = engine
            .worker_rx
            .try_recv()
            .expect("WorktreeRemoveCompleted must be on the channel");
        let reaction = engine.process_worker_event(event);

        // Pending state must be cleared by the event handler.
        assert!(
            !engine.pending_deletions.contains("s1"),
            "pending_deletions must be cleared after a panicked removal"
        );
        assert!(
            !engine.deletion_busy_messages.contains_key("s1"),
            "deletion_busy_messages must be cleared after a panicked removal"
        );

        // The reaction must be the failure variant so the UI surfaces the error.
        match reaction {
            EventReaction::WorktreeRemoveFailed {
                session_id,
                message,
            } => {
                assert_eq!(session_id, "s1");
                assert!(
                    message.contains("simulated git failure"),
                    "failure message must include the panic reason; got: {message}"
                );
            }
            other => panic!(
                "expected WorktreeRemoveFailed after a panicked removal, got {}",
                reaction_kind(&other)
            ),
        }
    }
}
