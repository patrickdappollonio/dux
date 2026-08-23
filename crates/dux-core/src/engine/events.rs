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
use crate::engine::{CreateLaunchOutcome, Engine, InFlightKey, ResolvedFinal};
use crate::logger;
use crate::model::{
    AgentSession, GhStatus, PrState, Project, ProjectBranchStatus, ProviderKind, SessionStatus,
};
use crate::startup::StartupCommandLogListing;
use crate::statusline::{StatusScope, StatusTone};
use crate::storage::StoredPr;
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchKind, AgentLaunchReadyData, BranchWarningKind, BrowserEntry,
    CreateAgentBranchInspection, NonDefaultBranchAction, ProjectPersistenceAction,
    ProjectWorktreeEntry, PullTarget, ResolvedPullRequest, ResourceStats, WorkerEvent,
};

/// Log line for an intentional branch rename: the session identifier plus the
/// new branch, the branch it replaced, and the agent's immutable original branch
/// (for lineage context). Past tense — this fires AFTER the git rename
/// succeeded (`BranchRenameCompleted { Ok }`). `label` carries the agent's
/// display name (title, or branch when unnamed) for greppable context.
pub(crate) fn branch_rename_log_line(
    session_id: &str,
    label: &str,
    new: &str,
    previous: &str,
    original: &str,
) -> String {
    format!(
        "[{session_id}] agent \"{label}\" renamed branch to {new} from {previous} (original branch name was {original})"
    )
}

/// Log line for an *external* branch change picked up by the branch-sync poller
/// (something ran `git checkout -b` in the worktree). Written at warning tone so
/// the exact drift scenario is greppable in `dux.log`. Includes the session
/// identifier and display label the code it replaced logged.
pub(crate) fn branch_drift_log_line(
    session_id: &str,
    label: &str,
    new: &str,
    previous: &str,
    original: &str,
) -> String {
    format!(
        "[{session_id}] agent \"{label}\" branch changed externally to {new} from {previous} \
         (original was {original}) — if unexpected, check for git activity in the worktree outside dux"
    )
}

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
    /// Delivery audience. Defaults to [`StatusScope::All`] (broadcast, the
    /// pre-scoping behaviour). Stamped from `Engine::current_origin` at the
    /// command mint sites so a web operation's toasts reach only the
    /// originating connection. The TUI ignores it.
    pub scope: StatusScope,
    /// Whether the surface must hold this message until the user dismisses it.
    /// Set it only when the user must act OUTSIDE the toast to recover, or when
    /// something may have been lost or left half-done. See
    /// [`crate::statusline::KeyedWireStatus::sticky`]; the TUI ignores it (a
    /// single status line already waits for the next message).
    pub sticky: bool,
}

impl StatusUpdate {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Info,
            message: message.into(),
            key: None,
            scope: StatusScope::All,
            sticky: false,
        }
    }
    /// SEALED: a `Busy` status may only be born from a [`StatusOp`] (its
    /// `pending_status`/`progress`). This constructor is `pub(crate)` so no
    /// surface crate can hand-roll an indeterminate status without declaring its
    /// outcomes; only the `status_op` module is meant to call it.
    ///
    /// [`StatusOp`]: crate::engine::StatusOp
    pub(crate) fn busy(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Busy,
            message: message.into(),
            key: None,
            scope: StatusScope::All,
            sticky: false,
        }
    }
    #[allow(dead_code)]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Warning,
            message: message.into(),
            key: None,
            scope: StatusScope::All,
            sticky: false,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Error,
            message: message.into(),
            key: None,
            scope: StatusScope::All,
            sticky: false,
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
            scope: StatusScope::All,
            sticky: false,
        }
    }

    /// Mark this status as one that waits for the user (builder form). Reserved
    /// for the small set of outcomes where the user must act outside the toast
    /// to recover, or where something may have been lost or left half-done.
    pub fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    /// Attach a correlation key to this update (builder form). Lets callers
    /// start from one of the tone helpers and then chain `.with_key(k)`.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Set this update's delivery [`StatusScope`] (builder form). Used by the
    /// engine mint sites to stamp `current_origin` onto a freshly-minted status.
    pub fn with_scope(mut self, scope: StatusScope) -> Self {
        self.scope = scope;
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

    /// The worktree manager's listing arrived (TUI only; the web reads the
    /// same core function through its REST route).
    ManageableWorktreesArrived {
        project_id: String,
        result: Result<Vec<crate::worktree_manager::ManagedWorktree>, String>,
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
        branches: RemovedBranches,
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
    /// Rows plus whether this sample had to re-establish its CPU baseline
    /// (see [`WorkerEvent::ResourceStatsReady`]).
    ResourceStatsArrived(Vec<ResourceStats>, bool),

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
    /// A fresh (unborn) repo has just had its empty initial commit created;
    /// register the project on its now-born branch. Mirrors
    /// `AddProjectAfterBranchCheckout` but with no branch switch.
    AddProjectAfterInitialCommit {
        path: String,
        name: String,
        /// The branch the commit landed on (the repo's real current branch).
        branch: String,
        leading_branch: String,
        /// This add ran `git init` first (the adopt-a-folder flow).
        initialized_repo: bool,
        /// The worker seeded a starter `.gitignore`.
        seeded_gitignore: bool,
        /// Non-fatal seed failure to surface as a persistent warning.
        seed_warning: Option<String>,
        /// Correlation id for a web add-project `HandlerStatusOp`. `Some` on the
        /// web path, resolved in `drive_add_project_followup`; `None` for the TUI.
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

    // -- Startup command / log picker (App opens the overlay). --
    /// A scope's runs are loaded; open the picker on the newest one. Only
    /// emitted when the listing is NON-empty: an empty scope is reported by the
    /// load's keyed status and opens nothing.
    StartupLogsArrived {
        scope_label: String,
        listing: StartupCommandLogListing,
    },
    /// A run the picker moved onto finished reading. The App applies it only if
    /// `path` is still the selected run.
    StartupLogContentArrived {
        path: std::path::PathBuf,
        result: Result<String, String>,
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
    /// The BACKGROUND web server's bind pre-flight finished. Same shape and same
    /// pass-through as `ServerFlipPreflightReady`: the listeners are the App's,
    /// and the engine has nothing to mutate. Distinct from it because the flip
    /// ends a TUI session and this starts a serve beside one.
    BackgroundServerPreflightReady {
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
    /// The tab whose launch completed (== `session.id` for the session-slot tab). Lets a
    /// surface route an extra-tab ready to the correct pane without re-deriving.
    pub tab_id: String,
    pub pty_size: (u16, u16),
    pub detached_session_id: Option<String>,
    /// Copied from `AgentLaunchRequest::wants_fullscreen`: the TUI lands this
    /// completion fullscreen when `true` and focused-but-minimized otherwise.
    /// The web never reads it.
    pub wants_fullscreen: bool,
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
        /// The project the failed create belonged to, or `None` when it was a
        /// standalone agent, which belongs to no project. Carried as an option
        /// rather than an empty string so a consumer that keys work by project
        /// cannot silently key it under one that does not exist.
        project_id: Option<String>,
        message: String,
    },
    /// Reconnect-family failure. `session_id` is the pre-existing session that
    /// was being relaunched — used by the wire layer to key the failure status
    /// so it replaces the corresponding "launching…" busy toast.
    Reconnect {
        session_id: String,
        /// The agent's DISPLAY LABEL, not a branch: its title when it has one,
        /// otherwise the branch it tracks, and for a standalone agent its folder's
        /// name. Every consumer prints it as the agent's name, and a standalone
        /// agent has no branch to print, so naming this field for a branch would
        /// be a lie the surfaces render.
        agent_label: String,
        message: String,
    },
    /// Force-reconnect failure. `session_id` carries the pre-existing session
    /// id for the same keying purpose as `Reconnect`.
    ForceReconnect {
        session_id: String,
        /// The agent's DISPLAY LABEL, not a branch: its title when it has one,
        /// otherwise the branch it tracks, and for a standalone agent its folder's
        /// name. Every consumer prints it as the agent's name, and a standalone
        /// agent has no branch to print, so naming this field for a branch would
        /// be a lie the surfaces render.
        agent_label: String,
        message: String,
    },
    /// Engine logged + marked Detached; App has nothing to do.
    ResumeFallback,
    /// Startup-auto-reopen failure. `session_id` carries the pre-existing
    /// session id for the same keying purpose as `Reconnect`.
    StartupAutoReopen {
        session_id: String,
        /// The agent's DISPLAY LABEL, not a branch: its title when it has one,
        /// otherwise the branch it tracks, and for a standalone agent its folder's
        /// name. Every consumer prints it as the agent's name, and a standalone
        /// agent has no branch to print, so naming this field for a branch would
        /// be a lie the surfaces render.
        agent_label: String,
        message: String,
    },
    /// An extra-tab launch failed. `tab_id` keys the failure toast to the
    /// specific tab. For an `is_fresh` create the Engine has already deleted the
    /// tab's row (the create never came up); for a dormant relaunch the row is
    /// kept so the user can retry.
    Tab {
        session_id: String,
        tab_id: String,
        /// The agent's DISPLAY LABEL, not a branch: its title when it has one,
        /// otherwise the branch it tracks, and for a standalone agent its folder's
        /// name. Every consumer prints it as the agent's name, and a standalone
        /// agent has no branch to print, so naming this field for a branch would
        /// be a lie the surfaces render.
        agent_label: String,
        message: String,
    },
    /// A launch failure for an extra tab whose row was deleted while the
    /// launch was in flight (mirrors `AgentLaunchReadyView::SessionMissing` on
    /// the success path). Silent by design: the tab is already closed from the
    /// user's perspective, so there is nothing to warn about and no row left
    /// to delete again.
    Silent,
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

/// What happened to the agent's BRANCHES when its worktree was removed.
///
/// Two answers, because a delete now has two legal shapes. dux deletes the
/// branches it created; it removes the worktree and keeps the branches when
/// they are not its own (an agent attached to an existing branch, or adopted
/// along with an existing worktree). The keep path never calls the branch
/// deleting code at all, so it has no [`crate::git::RemoveResult`] to report
/// and must not invent one: a `RemoveResult` full of `Deleted` would be a
/// straightforward lie, and one full of `AlreadyGone` would be the opposite lie.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemovedBranches {
    /// dux deleted the branches it owned; git's per-branch report.
    Deleted(crate::git::RemoveResult),
    /// Nothing was deleted. The branches predate dux's ownership, so the
    /// worktree went and both branches stayed. Carries the provenance so the
    /// message can say WHY they were kept.
    Kept(crate::model::BranchProvenance),
}

/// The refusal for a delete that asked dux to remove a standalone agent's
/// folder. Shared by the synchronous and graceful delete paths so the two can
/// never word the same refusal differently.
///
/// It names the folder, because the whole point is to reassure the user that
/// the directory they pointed dux at is still theirs, and it says what to do
/// instead rather than only saying no.
pub fn standalone_delete_directory_refusal(agent_name: &str, folder: &str) -> String {
    format!(
        "Agent \"{agent_name}\" is a standalone agent: it runs in \"{}\", a folder you already \
         had, and dux never removes it. Delete the agent on its own to remove dux's \
         record of it, and remove the folder yourself if you no longer want it.",
        crate::home_path::shorten_home(std::path::Path::new(folder))
    )
}

/// What happened to the session's worktree during deletion. Each variant maps
/// 1:1 to a user-facing status message; the illegal "delete requested, no
/// siblings, but no result" state has no representation. Replaces the former
/// `(delete_worktree: bool, remove_outcome: Option<bool>)` pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeRemoval {
    /// Deletion NOT requested; worktree shared with sibling sessions.
    PreservedShared,
    /// Deletion NOT requested; no siblings — worktree left at its path.
    PreservedOrphan,
    /// Deletion requested but skipped because siblings still use the worktree.
    SkippedForSiblings,
    /// Worktree removed. `branches` says what became of the branches: git's
    /// report for BOTH branches the removal targeted (the one the worktree was
    /// on and, when the agent drifted, the one it was born on), or that they
    /// were deliberately kept because they were not dux's to delete.
    Performed { branches: RemovedBranches },
    /// There was no worktree, because this was a STANDALONE agent: it ran in a
    /// folder the user already had, and deleting the agent removed dux's record
    /// of it and nothing else. Its own variant rather than one of the
    /// "preserved" ones above, because those all describe a worktree dux
    /// decided not to remove, and there was never one here to decide about.
    /// The folder is carried so the message can name it.
    NothingToRemove { folder_label: String },
}

impl WorktreeRemoval {
    /// Derive the removal outcome for a synchronous (inline / `do_delete`)
    /// decision, given user intent and whether siblings share the worktree.
    /// `performed` is `Some(result)` when git actually removed the worktree,
    /// `None` when it was not run. The caller guarantees `performed.is_some()`
    /// exactly when `delete_worktree && !other_sessions`.
    fn from_decision(
        session: &AgentSession,
        delete_worktree: bool,
        other_sessions_on_worktree: bool,
        performed: Option<RemovedBranches>,
    ) -> Self {
        // A standalone agent never had a worktree, so none of the worktree
        // outcomes below can describe what happened. Answered off the workspace
        // rather than inferred from `performed.is_none()`, which is also true
        // of every managed delete that merely declined to remove one.
        if let crate::model::AgentWorkspace::Folder(folder) = &session.workspace {
            return WorktreeRemoval::NothingToRemove {
                folder_label: crate::home_path::shorten_home(std::path::Path::new(
                    &folder.folder_path,
                )),
            };
        }
        match (delete_worktree, other_sessions_on_worktree, performed) {
            (_, _, Some(branches)) => WorktreeRemoval::Performed { branches },
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
    /// A tab of this session still has a launch in flight (marked in-flight but
    /// not yet registered in `providers`). Such a tab is invisible to the
    /// live-tab check, so deleting now could remove the worktree out from under
    /// the still-spawning process. The caller shows a "try again" error and does
    /// NOT delete.
    TabLaunching,
    /// Worktree-removing delete: the Engine has already SIGTERMed the agent PTY
    /// (and the session's terminals) and moved them to the terminating set,
    /// capturing the worktree removal to run only after the agent exits. The
    /// caller **vanishes the session now** (`finish_delete_session` with
    /// `update_status=false`) and mints its OWN keyed `HandlerStatusOp` from this
    /// `busy_message` (the TUI in `App::pending_delete_ops`, the web in
    /// `Engine::pending_delete_ops_web`), showing its pending busy. Once the agent
    /// PTY is reaped, `reap_terminating_ptys` hands the removal to
    /// `dispatch_deferred_worktree_removal`, whose worker posts
    /// `WorktreeRemoveCompleted` — resolving that op with each surface's wording.
    AsyncStarted { busy_message: String },
    /// Inline path: no worktree removal needed (no `delete_worktree` request
    /// or shared with siblings). App should call the existing
    /// `finish_delete_session` wrapper to complete cleanup + emit status.
    Inline { removal: WorktreeRemoval },
    /// The caller asked to remove a directory dux may not remove: a standalone
    /// agent's folder. Nothing was deleted, not even the agent record, and the
    /// message says why. Distinct from every other arm because it is the one
    /// where the DELETE ITSELF did not happen.
    Refused { message: String },
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
    /// The tab whose launch was dispatched (== `session_id` for the session-slot tab).
    pub tab_id: String,
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
    session.display_label()
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
                    // Canonical comparison, like every other place dux asks
                    // whether two agents occupy one directory (the
                    // occupied-directory refusal at create, the worktree
                    // manager). A raw string compare misses a symlinked
                    // spelling of the same worktree, and then both agents run a
                    // provider in it and resume each other's conversation.
                    && crate::project_browser::same_directory(s.directory(), worktree_path)
                    // Tab-aware: a conflicting session may have its session-slot tab dead
                    // while an extra tab is still live in the shared worktree.
                    // `providers` is tab-keyed, so check every tab, not just `s.id`.
                    && self
                        .tab_ids_for_session(&s.id)
                        .iter()
                        .any(|id| self.providers.contains_key(id))
            })
            .cloned()?;

        let label = session_label(&conflicting);
        let provider = conflicting.provider.as_str().to_string();
        // Tear down EVERY tab of the conflicting agent (Main + Support) — a
        // extra tab left running would keep holding the contested worktree.
        // Keep its `agent_tabs` rows: the session still exists, just detached.
        // This also drops the tabs' `pty_activity`/`pty_input` entries, so the
        // callers no longer need their own follow-up clear.
        self.clear_session_tab_runtime(&conflicting.id);
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
    /// Combine a launch View reaction with the create op's resolved final (when
    /// the launch was a create-kind, whose shared op is resolved engine-side). The
    /// final rides ALONGSIDE the View as a `Multi`, so whichever surface is running
    /// applies the View's non-status work AND the same keyed final. Non-create
    /// launches carry no final here (their reconnect op is resolved per-surface).
    fn launch_view_with_final(
        view: EventReaction,
        create_final: Option<ResolvedFinal>,
    ) -> EventReaction {
        match create_final {
            Some(resolved) => EventReaction::Multi(vec![view, resolved.into_reaction()]),
            None => view,
        }
    }

    /// Resolve the shared create op (if one is stashed) against `outcome`, popping
    /// it from the registry. Returns the keyed final for `launch_view_with_final`.
    fn resolve_create_op(
        &mut self,
        status_op_id: &str,
        outcome: CreateLaunchOutcome,
    ) -> Option<ResolvedFinal> {
        self.pending_create_ops
            .remove(status_op_id)
            .map(|op| op.resolve(&outcome))
    }

    pub fn process_agent_launch_ready(
        &mut self,
        data: AgentLaunchReadyData,
    ) -> (AgentLaunchReadyOutcome, Option<ResolvedFinal>) {
        let AgentLaunchReadyData { request, client } = data;
        let session = request.session.clone();
        let pty_size = request.pty_size;
        // Runtime PTY/provider state is keyed by tab id (== session.id for the
        // session-slot tab). Use it for the in-flight clear, the providers insert, and
        // the resume-fallback candidate so an extra tab tracks under its own key.
        let tab_id = request.tab_id.clone();
        let wants_fullscreen = request.wants_fullscreen;
        self.clear_in_flight(&InFlightKey::AgentLaunch(tab_id.clone()));

        if let AgentLaunchKind::Create { status_op_id, .. } = &request.kind {
            let status_op_id = status_op_id.clone();
            self.clear_in_flight(&InFlightKey::CreateAgent);
            if let Err(err) = self.session_store.upsert_session(&session) {
                logger::error(&format!(
                    "session store upsert failed for {}: {err}",
                    session.id,
                ));
                let create_final = self.resolve_create_op(
                    &status_op_id,
                    CreateLaunchOutcome::PersistFailed {
                        error: err.to_string(),
                    },
                );
                return (
                    AgentLaunchReadyOutcome {
                        session,
                        tab_id: tab_id.clone(),
                        pty_size,
                        detached_session_id: None,
                        wants_fullscreen,
                        view: AgentLaunchReadyView::CreatePersistFailed {
                            error: err.to_string(),
                        },
                    },
                    create_final,
                );
            }
            let detached =
                self.detach_conflicting_worktree_session(session.directory(), &session.id);
            self.providers.insert(tab_id.clone(), client);
            self.record_launched_drop_paste(&tab_id, &request.provider, &request.provider_config);
            self.sessions.insert(0, session.clone());
            // Correlate this create op with the session it just produced so a REST
            // create handler holding the op id (from `WireCommandOutcome.created_op_id`)
            // resolves its exact session without a racy set-difference scan.
            self.record_created_session(status_op_id.clone(), session.id.clone());
            self.mark_session_provider_started(&session.id, &session.provider);
            // A brand-new STANDALONE agent's folder is classified now, so its
            // changes panel, its mutation gate and its upload seed all know the
            // truth from the first frame instead of starting at "not looked
            // yet" (which fails closed). A no-op for every other kind.
            self.spawn_folder_repo_probe(&session.id);
            if request.resume {
                self.resume_fallback_candidates
                    .insert(tab_id.clone(), Instant::now());
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

            // Resolve the shared create op engine-side so both surfaces replace the
            // create busy with the SAME final (success line or startup-failure).
            let create_outcome = match &startup_result_error {
                Some(error) => CreateLaunchOutcome::StartupFailed {
                    branch_name: session.display_label(),
                    error: error.clone(),
                },
                None => CreateLaunchOutcome::Committed {
                    status_message: status_message.clone(),
                },
            };
            let create_final = self.resolve_create_op(&status_op_id, create_outcome);

            return (
                AgentLaunchReadyOutcome {
                    session,
                    tab_id: tab_id.clone(),
                    pty_size,
                    detached_session_id: detached.map(|d| d.id),
                    wants_fullscreen,
                    view: AgentLaunchReadyView::CreateCommitted {
                        status_message,
                        startup_result_error,
                    },
                },
                create_final,
            );
        }

        // Non-Create branches share the "drop on missing session" guard.
        if !self.sessions.iter().any(|s| s.id == session.id) {
            logger::info(&format!(
                "dropping launched PTY for missing session {}",
                session.id,
            ));
            return (
                AgentLaunchReadyOutcome {
                    session,
                    tab_id: tab_id.clone(),
                    pty_size,
                    detached_session_id: None,
                    wants_fullscreen,
                    view: AgentLaunchReadyView::SessionMissing,
                },
                None,
            );
        }

        // Ghost-launch guard: an extra tab whose row was deleted while its
        // launch was in flight must not resurrect a live PTY under a dead tab
        // id. Dropping `client` here (PtyClient::Drop) terminates the freshly
        // spawned process. The session-slot tab (`tab_id == session.id`, which never has
        // an `agent_tabs` row) is exempt — "no row" is normal for Main.
        let is_main = tab_id == session.id;
        if !is_main && !self.agent_tabs.contains_key(&tab_id) {
            logger::info(&format!(
                "dropping launched PTY for closed extra tab {tab_id} of session {}",
                session.id,
            ));
            return (
                AgentLaunchReadyOutcome {
                    session,
                    tab_id: tab_id.clone(),
                    pty_size,
                    detached_session_id: None,
                    wants_fullscreen,
                    view: AgentLaunchReadyView::SessionMissing,
                },
                None,
            );
        }

        let detached = self.detach_conflicting_worktree_session(session.directory(), &session.id);
        self.providers.insert(tab_id.clone(), client);
        self.record_launched_drop_paste(&tab_id, &request.provider, &request.provider_config);
        if request.resume {
            self.resume_fallback_candidates
                .insert(tab_id.clone(), Instant::now());
        }
        // Session-level running state stays Main-scoped: an extra-tab launch
        // must not flip the whole agent to Active or persist desired_running
        // (that would light the sidebar and auto-reopen the Main provider).
        if is_main {
            self.mark_session_desired_running(&session.id, true);
            self.mark_session_status(&session.id, SessionStatus::Active);
        }
        // Record the provider that actually launched (the effective per-tab
        // provider), so directory-scoped resume state stays correct even when a
        // extra tab ran a different provider than the session default.
        self.mark_session_provider_started(&session.id, &request.provider);

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
            // An extra-tab ready behaves like a reconnect for the view (show the
            // surface + info); it is never resumed and never Main-scoped.
            AgentLaunchKind::Tab { status_message, .. } => {
                AgentLaunchReadyView::Reconnect { status_message }
            }
            AgentLaunchKind::Create { .. } => unreachable!("create launch handled above"),
        };

        (
            AgentLaunchReadyOutcome {
                session,
                tab_id,
                pty_size,
                detached_session_id: detached.map(|d| d.id),
                wants_fullscreen,
                view,
            },
            None,
        )
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
                // A removed project takes its project terminals with it (graceful
                // SIGTERM via the terminating set); otherwise they would be
                // orphaned with no sidebar row and no owner to route through.
                self.begin_close_project_terminals(project_id);
                self.projects.retain(|p| p.id != *project_id);
                ProjectPersistenceView::Removed {
                    project_name: project_name.clone(),
                }
            }
            ProjectPersistenceAction::Delete {
                project_id,
                project_name,
            } => {
                self.begin_close_project_terminals(project_id);
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
    /// Clear every runtime map entry (`providers`, `running_provider_pins`,
    /// `resume_fallback_candidates`, `pty_activity`, `pty_input`, and the
    /// in-flight `AgentLaunch` key) for ALL of a session's tabs — the
    /// session-slot tab (`tab_id == session_id`) and every extra tab. Does NOT
    /// remove the persisted `agent_tabs` records: a detach keeps them (the
    /// session lives on, just disconnected); a delete removes them separately
    /// via `agent_tabs.retain`.
    ///
    /// This is the tab-aware replacement for the single-`session.id` map clears
    /// every WHOLE-AGENT teardown path used before tabs existed. Session-slot
    /// scoped operations (`kill_session_pty`, force-reconnect) deliberately do
    /// NOT use it — they act on the session-slot tab only via `clear_tab_runtime`
    /// directly and must leave the user's independent extra tabs running.
    fn clear_session_tab_runtime(&mut self, session_id: &str) {
        for tab_id in self.tab_ids_for_session(session_id) {
            self.clear_tab_runtime(&tab_id);
        }
    }

    /// Remember what a tab LAUNCHED with, so the spine can answer for that TAB
    /// rather than for its provider's name: two live tabs of one provider
    /// launched either side of a config edit need the two forms they each
    /// started with, and one name cannot carry both. It is also what still
    /// answers after the user renames or removes the tab's `[providers.<name>]`
    /// block. Both halves come from the exact [`ProviderCommandConfig`] the
    /// launch used rather than being re-read from the current config, because
    /// the whole point is to survive a later edit.
    /// Retired by [`Engine::clear_tab_runtime`] when the process goes.
    fn record_launched_drop_paste(
        &mut self,
        tab_id: &str,
        provider: &ProviderKind,
        provider_config: &crate::config::ProviderCommandConfig,
    ) {
        self.launched_drop_paste.insert(
            tab_id.to_string(),
            crate::engine::LaunchedDropPaste {
                provider: provider.as_str().to_string(),
                form: provider_config.resolved_web_dragdrop_paste(),
                command_name: provider_config.command_file_name(),
            },
        );
    }

    /// Clear every runtime map keyed by ONE tab id: the body below is the list,
    /// and it is the SINGLE source of truth for it. Three callers rely on that:
    /// `close_tab` (a single extra tab), `clear_session_tab_runtime` (a whole
    /// session, looped) and `retry_resume_fallback` (a stale resume attempt
    /// about to be relaunched). Adding a new tab-keyed map is therefore a
    /// one-line change here rather than a comment-enforced convention spread
    /// across three files.
    ///
    /// `retry_resume_fallback` used to name three of these maps itself, and
    /// every other one leaked whenever the relaunch it dispatched then failed.
    /// Do not go back to naming maps at a call site.
    pub fn clear_tab_runtime(&mut self, tab_id: &str) {
        self.providers.remove(tab_id);
        self.running_provider_pins.remove(tab_id);
        self.launched_drop_paste.remove(tab_id);
        self.resume_fallback_candidates.remove(tab_id);
        self.pty_activity.remove(tab_id);
        self.pty_input.remove(tab_id);
        self.pty_pointer.remove(tab_id);
        // Attention/progress runtime state is torn down with the tab so a
        // detach/relaunch/delete can never leave a stale flag or a stuck
        // "working" progress override behind.
        self.needs_attention.remove(tab_id);
        self.pty_progress.remove(tab_id);
        self.agent_viewed.remove(tab_id);
        self.clear_in_flight(&InFlightKey::AgentLaunch(tab_id.to_string()));
    }

    /// Drop the activity/input runtime entries for a companion terminal being
    /// torn down. Terminals share `pty_activity`/`pty_input`/`pty_pointer` with agent tabs
    /// (keyed by the disjoint `term-N` id), so a removed terminal must clear both
    /// or a later recycled `term-N` id could inherit stale activity and read as
    /// working/typing before it has emitted a byte. The terminal analogue of
    /// [`Engine::clear_tab_runtime`]; call it wherever a terminal leaves
    /// `companion_terminals`.
    pub fn clear_terminal_runtime(&mut self, terminal_id: &str) {
        self.pty_activity.remove(terminal_id);
        self.pty_input.remove(terminal_id);
        self.pty_pointer.remove(terminal_id);
    }

    pub fn finish_delete_session(
        &mut self,
        session_id: &str,
    ) -> anyhow::Result<Option<FinishDeleteSessionOutcome>> {
        if !self.sessions.iter().any(|s| s.id == session_id) {
            return Ok(None);
        }
        // Persist the deletion FIRST so a DB failure leaves in-memory state
        // untouched and the session remains visible in the UI. If we cleared
        // in-memory state first and the DB call then failed, the session
        // would vanish from the UI but reappear on restart.
        self.session_store.delete_session(session_id)?;
        Ok(self.finish_delete_session_memory(session_id))
    }

    /// The IN-MEMORY half of a session deletion (no DB write): tear down the
    /// runtime maps for every tab, drop the session/companion-terminals/extra-tab
    /// records, and refresh derived state. Infallible. `finish_delete_session`
    /// calls this after persisting; `Command::RemoveProject` calls it directly,
    /// because `remove_project_records` already deleted the rows transactionally —
    /// re-running `delete_session` there (and letting a transient DB error abort
    /// the in-memory cleanup) would strand ghost sessions/tabs against an empty DB.
    pub(crate) fn finish_delete_session_memory(
        &mut self,
        session_id: &str,
    ) -> Option<FinishDeleteSessionOutcome> {
        let session = self.sessions.iter().find(|s| s.id == session_id).cloned()?;
        // The session is being removed now, so drop any "closing" marker set at the
        // start of its delete (both the async and synchronous delete paths clear it
        // here, in addition to the async worktree-removal-completed path).
        self.closing_sessions.remove(session_id);
        let project = session.project_id().and_then(|project_id| {
            self.projects
                .iter()
                .find(|project| project.id == project_id)
                .cloned()
        });
        let other_sessions_on_worktree = self.sessions.iter().any(|s| {
            s.id != session.id
                && crate::project_browser::same_directory(s.directory(), session.directory())
        });

        // Startup-command logs are keyed by project id, and only a managed
        // agent can ever have run a startup command (it is a worktree
        // provisioning step). A standalone agent has no project and no logs, so
        // there is nothing to delete; passing an empty project id would point
        // the delete at the shared log root instead of one agent's directory.
        if let Some(project_id) = session.project_id() {
            crate::startup::spawn_delete_startup_command_logs(
                self.paths.clone(),
                project_id.to_string(),
                session.id.clone(),
            );
        }

        // Tear down the runtime maps for EVERY tab of this agent (Main + Support),
        // then drop the session, its companion terminals, and its extra-tab
        // records. The graceful `begin_delete_session` already moved live tab
        // PTYs into the terminating set, so `providers.remove` here is a no-op for
        // those; this cleans up the remaining pin/activity/input/in-flight entries.
        self.clear_session_tab_runtime(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        let removed_terminals: Vec<String> = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.owner.closed_by_session_delete(&session.id))
            .map(|(id, _)| id.clone())
            .collect();
        for terminal_id in &removed_terminals {
            self.companion_terminals.remove(terminal_id);
            self.clear_terminal_runtime(terminal_id);
        }
        self.agent_tabs.retain(|_, t| t.session_id != session.id);
        // Drop the PR runtime state with the session: an in-flight PR check's
        // late result is separately guarded at `PrStatusReady`, and the store
        // rows go with the session row, so anything left here would be pure
        // in-memory residue for an agent that no longer exists.
        self.pr_statuses.remove(&session.id);
        self.pr_last_checked.remove(&session.id);
        // The in-memory pin goes too (its store row is deleted with the
        // session); leaving it would ghost-gate the identity guard and the
        // detach palette entry for a later session reusing the id.
        self.pr_overrides.remove(&session.id);
        // The folder-repository verdict is runtime state keyed by session id,
        // so it goes with the session too. A verdict arriving after this point
        // is dropped by the `FolderRepoStatusReady` handler's own
        // still-exists check; this is the other half, for the verdict already
        // stored.
        self.folder_repo_statuses.remove(&session.id);
        // The detach state goes with the session too, so a later session that
        // reuses the id does not inherit a detach it never asked for.
        self.pr_suppressions.remove(&session.id);
        self.update_branch_sync_sessions();

        // A standalone agent belongs to no project, so "does its project still
        // have agents" is not a question about it at all. Answering `false`
        // (which comparing two absent ids would do) would tell the caller a
        // project just emptied out when no project was involved.
        let project_still_has_sessions = session.project_id().is_some_and(|project_id| {
            self.sessions
                .iter()
                .any(|candidate| candidate.project_id() == Some(project_id))
        });

        Some(FinishDeleteSessionOutcome {
            session,
            project,
            other_sessions_on_worktree,
            project_still_has_sessions,
        })
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
        // THE EXPLICIT WIRE CONTRACT for `delete_worktree=true` on a standalone
        // id: refuse, out loud. Quietly ignoring it would be success theater
        // about a destructive request, and the user would come away believing
        // dux had cleaned something up. The default is already false, so only
        // a caller that asked on purpose can reach this.
        if delete_worktree && !session.workspace.deletion_may_remove_directory() {
            anyhow::bail!(standalone_delete_directory_refusal(
                &session.display_label(),
                session.directory()
            ));
        }
        logger::info(&format!(
            "deleting session {} at {} (delete_worktree={}, sync)",
            session.id,
            session.directory(),
            delete_worktree
        ));
        // The project may be ABSENT for an orphaned session; we can still delete
        // the record but cannot remove its worktree without the project repo, so
        // an orphan always keeps its worktree.
        let project = session.project_id().and_then(|project_id| {
            self.projects
                .iter()
                .find(|project| project.id == project_id)
                .cloned()
        });
        let other_sessions_on_worktree = self.sessions.iter().any(|s| {
            s.id != session.id
                && crate::project_browser::same_directory(s.directory(), session.directory())
        });

        // What a removal actually needs: a project to run git in AND a managed
        // working copy to remove. Resolved as one pair up front, so the removal
        // block below can be entered only when both exist and there is no arm
        // in it that could delete a directory dux did not create.
        //
        // A standalone agent's directory is the user's folder, so its deletion
        // removes dux's record of the agent and nothing else, ever: it has no
        // managed workspace, so `removal_target` is `None` for it and the block
        // is unreachable rather than guarded.
        let removal_target = match (project.as_ref(), session.workspace.as_managed()) {
            (Some(project), Some(managed)) if delete_worktree && !other_sessions_on_worktree => {
                Some((project, managed))
            }
            _ => None,
        };
        let should_remove_worktree = removal_target.is_some();

        if self.pending_deletions.contains(session_id) {
            crate::logger::error(&format!(
                "do_delete_session called while an async delete worker is in-flight for {session_id} \u{2014} refusing to proceed to avoid racing git::remove_worktree",
            ));
            return Ok(None);
        }
        // Refuse if any tab of this session has a launch in flight: such a tab is
        // marked in-flight but not yet in `providers`, so the pre-kill below cannot
        // reach it and `git::remove_worktree` could race the provider mid-spawn in
        // the worktree (git-lock / cwd-deleted-under-fork). Mirrors the blanket
        // precondition in `begin_delete_session`.
        if should_remove_worktree
            && self
                .tab_ids_for_session(session_id)
                .iter()
                .any(|id| self.is_in_flight(&InFlightKey::AgentLaunch(id.clone())))
        {
            crate::logger::error(&format!(
                "do_delete_session for {session_id}: a tab is still launching \u{2014} refusing to remove its worktree to avoid racing the spawning provider",
            ));
            return Ok(None);
        }
        // Mark the session "closing" so a concurrent `create_tab`/`launch_agent`
        // can't spawn a fresh provider into the worktree we are about to remove.
        // `finish_delete_session_memory` (called below) clears it. This path is
        // synchronous so the window is tiny, but the flag keeps the invariant with
        // `begin_delete_session` uniform.
        if should_remove_worktree {
            self.closing_sessions.insert(session.id.clone());
        }
        let remove_outcome = if let Some((project, managed)) = removal_target {
            // Hard-kill every live tab PTY (Main + Support) and companion terminal
            // of this session BEFORE removing the worktree: dropping a `PtyClient`
            // SIGKILLs its whole process group, so no provider process is alive in
            // the directory when `git::remove_worktree` runs. `finish_delete_session`
            // below then clears the remaining runtime map entries. (This is the
            // synchronous counterpart to `begin_delete_session`'s deferred group
            // barrier — the project-delete loop that calls us is synchronous.)
            for tab_id in self.tab_ids_for_session(session_id) {
                self.providers.remove(&tab_id);
            }
            let removed_terminals: Vec<String> = self
                .companion_terminals
                .iter()
                .filter(|(_, t)| t.owner.closed_by_session_delete(session_id))
                .map(|(id, _)| id.clone())
                .collect();
            for terminal_id in &removed_terminals {
                self.companion_terminals.remove(terminal_id);
                self.clear_terminal_runtime(terminal_id);
            }
            // Clear `closing_sessions` even if removal fails: the session record
            // survives an `Err` (the `?` aborts the delete), and unlike the async
            // `WorktreeRemoveCompleted` handler nothing else would clear the flag,
            // leaving the agent permanently barred from creating/relaunching tabs.
            // THE GATE. dux deletes the branches it created and only those.
            // An agent attached to `develop`, or adopted with an existing
            // worktree, gives up its worktree and keeps its branches: they were
            // the user's before the agent existed. Deciding it HERE means the
            // project-delete cascade (which calls this per agent) inherits it,
            // so removing a project can no longer take `develop` with it.
            let result = if managed.branch_provenance.dux_may_delete_branch() {
                match crate::git::remove_worktree(
                    std::path::Path::new(&project.path),
                    std::path::Path::new(&managed.worktree_path),
                    &managed.branch_name,
                    // The BIRTH branch too: `branch_name` tracks whatever the
                    // worktree drifted onto, so deleting only that leaves the
                    // original behind and recreating the agent collides with it.
                    Some(managed.initial_branch.as_str()),
                ) {
                    Ok(result) => RemovedBranches::Deleted(result),
                    Err(err) => {
                        self.closing_sessions.remove(session_id);
                        return Err(err);
                    }
                }
            } else {
                match crate::git::remove_worktree_keep_branch(
                    std::path::Path::new(&project.path),
                    std::path::Path::new(&managed.worktree_path),
                ) {
                    Ok(()) => RemovedBranches::Kept(managed.branch_provenance),
                    Err(err) => {
                        self.closing_sessions.remove(session_id);
                        return Err(err);
                    }
                }
            };
            Some(result)
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
                &finish.session,
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
        // Same explicit contract as the synchronous path: a worktree-removing
        // delete of a standalone agent is refused rather than silently
        // downgraded to an ordinary one.
        if delete_worktree && !session.workspace.deletion_may_remove_directory() {
            return BeginDeleteSessionOutcome::Refused {
                message: standalone_delete_directory_refusal(
                    &session.display_label(),
                    session.directory(),
                ),
            };
        }
        // Blanket precondition: refuse while ANY tab of this session has a launch
        // in flight. Such a tab is marked in-flight but not yet in `providers`, so
        // it is invisible to the `live_tabs` check below — a worktree-removing
        // delete could otherwise dispatch `git worktree remove` while the provider
        // is mid-spawn in that worktree (git-lock / cwd-deleted-under-fork). Must
        // run before ANY removal branch is selected.
        if self
            .tab_ids_for_session(&session.id)
            .iter()
            .any(|id| self.is_in_flight(&InFlightKey::AgentLaunch(id.clone())))
        {
            return BeginDeleteSessionOutcome::TabLaunching;
        }
        // The project may be ABSENT for an orphaned session (its project was
        // removed but the session record outlived it). We can still delete the
        // session record; we just cannot run `git worktree remove` without the
        // project repo, so an orphan keeps its worktree and takes the inline path.
        let project = session.project_id().and_then(|project_id| {
            self.projects
                .iter()
                .find(|project| project.id == project_id)
                .cloned()
        });
        let other_sessions_on_worktree = self.sessions.iter().any(|s| {
            s.id != session.id
                && crate::project_browser::same_directory(s.directory(), session.directory())
        });
        // A standalone agent's directory is the user's folder, so deletion
        // removes dux's record of the agent and nothing else, ever. The named
        // question answers that; the removal payload below cannot even be built
        // without a managed workspace, so this is a guard and a restatement.
        let should_remove_worktree = delete_worktree
            && session.workspace.deletion_may_remove_directory()
            && !other_sessions_on_worktree
            && project.is_some();

        // Mark the session "closing" synchronously, for the whole grace window
        // (until `WorktreeRemoveCompleted`). While set, `create_tab`/`launch_agent`
        // refuse to spawn a fresh provider into the worktree that is about to be
        // removed. `pending_deletions` alone is insufficient: it isn't set until
        // the removal worker is actually dispatched, which for a live agent only
        // happens after its PTYs reap — leaving a create-race window open.
        if should_remove_worktree {
            self.closing_sessions.insert(session.id.clone());
        }

        // Graceful, non-blocking close: SIGTERM the agent PTY and its companion
        // terminals and move them to the terminating set for a background reap,
        // instead of dropping them here (an immediate hard SIGKILL). The caller
        // vanishes the session from the UI next (`finish_delete_session`), so this
        // is snappy. For a worktree-removing delete, the removal is captured on
        // the agent's terminating entry and dispatched by `reap_terminating_ptys`
        // only AFTER the agent has actually exited — files are never deleted out
        // from under a live process (which also avoids git-lock failures).
        // The payload carries the MANAGED workspace itself rather than four
        // loose git fields, so a standalone agent cannot produce one: there is
        // no value of the right type to put in it. That is the structural half
        // of "nothing deletes the folder"; `should_remove_worktree` above is
        // the readable half, and the two cannot drift apart.
        let worktree_removal = match (
            should_remove_worktree,
            session.workspace.as_managed(),
            project.as_ref(),
        ) {
            (true, Some(managed), Some(project)) => Some(super::DeferredWorktreeRemoval {
                session_id: session.id.clone(),
                project_path: project.path.clone(),
                managed: managed.clone(),
                busy_message: format!(
                    "Removing worktree for agent \"{}\"\u{2026}",
                    session.display_label()
                ),
            }),
            _ => None,
        };
        let busy_message = worktree_removal.as_ref().map(|r| r.busy_message.clone());

        // Gracefully close EVERY live tab PTY of this agent (the session-slot tab and any
        // extra tabs), not just the Main provider — an extra tab left running
        // would be orphaned and, for a worktree-removing delete, keep writing into
        // a worktree about to be removed.
        let live_tabs: Vec<String> = self
            .tab_ids_for_session(&session.id)
            .into_iter()
            .filter(|id| self.providers.contains_key(id))
            .collect();
        // A tab closed moments earlier (e.g. via `close_tab`) is already out of
        // `providers` and parked in `terminating_ptys` under its own SIGTERM grace
        // period — it is still alive (a child process) but invisible to the
        // `live_tabs` check above. Without counting it, `git::remove_worktree`
        // could fire while that straggler is still using the worktree as its cwd
        // (a git-lock/corruption race). Fold it into the barrier the same way a
        // still-live tab is, without re-issuing `begin_close_provider` for it (it
        // already has its own terminating entry in flight).
        let already_terminating: Vec<String> = self
            .terminating_ptys
            .iter()
            .filter(|t| t.kind == crate::engine::PrunedPtyKind::Agent)
            .filter(|t| self.owning_session_for_tab(&t.id).as_deref() == Some(session.id.as_str()))
            .map(|t| t.id.clone())
            .collect();
        match worktree_removal {
            // No live or already-terminating tab PTY to wait for (the agent
            // already fully exited): remove the worktree now — there is nothing
            // to reap, and the reaper would never see it.
            Some(removal) if live_tabs.is_empty() && already_terminating.is_empty() => {
                let _ = self.dispatch_deferred_worktree_removal(removal);
            }
            // Exactly one live tab and no already-terminating stragglers: carry
            // the removal on its terminating entry; `reap_terminating_ptys`
            // dispatches it when that PTY reaps.
            Some(removal) if live_tabs.len() == 1 && already_terminating.is_empty() => {
                let unhandled = self.begin_close_provider(
                    &live_tabs[0],
                    session.display_label(),
                    Some(removal),
                );
                if let Some(req) = unhandled {
                    let _ = self.dispatch_deferred_worktree_removal(req);
                }
            }
            // Multiple tabs to wait for (live and/or already-terminating): close
            // each still-live tab with no per-entry removal and park a GROUP
            // barrier listing every one of them — including the already-terminating
            // stragglers — so the removal fires exactly once, only after the LAST
            // tab PTY has reaped (clean exit or force-kill), never out from under a
            // still-running sibling or straggler tab.
            Some(removal) => {
                for id in &live_tabs {
                    let _ = self.begin_close_provider(id, session.display_label(), None);
                }
                let pending_ids: std::collections::HashSet<String> = live_tabs
                    .iter()
                    .cloned()
                    .chain(already_terminating.iter().cloned())
                    .collect();
                self.pending_group_removals
                    .push(super::GroupWorktreeRemoval {
                        pending_ids,
                        removal,
                    });
            }
            // Keep-worktree delete: gracefully close every live tab, nothing deferred.
            None => {
                for id in &live_tabs {
                    let _ = self.begin_close_provider(id, session.display_label(), None);
                }
            }
        }
        self.begin_close_session_terminals(&session.id);

        match busy_message {
            Some(busy_message) => {
                logger::info(&format!(
                    "deleting session {} at {} (delete_worktree=true; worktree removal deferred until the agent exits)",
                    session.id,
                    session.directory()
                ));
                // The caller vanishes the session now and mints a keyed op from
                // `busy_message`; the reaper spawns the worktree worker once the
                // agent PTY is reaped, and its `WorktreeRemoveCompleted` resolves
                // that op.
                BeginDeleteSessionOutcome::AsyncStarted { busy_message }
            }
            None => {
                logger::info(&format!(
                    "deleting session {} at {} (delete_worktree={}, no worktree removal)",
                    session.id,
                    session.directory(),
                    delete_worktree
                ));
                BeginDeleteSessionOutcome::Inline {
                    removal: WorktreeRemoval::from_decision(
                        &session,
                        delete_worktree,
                        other_sessions_on_worktree,
                        None,
                    ),
                }
            }
        }
    }

    /// Spawn the background worker that removes a deleted agent's worktree, now
    /// that its PTY has been reaped by `reap_terminating_ptys`. Deferred from the
    /// delete itself (see `begin_delete_session`) so files are never removed out
    /// from under a still-running process. Mirrors the async branch of
    /// `begin_delete_session`: marks the in-flight guard, stashes the Busy message
    /// for status correlation, spawns the worker, and posts
    /// `WorktreeRemoveCompleted` on completion. Returns the Busy message so the
    /// caller can mint its keyed status op.
    pub fn dispatch_deferred_worktree_removal(
        &mut self,
        req: super::DeferredWorktreeRemoval,
    ) -> String {
        let super::DeferredWorktreeRemoval {
            session_id,
            project_path,
            managed,
            busy_message,
        } = req;
        let crate::model::ManagedWorkspace {
            worktree_path,
            branch_name,
            initial_branch,
            branch_provenance,
            ..
        } = managed;
        // RE-CHECK the occupancy the decision was made on. This removal was
        // planned when the delete began and runs only once the agent's PTYs
        // reap, which is seconds later; in that window another agent can come
        // to occupy the directory. `closing_sessions` does not cover it: that
        // blocks new TABS on the dying agent, not a new agent pointed at the
        // same place, which is exactly what creating a standalone agent there
        // does.
        //
        // Preserving the directory is the safe direction to be wrong in: the
        // worst case is a leftover the worktree manager can still remove, and
        // the alternative is `git worktree remove --force` on a directory a
        // live provider is running in.
        if let Some(occupant) = self.sessions.iter().find(|s| {
            s.id != session_id
                && crate::project_browser::same_directory(s.directory(), &worktree_path)
        }) {
            let message = format!(
                "Kept the worktree at \"{}\": agent \"{}\" started working in it while this \
                 agent was shutting down. Remove it from the worktree manager if you still \
                 want it gone.",
                crate::home_path::shorten_home(std::path::Path::new(&worktree_path)),
                occupant.display_label()
            );
            logger::warn(&message);
            return message;
        }
        // Guard against a duplicate worker (e.g. a project delete racing the
        // reap); the completion handler clears it.
        self.pending_deletions.insert(session_id.clone());
        self.deletion_busy_messages
            .insert(session_id.clone(), busy_message.clone());
        let tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // The same gate as the synchronous path: only branches dux
                // created are dux's to delete.
                if branch_provenance.dux_may_delete_branch() {
                    crate::git::remove_worktree(
                        std::path::Path::new(&project_path),
                        std::path::Path::new(&worktree_path),
                        &branch_name,
                        // The BIRTH branch too; see `git::remove_worktree`.
                        Some(initial_branch.as_str()),
                    )
                    .map(RemovedBranches::Deleted)
                    .map_err(|e| format!("{e:#}"))
                } else {
                    crate::git::remove_worktree_keep_branch(
                        std::path::Path::new(&project_path),
                        std::path::Path::new(&worktree_path),
                    )
                    .map(|()| RemovedBranches::Kept(branch_provenance))
                    .map_err(|e| format!("{e:#}"))
                }
            }))
            .unwrap_or_else(|payload| {
                let reason = crate::engine::spawn_worker::format_panic_payload(payload);
                crate::logger::error(&format!(
                    "deferred worktree-remove worker panicked for session {session_id}: {reason}"
                ));
                Err(format!("Worker panicked: {reason}"))
            });
            let _ =
                tx.send(crate::worker::WorkerEvent::WorktreeRemoveCompleted { session_id, result });
        });
        busy_message
    }

    pub fn process_agent_launch_failed(
        &mut self,
        data: AgentLaunchFailedData,
    ) -> (AgentLaunchFailedOutcome, Option<ResolvedFinal>) {
        let AgentLaunchFailedData { request, message } = data;
        // Clear the tab-keyed in-flight lock (== session.id for the session-slot tab),
        // mirroring the success path in `process_agent_launch_ready`.
        let tab_id = request.tab_id.clone();
        let session = request.session;
        self.clear_in_flight(&InFlightKey::AgentLaunch(tab_id.clone()));

        match request.kind {
            AgentLaunchKind::Create { status_op_id, .. } => {
                self.clear_in_flight(&InFlightKey::CreateAgent);
                // Resolve the shared create op to its keyed error final so both
                // surfaces replace the create busy in place with the same message.
                let create_final = self.resolve_create_op(
                    &status_op_id,
                    CreateLaunchOutcome::Failed {
                        message: message.clone(),
                    },
                );
                (
                    AgentLaunchFailedOutcome::Create {
                        project_id: session.project_id().map(str::to_string),
                        message,
                    },
                    create_final,
                )
            }
            AgentLaunchKind::Reconnect { .. } => (
                AgentLaunchFailedOutcome::Reconnect {
                    agent_label: session.display_label(),
                    session_id: session.id,
                    message,
                },
                None,
            ),
            AgentLaunchKind::ForceReconnect { .. } => (
                AgentLaunchFailedOutcome::ForceReconnect {
                    agent_label: session.display_label(),
                    session_id: session.id,
                    message,
                },
                None,
            ),
            AgentLaunchKind::ResumeFallback { .. } => {
                logger::error(&format!(
                    "fallback PTY spawn failed for {}: {}",
                    session.id, message,
                ));
                self.mark_session_status(&session.id, SessionStatus::Detached);
                (AgentLaunchFailedOutcome::ResumeFallback, None)
            }
            AgentLaunchKind::StartupAutoReopen => {
                logger::error(&format!(
                    "startup auto-reopen failed for agent \"{}\": {}",
                    session.display_label(),
                    message,
                ));
                (
                    AgentLaunchFailedOutcome::StartupAutoReopen {
                        agent_label: session.display_label(),
                        session_id: session.id,
                        message,
                    },
                    None,
                )
            }
            AgentLaunchKind::Tab { is_fresh, .. } => {
                // Ghost-launch guard, mirroring `process_agent_launch_ready`: a
                // extra tab whose row was deleted while its launch was in
                // flight must not be treated as a real failure. Without this,
                // an abort-during-launch would log an ERROR, call
                // `delete_agent_tab` again (hitting the "map and SQLite may
                // have diverged" WARN in storage.rs for a row that is already
                // gone), and surface a user-facing "Tab launch failed"
                // warning for a tab the user already closed. The session-slot tab
                // (`tab_id == session.id`, which never has an `agent_tabs`
                // row) is exempt, same as the ready-path guard.
                let is_main = tab_id == session.id;
                if !is_main && !self.agent_tabs.contains_key(&tab_id) {
                    logger::info(&format!(
                        "dropping launch-failed event for closed extra tab {tab_id} of session {}",
                        session.id,
                    ));
                    return (AgentLaunchFailedOutcome::Silent, None);
                }
                logger::error(&format!(
                    "extra tab {tab_id} launch failed for agent \"{}\": {}",
                    session.display_label(),
                    message,
                ));
                // A brand-new tab whose very first spawn failed never had a
                // conversation — remove its row so it does not linger as a
                // permanently-broken dormant tab. An explicit relaunch of an
                // already-persisted dormant tab keeps its row so the user can
                // retry; either way the real error is surfaced to the caller.
                if is_fresh {
                    // Persist-first (mirrors close_tab): only drop the in-memory
                    // entry once the row is actually gone, so a failed DB delete
                    // leaves a visible/closeable tab rather than an invisible
                    // ghost that still consumes a cap slot.
                    match self.session_store.delete_agent_tab(&tab_id) {
                        Ok(()) => {
                            self.agent_tabs.remove(&tab_id);
                        }
                        Err(err) => logger::error(&format!(
                            "failed to delete failed-create extra tab {tab_id}: {err}",
                        )),
                    }
                }
                (
                    AgentLaunchFailedOutcome::Tab {
                        agent_label: session.display_label(),
                        session_id: session.id,
                        tab_id,
                        message,
                    },
                    None,
                )
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
    /// so the deferred user commands are re-applied against it rather than dropped.
    /// The reload-failed reaction is placed LAST in the returned `Multi` so
    /// its error status wins the surface's status line instead of being overwritten
    /// by a deferred save's success message.
    fn process_config_reload_ready(&mut self, result: Result<Config, String>) -> EventReaction {
        let deferred = std::mem::take(&mut self.deferred_commands);
        let has_deferred = !deferred.is_empty();
        // Pre-swap `self.config` to the reloaded config (rather than leaving the
        // surface to do the swap) whenever we must base a follow-up write on the
        // reloaded config: a deferred command drain. With no deferral the engine
        // leaves `self.config` untouched so the surface can still diff old vs new.
        let must_preswap = has_deferred;

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
                    // Apply the reloaded config so the deferred drain re-mutates it
                    // (and the surfaced config carries those edits).
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

        if !must_preswap {
            // No pre-swap needed (no deferral): the bare reloaded config is
            // surfaced for the surface to swap. Exactly one of `bare_apply`
            // (success) / `failure` (parse error) is set; fall back to Nothing.
            return bare_apply.or(failure).unwrap_or(EventReaction::Nothing);
        }

        // Step 3: re-apply each deferred command now that the barrier is closed.
        // Each re-mutates the current config and eager-writes — the deferred write
        // is therefore the LAST write to disk. On a failed reload the config is
        // unchanged/current, so re-applying against it is still correct: deferred
        // commands are never dropped. Collect status reactions so the
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
            WorkerEvent::CreateAgentProgress {
                status_op_id,
                message,
            } => {
                // Re-emit an updated busy on the SAME opaque id via the op's
                // `progress`, without consuming the op (the eventual final still
                // resolves it). If the op is already gone the create has
                // resolved, so this progress tick is stale — drop it. (A busy can
                // only be born from a StatusOp; there is no hand-keyed fallback.)
                match self.pending_create_ops.get(&status_op_id) {
                    Some(op) => EventReaction::Status(op.progress(message)),
                    None => EventReaction::Nothing,
                }
            }
            WorkerEvent::CreateAgentFailed {
                status_op_id,
                message,
            } => {
                self.clear_in_flight(&InFlightKey::CreateAgent);
                // The create worker failed before any launch was attempted (e.g.
                // worktree creation failed). Resolve the shared create op to its
                // keyed error final so both surfaces replace the busy in place.
                match self.pending_create_ops.remove(&status_op_id) {
                    Some(op) => op
                        .resolve(&CreateLaunchOutcome::Failed { message })
                        .into_reaction(),
                    None => {
                        EventReaction::Status(StatusUpdate::error(message).with_key(status_op_id))
                    }
                }
            }
            WorkerEvent::AgentLaunchReady(boxed) => {
                let (outcome, create_final) = self.process_agent_launch_ready(*boxed);
                Self::launch_view_with_final(
                    EventReaction::AgentLaunchReadyView(Box::new(outcome)),
                    create_final,
                )
            }
            WorkerEvent::AgentLaunchFailed(boxed) => {
                let (outcome, create_final) = self.process_agent_launch_failed(*boxed);
                Self::launch_view_with_final(
                    EventReaction::AgentLaunchFailedView(Box::new(outcome)),
                    create_final,
                )
            }
            WorkerEvent::ChangedFilesReady { outcome, worktree } => {
                // A read git could not answer leaves the lists exactly as they
                // are. Emptying them would render an unreadable worktree as a
                // clean one, and the surface that asked for this refresh reports
                // the failure from the same event.
                let Ok((staged, unstaged)) = outcome else {
                    return EventReaction::Nothing;
                };
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
            WorkerEvent::FolderRepoStatusReady { session_id, status } => {
                self.clear_in_flight(&InFlightKey::FolderRepoProbe(session_id.clone()));
                // A verdict for an agent deleted while its probe was in
                // flight is dropped rather than stored: nothing would ever read
                // it, and nothing would ever remove it (the delete has already
                // run its own prune of this map).
                if !self.sessions.iter().any(|s| s.id == session_id) {
                    return EventReaction::Nothing;
                }
                let changed =
                    self.folder_repo_statuses.insert(session_id.clone(), status) != Some(status);
                // Re-enrol (or drop) the changed-files watch when the verdict
                // for the agent currently on screen actually moved. A folder
                // that just became a repository gets its panel; one that
                // stopped being one goes quiet, instead of the poller running
                // git in it every cycle and reporting "the repository is busy".
                if changed && self.watched_session_id.as_deref() == Some(session_id.as_str()) {
                    if let Some(worktree) = self.set_watched_session(Some(&session_id)) {
                        self.spawn_changed_files_refresh(worktree);
                    }
                    return EventReaction::ClampFilesCursor;
                }
                EventReaction::Nothing
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
                    && let Ok(outcome) = &result
                    && let Some(branch_name) = outcome.current_branch()
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
                        // A standalone agent has no branch to rename, so a
                        // rename can never have been dispatched for one, and
                        // asking the workspace is the structural restatement:
                        // this arm has no field to write when there is no
                        // branch.
                        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            let label = session.display_label();
                            if let Some(managed) = session.workspace.as_managed_mut() {
                                // Log lineage before mutating: new, previous, and
                                // the immutable original branch. `initial_branch`
                                // is never touched here.
                                let previous = managed.branch_name.clone();
                                let original = managed.initial_branch.clone();
                                logger::info(&branch_rename_log_line(
                                    &session.id,
                                    &label,
                                    &new_branch,
                                    &previous,
                                    &original,
                                ));
                                managed.branch_name = new_branch.clone();
                            }
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
                    Err(err) => {
                        logger::warn(&format!(
                            "[{session_id}] agent rename to {new_branch} failed: {err}"
                        ));
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
                // Clear the in-flight-rename guard set at dispatch, on BOTH
                // outcomes: the rename is over, so a subsequent `BranchSyncReady`
                // should resume classifying real external drift for this session.
                self.clear_in_flight(&InFlightKey::BranchRename(session_id.clone()));
                self.rename_expected.remove(&session_id);
                EventReaction::Multi(vec![
                    EventReaction::RebuildLeftItems,
                    status.into_reaction(),
                ])
            }
            WorkerEvent::BranchSyncReady(updates) => {
                let mut changed = false;
                for (session_id, actual_branch) in updates {
                    // In-flight-rename guard: the branch-sync poller can observe
                    // the user's OWN in-progress rename and would otherwise
                    // classify it as external drift — logging a false warning and,
                    // if it lands before `BranchRenameCompleted`, reading a
                    // corrupted `previous` (the already-updated branch). But the
                    // skip is SCOPED to the rename's own branches so an *unrelated*
                    // external change landing mid-rename isn't silently swallowed:
                    // skip quietly only when the observed branch is the still-
                    // pending old name or the expected new name; log an unexpected
                    // value and still skip (no mutation mid-rename — that races
                    // `BranchRenameCompleted`, which writes the authoritative
                    // branch and clears the marker). Check before the mutable
                    // borrow below.
                    if self.is_in_flight(&InFlightKey::BranchRename(session_id.clone())) {
                        match self.rename_expected.get(&session_id) {
                            Some(expected) if expected.matches(&actual_branch) => {}
                            Some(expected) => {
                                logger::warn(&format!(
                                    "[{session_id}] branch-sync observed unexpected branch '{actual_branch}' \
                                     while a rename to '{}' (from '{}') is in flight; deferring until the rename completes",
                                    expected.new_branch, expected.old_branch,
                                ));
                            }
                            None => {
                                logger::debug(&format!(
                                    "[{session_id}] branch-sync skipped mid-rename (no expected branch recorded); actual '{actual_branch}'",
                                ));
                            }
                        }
                        continue;
                    }
                    // Standalone agents are never enrolled in branch sync (no
                    // branch, nothing to watch), so no result can name one.
                    // Asking the workspace keeps this arm unreachable for them
                    // rather than merely unused.
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id)
                        && session.branch_name() != Some(actual_branch.as_str())
                    {
                        let label = session.display_label();
                        let Some(managed) = session.workspace.as_managed_mut() else {
                            continue;
                        };
                        // External drift: the worktree's current branch changed
                        // out from under us. Update the current branch label
                        // (correct — the label should track reality) but never
                        // `title` or `initial_branch`. Warn so the exact
                        // name-vs-branch scenario is greppable in the log.
                        let previous = managed.branch_name.clone();
                        let original = managed.initial_branch.clone();
                        logger::warn(&branch_drift_log_line(
                            &session.id,
                            &label,
                            &actual_branch,
                            &previous,
                            &original,
                        ));
                        managed.branch_name = actual_branch;
                        session.updated_at = Utc::now();
                        if let Err(err) = self.session_store.upsert_session(session) {
                            logger::error(&format!(
                                "failed to persist branch-sync update for {} (new branch: {:?}): {err}",
                                session.id,
                                session.branch_name(),
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
            WorkerEvent::GhStatusChecked {
                generation,
                outcome,
            } => {
                // Discard a stale result FIRST: before the status changes,
                // before the host policy changes, before it is logged, and
                // before it can start the pull-request workers. Two probes
                // launched close together can finish out of order, and an older
                // answer overwriting a newer one presents as intermittent.
                if generation != self.gh_probe.generation {
                    // Logged so this pairs with anything the worker itself wrote
                    // on its way here (the spawn primitive records a panic at
                    // error level before the synthesised result is built, and it
                    // is right to: the panic happened whether or not its answer
                    // is used). Debug rather than warn: overlapping probes are
                    // expected, not a fault.
                    logger::debug(&format!(
                        "[gh-integration] discarding a stale host probe result \
                         (generation {generation}, current {})",
                        self.gh_probe.generation,
                    ));
                    return EventReaction::Nothing;
                }
                // A probe that DECIDED may tear armed work down; a transient one
                // may not, because it decided nothing.
                let decisive = !matches!(outcome, crate::gh::GhProbe::Transient(_));
                let status = match outcome {
                    crate::gh::GhProbe::NotInstalled => {
                        // Deny all rather than preserving the last known set:
                        // `gh` is gone, so dux can reach none of those hosts.
                        self.set_github_host_policy(crate::gh::GithubHostPolicy::DenyAll);
                        GhStatus::NotInstalled
                    }
                    crate::gh::GhProbe::Transient(reason) => {
                        logger::info(&format!(
                            "[gh-integration] gh host probe did not decide ({reason}); \
                             keeping the last known host policy",
                        ));
                        // The previously computed value stands unchanged. The one
                        // exception is the very first probe: it must still move
                        // the status off Unknown to an unavailable one, so the
                        // interface reports something rather than rendering as
                        // neither available nor unavailable.
                        if matches!(self.gh_status, GhStatus::Unknown) {
                            GhStatus::NotAuthenticated
                        } else {
                            self.gh_status
                        }
                    }
                    crate::gh::GhProbe::Decided { available, policy } => {
                        self.set_github_host_policy(policy);
                        if available {
                            GhStatus::Available
                        } else {
                            GhStatus::NotAuthenticated
                        }
                    }
                };
                self.gh_status = status;
                if matches!(status, GhStatus::Available) && self.github_integration_enabled {
                    logger::info(&format!(
                        "[gh-integration] gh CLI is available; host policy: {:?}",
                        self.github_host_policy(),
                    ));
                    // This completion is the ONE place pull-request work is
                    // armed. Every off-to-on site launches the probe and stops
                    // there, so an enable produces exactly one refresh, and
                    // `spawn_pr_sync_worker` is single-instance so it produces
                    // at most one poller however often this runs.
                    //
                    // Re-seed from the store FIRST: a toggle-off cleared
                    // `pr_statuses`, and a manually attached PR must get its
                    // badge back the moment the integration re-arms rather
                    // than waiting for the first sync cycle. Idempotent (the
                    // stored rows are refreshed on every accepted result).
                    self.seed_pr_statuses_from_store();
                    self.update_pr_sync_sessions();
                    self.spawn_refs_watcher();
                    self.spawn_pr_sync_worker();
                    self.spawn_initial_pr_refresh();
                } else {
                    logger::info(&format!(
                        "[gh-integration] gh status: {:?}, integration enabled: {}",
                        status, self.github_integration_enabled,
                    ));
                    if decisive {
                        // `gh` answered, and the answer is that nothing works
                        // here (or the integration is off). Work armed from an
                        // older, better answer must not keep polling while the
                        // interface says GitHub is unavailable. A TRANSIENT
                        // result never reaches this: it decided nothing, so the
                        // last known good state stands.
                        self.disarm_pr_sync();
                    }
                }
                EventReaction::Nothing
            }
            WorkerEvent::PrStatusReady(results) => {
                let now = Instant::now();
                let mut changed = false;
                for (session_id, maybe_pr) in results {
                    // Clear any one-shot PR-check in-flight guard for this session
                    // (a no-op for batched-loop results, which set no key).
                    self.clear_in_flight(&InFlightKey::PrCheck(session_id.clone()));
                    // The check is async, so its result can land AFTER the
                    // session was deleted. Drop such a result whole: the
                    // sqlite upsert would fail the sessions FOREIGN KEY (an
                    // ERROR log on every delete-with-open-PR), and the map
                    // inserts would resurrect in-memory PR state for a
                    // session that no longer exists.
                    if !self.sessions.iter().any(|s| s.id == session_id) {
                        logger::debug(&format!(
                            "[gh-integration] dropping PR result for deleted session {session_id}",
                        ));
                        continue;
                    }
                    self.pr_last_checked.insert(session_id.clone(), now);
                    // Suppression guard, the in-flight race's answer. A check
                    // dispatched before the user detached can land after it,
                    // and re-badging an agent one tick after it was detached
                    // is exactly the bug the detach exists to fix. Dropped
                    // here, BEFORE `upsert_pr` and the `pr_statuses` insert,
                    // so nothing durable is written either. A pin is exempt:
                    // an attach lifts the suppression, so a session holding
                    // both is impossible, and the pin refresh path must keep
                    // working if one ever arose.
                    if self.pr_suppressions.contains(&session_id)
                        && !self.pr_overrides.contains_key(&session_id)
                    {
                        logger::debug(&format!(
                            "[gh-integration] dropping PR result for detached session \
                             {session_id}",
                        ));
                        continue;
                    }
                    // Identity guard for pinned sessions. This is deliberately
                    // NOT a `None`-only guard: several paths can still produce
                    // `Some(other_pr)` for a pinned session (a one-shot check
                    // racing the attach, or an early-return path answering
                    // from a stale `known_pr`), and a `None` from discovery
                    // must not clear a pin either. While an override exists,
                    // only a result matching the pin's (host, owner_repo,
                    // number) may touch `pr_statuses` or `upsert_pr`.
                    if let Some(pin) = self.pr_overrides.get(&session_id) {
                        let matches_pin = maybe_pr.as_ref().is_some_and(|pr| {
                            pr.number == pin.pr_number
                                && pr.owner_repo.eq_ignore_ascii_case(&pin.owner_repo)
                                && pr.host.eq_ignore_ascii_case(&pin.host)
                        });
                        if !matches_pin {
                            logger::debug(&format!(
                                "[gh-integration] dropping PR result for pinned session \
                                 {session_id} (does not match the pin, PR #{})",
                                pin.pr_number,
                            ));
                            continue;
                        }
                    }
                    match maybe_pr {
                        Some(pr) => {
                            let state_str = match pr.state {
                                PrState::Open => "OPEN",
                                PrState::Merged => "MERGED",
                                PrState::Closed => "CLOSED",
                            };
                            let pr_number = pr.number;
                            let row = StoredPr {
                                session_id: session_id.clone(),
                                pr_number,
                                host: pr.host.clone(),
                                owner_repo: pr.owner_repo.clone(),
                                state: state_str.to_string(),
                                title: pr.title.clone(),
                                url: pr.url.clone(),
                            };
                            if self.pr_overrides.contains_key(&session_id) {
                                // A PINNED session's accepted result refreshes
                                // the override row (its durable cache) and
                                // deliberately NEVER touches `session_prs`: a
                                // pin can live on a FORK, and a fork row in
                                // `session_prs` would become the post-detach
                                // `known_pr`, making the next cycle query the
                                // session's OWN repo with the fork's number.
                                if let Err(err) = self.session_store.upsert_pr_override(&row) {
                                    logger::error(&format!(
                                        "failed to refresh pinned PR for {session_id}: {err}",
                                    ));
                                }
                                self.pr_overrides.insert(session_id.clone(), row);
                            } else {
                                // Persist the autodetected association
                                // (including state) so it survives restarts
                                // and squash-merge branch deletions.
                                if let Err(err) = self.session_store.upsert_pr(&row) {
                                    logger::error(&format!(
                                        "failed to persist PR status for {session_id} (PR #{pr_number}): {err}",
                                    ));
                                }
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
            WorkerEvent::PrCheckAborted(session_id) => {
                // The one-shot check worker panicked; clear its guard so the next
                // trigger can retry. The badge is left untouched.
                self.clear_in_flight(&InFlightKey::PrCheck(session_id));
                EventReaction::Nothing
            }
            WorkerEvent::PullRequestReferenceResolved { .. } => {
                // Resolution is a question about the SURFACE's next screen (a
                // lookup, a picker over the matches, or a message naming a
                // repository dux has no project for), so the surface that asked
                // reads it off the channel itself. The engine has nothing to
                // decide here and deliberately does not invent a reaction.
                EventReaction::Nothing
            }
            WorkerEvent::PullRequestResolved {
                result,
                status_op_id,
                purpose: crate::worker::PrLookupPurpose::CreateAgent,
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
            WorkerEvent::PullRequestResolved {
                result,
                status_op_id,
                purpose: crate::worker::PrLookupPurpose::Attach { session_id },
            } => {
                // The one clear point for the mutual block. Keyed on the
                // purpose's session id rather than the status op id, so the
                // fallback below (no keyed op, or an op the map no longer
                // holds) unblocks the agent too. It runs before anything that
                // can return early, so success, failure, a session deleted
                // mid-lookup, and a panicking worker all end the block.
                self.clear_in_flight(&crate::engine::InFlightKey::PrAttach(session_id.clone()));
                // The attach application is engine-side and surface-agnostic:
                // this arm is the real vanished-session guard for BOTH
                // surfaces. The lookup is async, so a delete can land first,
                // and an unguarded apply would write an override row for a
                // dead id, exactly the orphan the storage cleanup defends
                // against. (`apply_pr_attach` re-checks too; the explicit
                // check here exists to say WHY nothing was attached.)
                let outcome = match result {
                    Ok(pr) => {
                        if !self.sessions.iter().any(|s| s.id == session_id) {
                            crate::engine::PrAttachOutcome::Failed {
                                message: format!(
                                    "The agent was deleted while PR #{} was being resolved; \
                                     nothing was attached.",
                                    pr.number,
                                ),
                            }
                        } else {
                            match self.apply_pr_attach(
                                &session_id,
                                &pr.host,
                                &pr.owner_repo,
                                pr.number,
                                &pr.title,
                                &pr.state,
                                "",
                            ) {
                                Ok(message) => crate::engine::PrAttachOutcome::Attached { message },
                                Err(err) => crate::engine::PrAttachOutcome::Failed {
                                    message: format!("Failed to attach PR #{}: {err:#}", pr.number),
                                },
                            }
                        }
                    }
                    Err(message) => crate::engine::PrAttachOutcome::Failed { message },
                };
                let attached = matches!(outcome, crate::engine::PrAttachOutcome::Attached { .. });
                let final_reaction = if let Some(id) = status_op_id
                    && let Some(op) = self.pending_pr_attach_ops.remove(&id)
                {
                    op.resolve(&outcome).into_reaction()
                } else {
                    // No keyed op (a defensive fallback): still surface the
                    // outcome rather than swallowing it.
                    match outcome {
                        crate::engine::PrAttachOutcome::Attached { message } => {
                            EventReaction::Status(StatusUpdate::info(message))
                        }
                        crate::engine::PrAttachOutcome::Failed { message } => {
                            EventReaction::Status(StatusUpdate::error(message))
                        }
                    }
                };
                if attached {
                    // The badge changed; the TUI sidebar re-derives from
                    // `pr_statuses` on rebuild (the web refetches the spine).
                    EventReaction::Multi(vec![final_reaction, EventReaction::RebuildLeftItems])
                } else {
                    final_reaction
                }
            }
            WorkerEvent::RefsChanged(session_id) => {
                logger::debug(&format!(
                    "[gh-integration] refs watcher: triggering PR check for session {}",
                    session_id,
                ));
                self.spawn_pr_check_for_session(&session_id, crate::engine::PR_CHECK_MIN_INTERVAL);
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
            WorkerEvent::ManageableWorktreesReady {
                project_id,
                result,
                status_op_id,
            } => EventReaction::ManageableWorktreesArrived {
                project_id,
                result,
                status_op_id,
            },
            WorkerEvent::WorktreeRemoveCompleted { session_id, result } => {
                // Always clear the in-flight guard so the session is
                // interactive again — whether we're about to remove it
                // (Ok path) or leave it in place for retry (Err path).
                self.pending_deletions.remove(&session_id);
                // The worktree removal is done (or failed and will be retried), so
                // the session is no longer "closing" — allow tab creation again.
                self.closing_sessions.remove(&session_id);

                // Retrieve (and remove) the exact Busy message we set when
                // the worker was spawned. The App compares this against the
                // current status-line content rather than checking tone
                // alone, because another operation (push, pull, refresh,
                // concurrent delete) may have since set its own Busy message
                // that must not be clobbered.
                let our_busy_msg = self.deletion_busy_messages.remove(&session_id);

                match result {
                    Ok(branches) => EventReaction::WorktreeRemoveSucceeded {
                        session_id,
                        branches,
                        our_busy_message: our_busy_msg,
                    },
                    Err(msg) => EventReaction::WorktreeRemoveFailed {
                        session_id,
                        message: msg,
                    },
                }
            }
            WorkerEvent::ResourceStatsReady(stats, was_baseline) => {
                self.clear_in_flight(&InFlightKey::ResourceStats);
                EventReaction::ResourceStatsArrived(stats, was_baseline)
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
            WorkerEvent::InitialCommitCreated {
                add,
                result,
                status_op_id,
            } => {
                // Release the per-path serialization gate now that the commit
                // attempt is done (success or failure).
                self.clear_in_flight(&InFlightKey::InitialCommit(add.path.clone()));
                match result {
                    Ok(()) => EventReaction::AddProjectAfterInitialCommit {
                        path: add.path,
                        name: add.name,
                        branch: add.branch,
                        leading_branch: add.leading_branch,
                        initialized_repo: add.initialized_repo,
                        seeded_gitignore: add.seeded_gitignore,
                        seed_warning: add.seed_warning,
                        // SUCCESS message is built in `drive_add_project_followup`
                        // after the inline add (web); the TUI builds it in its
                        // `AddProjectAfterInitialCommit` view handler.
                        status_op_id,
                    },
                    Err(err) => {
                        logger::error(&format!("initial commit failed for {}: {err}", add.path));
                        // A non-fatal seed failure must not be swallowed by a
                        // commit failure: the error's own recovery advice sends
                        // the user through the commit-only rung, which never
                        // seeds, so the warning is the only thing standing
                        // between them and an agent copying node_modules. Emit
                        // it as its own persistent warning ALONGSIDE the error
                        // final (the web shows both toasts; the TUI's single
                        // status line is documented-lossy and shows the last
                        // item, the error, whose advice is the primary next
                        // step).
                        let seed_warning = add
                            .seed_warning
                            .clone()
                            .map(|w| EventReaction::Status(StatusUpdate::warning(w)));
                        // Web path: resolve the keyed add-project op into its error
                        // final. TUI path (op map empty here) keeps the unkeyed Status.
                        let error_final = if let Some(id) = status_op_id
                            && let Some(op) = self.pending_web_add_project_ops.remove(&id)
                        {
                            op.resolve(&crate::engine::WebAddProjectOutcome::AddFailed {
                                message: err,
                            })
                            .into_reaction()
                        } else {
                            EventReaction::Status(StatusUpdate::error(err))
                        };
                        match seed_warning {
                            Some(warning) => EventReaction::Multi(vec![warning, error_final]),
                            None => error_final,
                        }
                    }
                }
            }
            WorkerEvent::CreateAgentBranchInspected {
                project,
                result,
                // The TUI resolves its keyed busy in `drain_events` (the op is
                // App-side); the engine keeps its unkeyed `Status`/view reactions.
                status_op_id: _,
            } => match result {
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
            WorkerEvent::StartupCommandLogsLoaded {
                scope_label,
                result,
            } => match result {
                Ok(listing) => EventReaction::StartupLogsArrived {
                    scope_label,
                    listing,
                },
                Err(err) => EventReaction::Status(StatusUpdate::error(format!(
                    "Could not read startup command logs for {scope_label}: {err}"
                ))),
            },
            WorkerEvent::StartupCommandLogContentLoaded { path, result } => {
                EventReaction::StartupLogContentArrived { path, result }
            }
            WorkerEvent::ServerFlipPreflightReady { result, warning } => {
                // No engine domain state to mutate — the listeners and the flip
                // are TUI concerns. Hand them straight to the App.
                EventReaction::ServerFlipPreflightReady { result, warning }
            }
            WorkerEvent::BackgroundServerPreflightReady { result, warning } => {
                // Same story: the listeners belong to whoever asked to serve.
                EventReaction::BackgroundServerPreflightReady { result, warning }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderCommandConfig;
    use crate::engine::test_support::{sample_project, sample_session, sample_tab, test_engine};
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
    fn finish_delete_session_clears_every_tab_and_drops_support_rows() {
        use std::time::Instant;
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        // An extra tab of s1 with runtime state spread across the maps, plus a
        // session-slot tab activity stamp.
        engine
            .agent_tabs
            .insert("tab-2".to_string(), sample_tab("tab-2", "s1", "codex", 1));
        engine
            .running_provider_pins
            .insert("tab-2".to_string(), ProviderKind::new("codex"));
        engine
            .pty_activity
            .insert("tab-2".to_string(), Instant::now());
        engine.pty_input.insert("tab-2".to_string(), Instant::now());
        // `pty_pointer` is asserted below alongside the others because its field
        // doc claims it is cleared wherever `pty_activity` is, and a claim
        // nothing pins is a claim that quietly stops being true.
        engine.note_pty_pointer("tab-2", crate::pty::PointerReport::Wheel);
        engine
            .resume_fallback_candidates
            .insert("tab-2".to_string(), Instant::now());
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.note_pty_pointer("s1", crate::pty::PointerReport::Wheel);

        engine
            .finish_delete_session("s1")
            .unwrap()
            .expect("outcome");

        // Every tab's runtime state is gone (Main AND Support), and the
        // extra-tab record is dropped from the in-memory map.
        for key in ["s1", "tab-2"] {
            assert!(!engine.pty_activity.contains_key(key));
            assert!(!engine.pty_input.contains_key(key));
            assert!(!engine.pty_pointer.contains_key(key));
            assert!(!engine.running_provider_pins.contains_key(key));
            assert!(!engine.resume_fallback_candidates.contains_key(key));
        }
        assert!(engine.agent_tabs.is_empty());
    }

    #[test]
    fn do_delete_session_clears_closing_flag_when_worktree_removal_fails() {
        // A failed synchronous worktree removal must still clear `closing_sessions`
        // so the agent isn't permanently barred from creating/relaunching tabs
        // (the async `WorktreeRemoveCompleted` handler already guarantees this;
        // the sync path must match it).
        let (mut engine, tmp) = test_engine();
        // A real (existing) worktree dir under a NON-git project: `git -C <proj>
        // worktree remove` fails, and because the path exists on disk
        // `remove_worktree` returns Err instead of the "already gone" Ok path.
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        engine
            .projects
            .push(sample_project("p1", proj.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "feat/x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let result = engine.do_delete_session("s1", true);

        assert!(
            result.is_err(),
            "removing a worktree from a non-git project must fail"
        );
        assert!(
            !engine.closing_sessions.contains("s1"),
            "closing_sessions must be cleared after a failed sync worktree removal"
        );
        // The delete aborted, so the session record survives.
        assert!(engine.sessions.iter().any(|s| s.id == "s1"));
    }

    /// The reported journey, end to end through the engine and a REAL repo:
    /// create an agent, let its branch drift, delete it with its worktree, and
    /// recreating it under the old name must not hit "branch already exists".
    /// Before the fix the birth branch survived, because the delete only ever
    /// saw `branch_name`, which the branch-sync poller had already rewritten.
    #[test]
    fn deleting_an_agent_whose_branch_drifted_removes_the_branch_it_was_born_on() {
        fn git(dir: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);

        let worktree = tmp.path().join("wt-born-here");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "born-here",
                worktree.to_str().unwrap(),
            ],
        );
        // The drift: the user switches the worktree onto a new branch, and the
        // branch-sync poller rewrites `branch_name` to follow it.
        git(&worktree, &["switch", "-c", "drifted"]);

        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "drifted");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "born-here".to_string();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .do_delete_session("s1", true)
            .unwrap()
            .expect("the delete should have run");

        assert_eq!(
            outcome.removal,
            WorktreeRemoval::Performed {
                branches: crate::engine::RemovedBranches::Deleted(crate::git::RemoveResult {
                    branch: crate::git::BranchDeletion::Deleted,
                    initial_branch: Some(crate::git::BranchDeletion::Deleted),
                }),
            },
            "both branches must be reported so the status line can name them"
        );
        let listed = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "branch", "--list"])
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&listed.stdout);
        assert!(
            !branches.contains("drifted"),
            "the current branch must be gone: {branches}"
        );
        assert!(
            !branches.contains("born-here"),
            "the branch the agent was born on must be gone too, or recreating it \
             fails with \"branch already exists\": {branches}"
        );
    }

    /// A repo on `main` with one commit, plus the named extra branches.
    #[cfg(test)]
    fn repo_with_branches(root: &std::path::Path, branches: &[&str]) -> std::path::PathBuf {
        fn git(dir: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        for branch in branches {
            git(&repo, &["branch", branch]);
        }
        repo
    }

    /// Attach a worktree at `repo/../wt-<branch>` to an EXISTING branch, the way
    /// the attach create arm does.
    #[cfg(test)]
    fn attach_worktree(repo: &std::path::Path, branch: &str) -> std::path::PathBuf {
        let worktree = repo.parent().unwrap().join(format!("wt-{branch}"));
        let out = std::process::Command::new("git")
            .args(["worktree", "add", worktree.to_str().unwrap(), branch])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        worktree
    }

    #[cfg(test)]
    fn branch_list(repo: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "branch", "--list"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn deleting_an_attached_agent_removes_the_worktree_and_keeps_the_branch() {
        // The whole point: `develop` existed before the agent, so the checkbox
        // takes the worktree and nothing else.
        let (mut engine, tmp) = test_engine();
        let repo = repo_with_branches(tmp.path(), &["develop"]);
        let worktree = attach_worktree(&repo, "develop");

        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "develop");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::AttachedExisting;
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .do_delete_session("s1", true)
            .unwrap()
            .expect("the delete should have run");

        assert_eq!(
            outcome.removal,
            WorktreeRemoval::Performed {
                branches: RemovedBranches::Kept(crate::model::BranchProvenance::AttachedExisting),
            },
            "nothing was deleted, so the outcome must not carry a deletion report"
        );
        assert!(!worktree.exists(), "the worktree must be gone");
        let branches = branch_list(&repo);
        assert!(
            branches.contains("develop"),
            "a branch that existed before the agent must survive it: {branches}"
        );
    }

    #[test]
    fn deleting_a_drifted_attached_agent_keeps_both_branches() {
        // Drift inside an attached agent creates a SECOND branch. Both are kept:
        // the gate is per agent, not per branch.
        let (mut engine, tmp) = test_engine();
        let repo = repo_with_branches(tmp.path(), &["develop"]);
        let worktree = attach_worktree(&repo, "develop");
        let out = std::process::Command::new("git")
            .args(["switch", "-c", "feature-x"])
            .current_dir(&worktree)
            .output()
            .unwrap();
        assert!(out.status.success());

        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        // The branch-sync poller has already followed the drift.
        let mut session = sample_session("s1", "p1", "feature-x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "develop".to_string();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::AttachedExisting;
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine.do_delete_session("s1", true).unwrap().expect("ran");

        let branches = branch_list(&repo);
        assert!(
            branches.contains("develop") && branches.contains("feature-x"),
            "both branches must survive: {branches}"
        );
        // And the message names both, with a reason each: "existed before this
        // agent" is false of the branch the drift created.
        let message = crate::wire::delete_session_status_message(&outcome.finish, &outcome.removal);
        assert!(
            message.contains("\"feature-x\" was created inside this agent's worktree and was kept")
                && message.contains("\"develop\" existed before this agent and was kept"),
            "each kept branch needs its own reason: {message}"
        );
    }

    #[test]
    fn deleting_a_project_keeps_the_branch_of_an_attached_agent() {
        // The cascade calls `do_delete_session` per agent, so it inherits the
        // gate: removing a project must not take the user's `develop` with it.
        let (mut engine, tmp) = test_engine();
        let repo = repo_with_branches(tmp.path(), &["develop", "dux-made"]);
        let attached = attach_worktree(&repo, "develop");
        let owned = attach_worktree(&repo, "dux-made");

        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut a = sample_session("s1", "p1", "develop");
        a.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::AttachedExisting;
        a.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = attached.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&a).unwrap();
        engine.sessions.push(a);
        let mut b = sample_session("s2", "p1", "dux-made");
        b.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::CreatedByDux;
        b.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = owned.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&b).unwrap();
        engine.sessions.push(b);

        engine
            .apply(crate::engine::Command::DeleteProject {
                project_id: "p1".to_string(),
                project_name: "repo".to_string(),
            })
            .unwrap();

        let branches = branch_list(&repo);
        assert!(
            branches.contains("develop"),
            "the attached agent's pre-existing branch must survive the project delete: {branches}"
        );
        assert!(
            !branches.contains("dux-made"),
            "a branch dux created is still cleaned up by the cascade: {branches}"
        );
    }

    #[test]
    fn re_adopting_an_orphaned_worktree_launders_a_dux_made_branch_into_a_kept_one() {
        // ACCEPTED behavior, pinned so nobody "fixes" it by accident. Deleting
        // without the checkbox keeps the worktree and destroys the session row,
        // and the provenance dies with it. Re-adopting that orphan yields
        // Adopted, so a branch dux originally minted now survives deletion.
        // Unknowable is treated as not-ours: losing a cleanup is recoverable,
        // losing a branch is not. The worktree manager is the manual way out.
        let (mut engine, tmp) = test_engine();
        let repo = repo_with_branches(tmp.path(), &["dux-made"]);
        let worktree = attach_worktree(&repo, "dux-made");

        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut first = sample_session("s1", "p1", "dux-made");
        first
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::CreatedByDux;
        first
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&first).unwrap();
        engine.sessions.push(first);

        // Delete WITHOUT the checkbox: worktree and branch stay, row goes.
        engine.do_delete_session("s1", false).unwrap().expect("ran");
        assert!(worktree.exists());

        // Re-adopt the orphan.
        let mut second = sample_session("s2", "p1", "dux-made");
        second
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::Adopted;
        second
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_str().unwrap().to_string();
        engine.session_store.upsert_session(&second).unwrap();
        engine.sessions.push(second);

        let outcome = engine.do_delete_session("s2", true).unwrap().expect("ran");

        assert_eq!(
            outcome.removal,
            WorktreeRemoval::Performed {
                branches: RemovedBranches::Kept(crate::model::BranchProvenance::Adopted),
            }
        );
        let branches = branch_list(&repo);
        assert!(
            branches.contains("dux-made"),
            "the laundered branch survives, deliberately: {branches}"
        );
    }

    #[test]
    fn clearing_a_tab_retires_the_form_it_launched_with() {
        // The sticky launched form is what lets a live tab keep its quoting after
        // its provider is renamed out of config. It must RETIRE with the process:
        // an entry that outlived its tab would keep publishing a provider name the
        // workspace no longer runs, and a later tab launching under that same name
        // would inherit a form nobody configured.
        let (mut engine, _tmp) = test_engine();
        engine.launched_drop_paste.insert(
            "s1".to_string(),
            crate::engine::LaunchedDropPaste {
                provider: "codex".to_string(),
                form: crate::config::WebDragDropPaste::SingleQuoted,
                command_name: "codex".to_string(),
            },
        );
        engine.clear_tab_runtime("s1");
        assert!(
            !engine.launched_drop_paste.contains_key("s1"),
            "the launched paste profile must be torn down with the tab, like \
             every other tab-keyed runtime map"
        );
    }

    #[test]
    fn detach_conflicting_tears_down_all_tabs_but_keeps_support_rows() {
        use crate::pty::PtyClient;
        use std::time::Instant;
        let (mut engine, _tmp) = test_engine();
        let tmp = tempfile::tempdir().expect("worktree dir");
        let worktree = tmp.path().to_string_lossy().to_string();

        // The conflicting ("victim") session that holds the shared worktree's live
        // PTY, plus an extra tab with runtime state.
        let mut victim = sample_session("victim", "p1", "feat");
        victim
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.clone();
        engine.sessions.push(victim);
        engine.agent_tabs.insert(
            "v-tab".to_string(),
            sample_tab("v-tab", "victim", "codex", 1),
        );
        engine.providers.insert(
            "victim".to_string(),
            PtyClient::spawn_with_env("cat", &[], tmp.path(), 24, 80, 1000, &[]).unwrap(),
        );
        engine
            .running_provider_pins
            .insert("v-tab".to_string(), ProviderKind::new("codex"));
        engine
            .pty_activity
            .insert("v-tab".to_string(), Instant::now());

        // A second session sharing the same worktree requests it.
        let mut requester = sample_session("req", "p1", "feat2");
        requester
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.clone();
        engine.sessions.push(requester);

        let detached = engine.detach_conflicting_worktree_session(&worktree, "req");
        assert_eq!(detached.map(|d| d.id), Some("victim".to_string()));
        // Every tab of the victim is torn down (Main provider + the extra tab's
        // runtime maps)...
        assert!(!engine.providers.contains_key("victim"));
        assert!(!engine.pty_activity.contains_key("v-tab"));
        assert!(!engine.running_provider_pins.contains_key("v-tab"));
        // ...but its extra-tab ROW survives: the session still exists, detached.
        assert!(engine.agent_tabs.contains_key("v-tab"));
    }

    /// Two spellings of one directory are one directory. The comparison used to
    /// be a raw string compare, so a symlinked path let a second agent launch a
    /// provider in a worktree another agent was already running in, which is the
    /// shared-conversation hazard every other same-directory check in dux
    /// compares canonically to avoid.
    #[test]
    fn detach_conflicting_sees_through_a_symlinked_spelling_of_the_worktree() {
        use crate::pty::PtyClient;
        let (mut engine, _tmp) = test_engine();
        let tmp = tempfile::tempdir().expect("worktree dir");
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).expect("real worktree");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink the worktree");

        let mut victim = sample_session("victim", "p1", "feat");
        victim
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = real.to_string_lossy().to_string();
        engine.sessions.push(victim);
        engine.providers.insert(
            "victim".to_string(),
            PtyClient::spawn_with_env("cat", &[], &real, 24, 80, 1000, &[]).unwrap(),
        );

        let detached =
            engine.detach_conflicting_worktree_session(link.to_string_lossy().as_ref(), "req");
        assert_eq!(detached.map(|d| d.id), Some("victim".to_string()));
        assert!(!engine.providers.contains_key("victim"));
    }

    #[test]
    fn detach_conflicting_detects_a_conflict_when_only_a_support_tab_is_live() {
        use crate::pty::PtyClient;
        let (mut engine, _tmp) = test_engine();
        let tmp = tempfile::tempdir().expect("worktree dir");
        let worktree = tmp.path().to_string_lossy().to_string();

        // Victim whose SESSION-SLOT tab is dead (absent from `providers`) but a Support
        // tab is still live in the shared worktree.
        let mut victim = sample_session("victim", "p1", "feat");
        victim
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.clone();
        engine.sessions.push(victim);
        engine.agent_tabs.insert(
            "v-tab".to_string(),
            sample_tab("v-tab", "victim", "codex", 1),
        );
        // Provider keyed under the EXTRA tab id, not the session/Main id.
        engine.providers.insert(
            "v-tab".to_string(),
            PtyClient::spawn_with_env("cat", &[], tmp.path(), 24, 80, 1000, &[]).unwrap(),
        );

        let mut requester = sample_session("req", "p1", "feat2");
        requester
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.clone();
        engine.sessions.push(requester);

        // A Main-only check would MISS this (no "victim" key in `providers`); the
        // tab-aware detection finds it and tears the live extra tab down.
        let detached = engine.detach_conflicting_worktree_session(&worktree, "req");
        assert_eq!(detached.map(|d| d.id), Some("victim".to_string()));
        assert!(!engine.providers.contains_key("v-tab"));
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // `cat` is always on PATH and simply echoes — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        assert!(
            engine
                .companion_terminals
                .values()
                .any(|t| t.owner == crate::model::TerminalOwner::Session("s1".to_string()))
        );

        engine
            .finish_delete_session("s1")
            .unwrap()
            .expect("outcome");

        assert!(
            !engine
                .companion_terminals
                .values()
                .any(|t| t.owner == crate::model::TerminalOwner::Session("s1".to_string())),
            "deleted session's companion terminals should be removed"
        );
    }

    #[test]
    fn finish_delete_session_leaves_project_terminals() {
        let (mut engine, _tmp) = test_engine();

        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat/x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_tid, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");

        engine
            .finish_delete_session("s1")
            .unwrap()
            .expect("outcome");

        assert!(
            engine.companion_terminals.contains_key(&project_tid),
            "deleting an agent must not delete the project's own terminals"
        );
        assert!(
            !engine
                .companion_terminals
                .values()
                .any(|t| t.owner == crate::model::TerminalOwner::Session("s1".to_string())),
            "the session's own terminals are removed"
        );
    }

    #[test]
    fn finish_delete_session_detects_sibling_on_same_worktree() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let mut sibling = sample_session("s1", "p1", "feat/x");
        let mut deleted = sample_session("s2", "p1", "feat/y");
        // Force both to share a worktree path.
        sibling
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/wt/shared".to_string();
        deleted
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/wt/shared".to_string();
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
            EventReaction::BrowserEntriesArrived { .. } => "BrowserEntriesArrived",
            EventReaction::ProjectWorktreesArrived { .. } => "ProjectWorktreesArrived",
            EventReaction::ManageableWorktreesArrived { .. } => "ManageableWorktreesArrived",
            EventReaction::OpenNewAgentPromptForPr { .. } => "OpenNewAgentPromptForPr",
            EventReaction::WorktreeRemoveSucceeded { .. } => "WorktreeRemoveSucceeded",
            EventReaction::WorktreeRemoveFailed { .. } => "WorktreeRemoveFailed",
            EventReaction::ResourceStatsArrived(_, _) => "ResourceStatsArrived",
            EventReaction::AddProjectAfterBranchCheckout { .. } => "AddProjectAfterBranchCheckout",
            EventReaction::AddProjectAfterInitialCommit { .. } => "AddProjectAfterInitialCommit",
            EventReaction::ContinueCreateAgentAfterInspection { .. } => {
                "ContinueCreateAgentAfterInspection"
            }
            EventReaction::DispatchProjectDefaultBranchCheckout { .. } => {
                "DispatchProjectDefaultBranchCheckout"
            }
            EventReaction::ApplyReloadedConfig(_) => "ApplyReloadedConfig",
            EventReaction::OpenConfigReloadFailedModal(_) => "OpenConfigReloadFailedModal",
            EventReaction::ProjectPersistenceOutcome(_) => "ProjectPersistenceOutcome",
            EventReaction::StartupLogsArrived { .. } => "StartupLogsArrived",
            EventReaction::StartupLogContentArrived { .. } => "StartupLogContentArrived",
            EventReaction::FinishDeleteSessionView(_) => "FinishDeleteSessionView",
            EventReaction::DoDeleteSessionView(_) => "DoDeleteSessionView",
            EventReaction::BeginDeleteSessionView(_) => "BeginDeleteSessionView",
            EventReaction::DispatchAgentLaunchView(_) => "DispatchAgentLaunchView",
            EventReaction::DeleteTerminalView(_) => "DeleteTerminalView",
            EventReaction::ServerFlipPreflightReady { .. } => "ServerFlipPreflightReady",
            EventReaction::BackgroundServerPreflightReady { .. } => {
                "BackgroundServerPreflightReady"
            }
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
            result: Ok(crate::worker::PullOutcome::Pulled {
                current_branch: Some("feature-x".to_string()),
            }),
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

    /// A no-origin refresh still updates the project's current branch and
    /// resolves the keyed info final (nothing to pull is not a failure).
    #[test]
    fn pull_completed_project_no_origin_updates_branch_and_resolves_info() {
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
            result: Ok(crate::worker::PullOutcome::NoOrigin {
                current_branch: Some("feature-x".to_string()),
            }),
            status: crate::engine::ResolvedFinal::new(
                "pull-project:p1",
                crate::engine::Final::info(
                    "Project \"p1-name\" has no origin remote; nothing to pull. Local branch state refreshed.",
                ),
            ),
        });

        assert!(!engine.is_in_flight(&InFlightKey::Pull(repo_path.clone())));
        let p = &engine.projects[0];
        assert_eq!(p.current_branch, "feature-x");

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(status.key.as_deref(), Some("pull-project:p1"));
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
        assert_eq!(s.branch_name().expect("managed test session"), "new");
        assert!(s.updated_at >= before_updated_at);

        // Verify the upsert hit the session store.
        let loaded = engine.session_store.load_sessions().expect("load");
        let stored = loaded.iter().find(|s| s.id == "s1").expect("stored s1");
        assert_eq!(stored.branch_name(), Some("new"));
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

    #[test]
    fn branch_sync_updates_current_branch_but_not_title_or_initial_branch() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "server-mode");
        session.title = Some("server-mode".into());
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "server-mode".into();
        engine.sessions.push(session);

        engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "agent-tabs".to_string(),
        )]));

        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        // The current branch follows git...
        assert_eq!(s.branch_name().expect("managed test session"), "agent-tabs");
        // ...but the human name and the original branch are durable/immutable.
        assert_eq!(s.title.as_deref(), Some("server-mode"));
        assert_eq!(
            s.initial_branch().expect("managed test session"),
            "server-mode"
        );
    }

    #[test]
    fn rename_log_line_includes_session_id_new_previous_and_original() {
        let msg = branch_rename_log_line("sess-1", "My Agent", "XYZ", "ABC", "DEF");
        // Past tense (fires after the git rename succeeded) and carries the
        // session identifier + label the code it replaced logged.
        assert_eq!(
            msg,
            "[sess-1] agent \"My Agent\" renamed branch to XYZ from ABC (original branch name was DEF)"
        );
        assert!(msg.contains("sess-1"));
        assert!(msg.contains("renamed"));
        assert!(!msg.contains("renaming"));
    }

    #[test]
    fn drift_log_line_includes_session_id_new_previous_and_original() {
        let msg = branch_drift_log_line(
            "sess-1",
            "My Agent",
            "agent-tabs",
            "server-mode",
            "server-mode",
        );
        assert_eq!(
            msg,
            "[sess-1] agent \"My Agent\" branch changed externally to agent-tabs from server-mode \
             (original was server-mode) — if unexpected, check for git activity in the worktree outside dux"
        );
        assert!(msg.contains("sess-1"));
        // The actionable clause must be present so a reader knows what to check.
        assert!(msg.contains("check for git activity in the worktree outside dux"));
    }

    #[test]
    fn branch_sync_skips_session_with_rename_in_flight() {
        // F-D: a session whose own rename is mid-flight must not be treated as
        // external drift by the branch-sync poller — no mutation, no warn.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "server-mode");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "server-mode".into();
        engine.sessions.push(session);
        // Simulate the dispatch marker set by `apply_rename_session`.
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));

        // The poller observes the (about-to-be) renamed branch first.
        let reaction = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "agent-tabs".to_string(),
        )]));

        // Nothing changed: the guard skipped the session entirely.
        assert!(matches!(reaction, EventReaction::Nothing));
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.branch_name().expect("managed test session"),
            "server-mode",
            "the mid-rename session's branch must not be mutated by branch-sync"
        );
        // The session store was never upserted for s1 (no drift persisted).
        let loaded = engine.session_store.load_sessions().expect("load");
        assert!(loaded.iter().all(|s| s.id != "s1"));
    }

    #[test]
    fn branch_sync_scoped_skip_ignores_expected_rename_branches() {
        // The scoped in-flight guard skips silently only for the rename's own
        // expected branches (still-pending old OR target new). Both must be
        // skipped without mutating the session.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "old-branch".into();
        engine.sessions.push(session);
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));
        engine.rename_expected.insert(
            "s1".into(),
            crate::engine::RenameExpectation {
                old_branch: "old-branch".into(),
                new_branch: "new-branch".into(),
            },
        );

        // Observing the expected NEW branch mid-rename is skipped.
        let r = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "new-branch".to_string(),
        )]));
        assert!(matches!(r, EventReaction::Nothing));
        // Observing the still-pending OLD branch is also skipped.
        let r = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "old-branch".to_string(),
        )]));
        assert!(matches!(r, EventReaction::Nothing));

        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.branch_name().expect("managed test session"),
            "old-branch",
            "expected rename branches must never mutate the session mid-rename"
        );
    }

    /// A deferred worktree removal that finds another agent living in the
    /// directory keeps it, and says who is there.
    ///
    /// The removal is planned when the delete begins and runs only once the
    /// dying agent's PTYs reap. A standalone agent created in that window
    /// occupies the directory, and `closing_sessions` does not see it: that
    /// blocks new TABS on the dying agent, not a new agent pointed at the same
    /// place. Without the re-check this ran `git worktree remove --force` on a
    /// directory a live provider was working in.
    #[test]
    fn a_deferred_removal_keeps_a_directory_another_agent_moved_into() {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("live.txt"), "in use\n").unwrap();
        // The agent that moved in while the other was shutting down, reaching
        // the directory by a different spelling for good measure.
        let link = tmp.path().join("link-to-wt");
        std::os::unix::fs::symlink(&worktree, &link).unwrap();
        engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "sa1",
                &link.to_string_lossy(),
            ));

        let message =
            engine.dispatch_deferred_worktree_removal(crate::engine::DeferredWorktreeRemoval {
                session_id: "s1".to_string(),
                project_path: repo.to_string_lossy().to_string(),
                managed: crate::model::ManagedWorkspace {
                    project_id: "p1".to_string(),
                    project_path: None,
                    source_branch: "main".to_string(),
                    branch_name: "feat".to_string(),
                    initial_branch: "feat".to_string(),
                    branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string_lossy().to_string(),
                },
                busy_message: "Removing worktree\u{2026}".to_string(),
            });

        assert!(
            message.contains("Kept the worktree"),
            "the outcome must say the directory stayed, got {message:?}"
        );
        assert!(
            message.contains("sa1-title"),
            "and name who is in it, got {message:?}"
        );
        assert!(worktree.exists(), "the directory must still be there");
        assert!(
            !engine.pending_deletions.contains("s1"),
            "no removal worker was dispatched, so nothing may be left marked pending"
        );
    }

    #[test]
    fn branch_sync_unexpected_branch_mid_rename_is_deferred_not_applied() {
        // The scoped guard's UNEXPECTED path: a branch that is neither the
        // pending old name nor the target new name appears while a rename is in
        // flight. The guard logs the anomaly but still defers (no mutation
        // mid-rename — that would race `BranchRenameCompleted`).
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "old-branch".into();
        engine.sessions.push(session);
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));
        engine.rename_expected.insert(
            "s1".into(),
            crate::engine::RenameExpectation {
                old_branch: "old-branch".into(),
                new_branch: "new-branch".into(),
            },
        );

        let r = engine.process_worker_event(WorkerEvent::BranchSyncReady(vec![(
            "s1".to_string(),
            "surprise-branch".to_string(),
        )]));

        assert!(matches!(r, EventReaction::Nothing));
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.branch_name().expect("managed test session"),
            "old-branch",
            "an unexpected mid-rename branch must not mutate the session"
        );
        // Nothing was persisted for s1 (no drift written).
        let loaded = engine.session_store.load_sessions().expect("load");
        assert!(loaded.iter().all(|s| s.id != "s1"));
    }

    #[test]
    fn revert_optimistic_rename_unwinds_title_marker_and_expectation() {
        // On a synchronous worker-spawn failure no `BranchRenameCompleted`
        // fires, so the call site must unwind the optimistic state itself.
        // `revert_optimistic_rename` restores the title, clears the in-flight
        // marker, and drops the expected-branch stash — otherwise the Busy would
        // hang forever and drift detection would be frozen for the session.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session.title = Some("optimistic-new-name".into());
        engine.sessions.push(session);
        // The optimistic state `apply_rename_session` sets up before dispatch.
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));
        engine.rename_expected.insert(
            "s1".into(),
            crate::engine::RenameExpectation {
                old_branch: "old-branch".into(),
                new_branch: "new-branch".into(),
            },
        );

        engine.revert_optimistic_rename("s1", Some("original-title".into()));

        assert!(
            !engine.is_in_flight(&InFlightKey::BranchRename("s1".into())),
            "the in-flight marker must be cleared so future renames aren't blocked"
        );
        assert!(
            !engine.rename_expected.contains_key("s1"),
            "the expected-branch stash must be dropped so branch-sync resumes"
        );
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.title.as_deref(),
            Some("original-title"),
            "the optimistic title must be reverted"
        );
        // The revert was persisted (reload sees the restored title).
        let loaded = engine.session_store.load_sessions().expect("load");
        let stored = loaded.iter().find(|s| s.id == "s1").expect("stored s1");
        assert_eq!(stored.title.as_deref(), Some("original-title"));
    }

    #[test]
    fn branch_rename_completed_error_clears_marker_and_expected_and_reverts_title() {
        // The Err arm (also the shape the panic_event synthesises) must revert
        // the title AND clear both the in-flight marker and the expected-branch
        // stash so drift detection is never permanently frozen.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session.title = Some("renamed-optimistically".into());
        engine.sessions.push(session);
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));
        engine.rename_expected.insert(
            "s1".into(),
            crate::engine::RenameExpectation {
                old_branch: "old-branch".into(),
                new_branch: "new-branch".into(),
            },
        );

        engine.process_worker_event(WorkerEvent::BranchRenameCompleted {
            session_id: "s1".into(),
            new_branch: "new-branch".into(),
            previous_title: Some("original-title".into()),
            result: Err("boom".into()),
            status: crate::engine::ResolvedFinal::error("k", "failed"),
        });

        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
        assert!(!engine.rename_expected.contains_key("s1"));
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.title.as_deref(),
            Some("original-title"),
            "the optimistic title must be reverted on failure"
        );
    }

    #[test]
    fn panicking_rename_worker_still_clears_in_flight_marker() {
        // A rename worker that panics must not permanently freeze drift
        // detection: the panic-safe primitive's `panic_event` synthesises the
        // completion event, whose handler clears the in-flight marker. This
        // exercises the real spawn→panic→panic_event→handler path.
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "p1", "old-branch"));
        engine.rename_expected.insert(
            "s1".into(),
            crate::engine::RenameExpectation {
                old_branch: "old-branch".into(),
                new_branch: "new-branch".into(),
            },
        );

        engine.spawn_background_worker(
            crate::engine::BackgroundWorkerSpec {
                label: "branch-rename-test".into(),
                in_flight_key: Some(InFlightKey::BranchRename("s1".into())),
                panic_event: Some(Box::new(|reason| WorkerEvent::BranchRenameCompleted {
                    session_id: "s1".into(),
                    new_branch: "new-branch".into(),
                    previous_title: None,
                    result: Err(reason.clone()),
                    status: crate::engine::ResolvedFinal::error("k", format!("panic: {reason}")),
                })),
            },
            |_tx| panic!("simulated rename worker panic"),
        );

        // The marker is set by spawn_background_worker until the completion
        // event arrives.
        assert!(engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));

        // Drain the synthesised panic completion and process it.
        let ev = engine
            .worker_rx
            .recv()
            .expect("panic_event should post a completion");
        engine.process_worker_event(ev);

        assert!(
            !engine.is_in_flight(&InFlightKey::BranchRename("s1".into())),
            "a panicking rename worker must still clear the in-flight marker"
        );
        assert!(
            !engine.rename_expected.contains_key("s1"),
            "a panicking rename worker must still clear the expected-branch stash"
        );
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
            outcome: Ok((
                vec![sample_changed_file("staged.txt")],
                vec![sample_changed_file("unstaged.txt")],
            )),
            worktree,
        });

        // The view follow-up that repaints the TUI's changed-files pane.
        assert!(matches!(reaction, EventReaction::ClampFilesCursor));
        assert_eq!(engine.staged_files.len(), 1);
        assert_eq!(engine.staged_files[0].path, "staged.txt");
        assert_eq!(engine.unstaged_files.len(), 1);
        assert_eq!(engine.unstaged_files[0].path, "unstaged.txt");
    }

    /// A read git could not answer must not be flattened into "no changes":
    /// blanking the pane would tell the user their worktree is clean when dux
    /// has no idea what is in it.
    #[test]
    fn changed_files_ready_failure_leaves_the_lists_alone() {
        let (mut engine, _tmp) = test_engine();
        let worktree = PathBuf::from("/tmp/wt-current");
        *engine.watched_worktree.lock().unwrap() = Some(worktree.clone());
        engine.staged_files = vec![sample_changed_file("keep-staged.txt")];
        engine.unstaged_files = vec![sample_changed_file("keep-unstaged.txt")];

        let reaction = engine.process_worker_event(WorkerEvent::ChangedFilesReady {
            outcome: Err("git status failed: index.lock exists".to_string()),
            worktree,
        });

        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.staged_files[0].path, "keep-staged.txt");
        assert_eq!(engine.unstaged_files[0].path, "keep-unstaged.txt");
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
            outcome: Ok((
                vec![sample_changed_file("stale-staged.txt")],
                vec![sample_changed_file("stale-unstaged.txt")],
            )),
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
            outcome: Ok((
                vec![sample_changed_file("stale.txt")],
                vec![sample_changed_file("stale.txt")],
            )),
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
    fn pr_status_ready_skips_results_for_deleted_sessions() {
        // The PR check is async: its result can land AFTER the session was
        // deleted. Applying it anyway used to (a) attempt an sqlite upsert
        // that failed the sessions FOREIGN KEY, logging a scary ERROR on
        // every delete-with-open-PR, and (b) re-insert in-memory PR status
        // and a poll timestamp for a session that no longer exists.
        let (mut engine, _tmp) = test_engine();
        let pr = PrInfo {
            number: 7,
            state: PrState::Open,
            title: "stale".into(),
            host: "github.com".into(),
            owner_repo: "o/r".into(),
            url: "https://example".into(),
        };

        // "ghost" is not a session the engine knows (deleted before the
        // result arrived). The result must be dropped whole: no status, no
        // timestamp, no store row, and no changed-flag rebuild.
        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "ghost".to_string(),
            Some(pr),
        )]));

        assert!(
            matches!(reaction, EventReaction::Nothing),
            "a ghost-only batch changes nothing, got {}",
            reaction_kind(&reaction),
        );
        assert!(!engine.pr_statuses.contains_key("ghost"));
        assert!(!engine.pr_last_checked.contains_key("ghost"));
        let stored = engine
            .session_store
            .load_all_latest_prs()
            .expect("load prs");
        assert!(stored.iter().all(|p| p.session_id != "ghost"));
    }

    #[test]
    fn deleting_a_session_drops_its_pr_runtime_state() {
        // The delete itself must clear the PR maps so a deleted agent leaves
        // no in-memory PR residue behind (the store rows cascade with the
        // session row).
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", "/tmp/p1");
        engine.projects.push(project);
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let session_id = "s1".to_string();
        let pr = PrInfo {
            number: 9,
            state: PrState::Open,
            title: "doomed".into(),
            host: "github.com".into(),
            owner_repo: "o/r".into(),
            url: "https://example".into(),
        };
        engine.pr_statuses.insert(session_id.clone(), pr);
        engine
            .pr_last_checked
            .insert(session_id.clone(), Instant::now());
        engine.pr_overrides.insert(
            session_id.clone(),
            crate::storage::StoredPr {
                session_id: session_id.clone(),
                pr_number: 9,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "OPEN".to_string(),
                title: "doomed".to_string(),
                url: "https://example".to_string(),
            },
        );

        engine
            .finish_delete_session_memory(&session_id)
            .expect("delete the session");

        assert!(!engine.pr_statuses.contains_key(&session_id));
        assert!(!engine.pr_last_checked.contains_key(&session_id));
        assert!(
            !engine.pr_overrides.contains_key(&session_id),
            "the in-memory pin must not outlive its session"
        );
    }

    #[test]
    fn pr_status_ready_none_removes_existing_and_records_timestamps() {
        let (mut engine, _tmp) = test_engine();
        // Results only apply to sessions the engine still knows (late results
        // for deleted sessions are dropped), so s1/s2 must exist.
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        engine.sessions.push(sample_session("s1", "p1", "feat/a"));
        engine.sessions.push(sample_session("s2", "p1", "feat/b"));
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
        // s1 must exist for its result to apply at all (ghost results drop).
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        engine.sessions.push(sample_session("s1", "p1", "feat/a"));
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

    /// The identity guard, in both directions. While a session is pinned, a
    /// sync result is accepted ONLY when its (host, owner_repo, number) matches
    /// the pin: a `Some(other_pr)` (the one-shot check racing an attach, or an
    /// early-return path answering from a stale `known_pr`) must not overwrite
    /// the pin, and a `None` must not clear it.
    #[test]
    fn pr_status_ready_identity_guard_drops_results_that_do_not_match_the_pin() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);
        let pinned = crate::storage::StoredPr {
            session_id: "s1".to_string(),
            pr_number: 12,
            host: "github.com".to_string(),
            owner_repo: "forker/Hello-World".to_string(),
            state: "OPEN".to_string(),
            title: "Pinned".to_string(),
            url: "https://github.com/forker/Hello-World/pull/12".to_string(),
        };
        engine.pr_statuses.insert(
            "s1".to_string(),
            crate::gh::reconstruct_pr_from_stored(&pinned).unwrap(),
        );
        engine.pr_overrides.insert("s1".to_string(), pinned);

        // Direction 1: a racing one-shot answers with a DIFFERENT PR.
        let other = PrInfo {
            number: 50,
            state: PrState::Open,
            title: "Autodetected".to_string(),
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
            url: "https://github.com/octocat/Hello-World/pull/50".to_string(),
        };
        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "s1".to_string(),
            Some(other),
        )]));
        assert!(
            matches!(reaction, EventReaction::Nothing),
            "a non-pin result changes nothing, got {}",
            reaction_kind(&reaction),
        );
        assert_eq!(
            engine.pr_statuses.get("s1").map(|p| p.number),
            Some(12),
            "the badge still shows the pin"
        );
        assert!(
            engine
                .session_store
                .load_all_latest_prs()
                .unwrap()
                .is_empty(),
            "the dropped result never reaches upsert_pr"
        );

        // Direction 2: a None (e.g. discovery finding nothing) cannot clear it.
        let reaction =
            engine.process_worker_event(WorkerEvent::PrStatusReady(vec![("s1".to_string(), None)]));
        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.pr_statuses.get("s1").map(|p| p.number), Some(12));

        // A result matching the pin IS accepted, and refreshes the override
        // row's cached state so a restart renders the fresh state.
        let refreshed = PrInfo {
            number: 12,
            state: PrState::Merged,
            title: "Pinned".to_string(),
            host: "github.com".to_string(),
            owner_repo: "forker/Hello-World".to_string(),
            url: "https://github.com/forker/Hello-World/pull/12".to_string(),
        };
        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "s1".to_string(),
            Some(refreshed),
        )]));
        assert!(matches!(reaction, EventReaction::RebuildLeftItems));
        assert_eq!(
            engine.pr_statuses.get("s1").map(|p| p.state.clone()),
            Some(PrState::Merged)
        );
        let rows = engine.session_store.load_pr_overrides().unwrap();
        assert_eq!(rows[0].state, "MERGED", "the pin's cached state refreshes");
        assert_eq!(
            engine.pr_overrides.get("s1").map(|p| p.state.as_str()),
            Some("MERGED"),
            "the in-memory pin refreshes too"
        );
        // An accepted PINNED result must never land in `session_prs`: the
        // override row is the pin's durable cache, and a fork row written into
        // `session_prs` would become the post-detach `known_pr`, making the
        // next cycle emit the FORK's number against the session's OWN repo.
        assert!(
            engine
                .session_store
                .load_all_latest_prs()
                .unwrap()
                .is_empty(),
            "a pinned cycle leaves session_prs untouched"
        );
    }

    /// The full detach cycle: pin a FORK PR, accept one pinned
    /// sync result, detach, and check what the next cycle would do. Under the
    /// detach-suppresses-everything rule the session is simply absent from the
    /// snapshot, so there is no cycle to smuggle the fork into; the residue
    /// half is still asserted at the store, because a resume later puts the
    /// session back in the plan and `session_prs` is what feeds its `known_pr`.
    #[test]
    fn detaching_a_fork_pin_leaves_no_fork_residue_for_the_next_cycle() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);
        engine
            .apply_pr_attach(
                "s1",
                "github.com",
                "forker/Hello-World",
                12,
                "Pinned",
                "OPEN",
                "",
            )
            .expect("attach the fork pin");

        // One accepted pinned cycle (the state refresh path).
        let refreshed = PrInfo {
            number: 12,
            state: PrState::Open,
            title: "Pinned".to_string(),
            host: "github.com".to_string(),
            owner_repo: "forker/Hello-World".to_string(),
            url: "https://github.com/forker/Hello-World/pull/12".to_string(),
        };
        engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "s1".to_string(),
            Some(refreshed),
        )]));

        engine.clear_pull_request_override("s1").expect("detach");

        // The next cycle's snapshot: the detached session is not in it at all.
        assert!(
            engine.pr_sync_sessions.lock().unwrap().is_empty(),
            "a detached session is excluded from the plan entirely"
        );
        // And the fork left no residue behind for a later resume to pick up:
        // a pinned cycle never writes `session_prs`, so the row that would
        // become a resumed session's `known_pr` does not exist.
        assert!(
            engine
                .session_store
                .load_all_latest_prs()
                .expect("load stored prs")
                .is_empty(),
            "the fork pin must leave nothing in session_prs to resume onto"
        );
        engine.resume_pr_autodetection("s1").expect("resume");
        let entries = engine.pr_sync_sessions.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].pinned.is_none());
        assert!(
            entries[0].known_pr.is_none(),
            "the fork pin must not survive detach as known_pr, got {:?}",
            entries[0].known_pr,
        );
    }

    /// The in-flight race: a PR check dispatched BEFORE the detach answers
    /// after it. The result must be dropped before it can reach `upsert_pr` or
    /// the badge, or the agent re-badges one tick after the user detached it.
    #[test]
    fn an_in_flight_pr_result_for_a_suppressed_session_is_dropped() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);
        engine.clear_pull_request_override("s1").expect("detach");

        let late = PrInfo {
            number: 12,
            state: PrState::Open,
            title: "Detected while the detach was landing".to_string(),
            host: "github.com".to_string(),
            owner_repo: "o/r".to_string(),
            url: "https://github.com/o/r/pull/12".to_string(),
        };
        let reaction = engine.process_worker_event(WorkerEvent::PrStatusReady(vec![(
            "s1".to_string(),
            Some(late),
        )]));

        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "a late result must not re-badge a detached agent"
        );
        assert!(
            engine
                .session_store
                .load_all_latest_prs()
                .expect("load stored prs")
                .is_empty(),
            "and it must not be persisted either, or a restart would resurrect it"
        );
        assert!(
            matches!(reaction, EventReaction::Nothing),
            "nothing changed, so there is nothing to rebuild"
        );
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
            result: Ok(crate::engine::RemovedBranches::Deleted(
                crate::git::RemoveResult {
                    branch: crate::git::BranchDeletion::AlreadyGone,
                    initial_branch: None,
                },
            )),
        });

        assert!(!engine.pending_deletions.contains("s1"));
        assert!(!engine.deletion_busy_messages.contains_key("s1"));

        match reaction {
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branches,
                our_busy_message,
            } => {
                assert_eq!(session_id, "s1");
                let crate::engine::RemovedBranches::Deleted(branches) = branches else {
                    panic!("a created-by-dux agent's branches are deleted, not kept");
                };
                assert_eq!(branches.branch, crate::git::BranchDeletion::AlreadyGone);
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
    fn create_agent_failed_flips_inflight_and_resolves_the_op_error() {
        let (mut engine, _tmp) = test_engine();
        engine.mark_in_flight(InFlightKey::CreateAgent);
        // Stash a create op as the dispatch would; the failure resolves it to a
        // same-key error final.
        let op = crate::engine::status_op("Creating a new agent\u{2026}").resolve_in_handler(
            |o: &crate::engine::CreateLaunchOutcome| match o {
                crate::engine::CreateLaunchOutcome::Failed { message } => {
                    crate::engine::Final::error(message.clone())
                }
                _ => crate::engine::Final::clear(),
            },
        );
        let op_id = op.id().to_string();
        engine.pending_create_ops.insert(op_id.clone(), op);

        let reaction = engine.process_worker_event(WorkerEvent::CreateAgentFailed {
            status_op_id: op_id.clone(),
            message: "nope".to_string(),
        });

        assert!(!engine.is_in_flight(&InFlightKey::CreateAgent));
        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.message, "nope");
        // The failure carries the op's opaque id so the web replaces the
        // "Creating a new agent…" loading toast in place, and the op is consumed.
        assert_eq!(status.key.as_deref(), Some(op_id.as_str()));
        assert!(engine.pending_create_ops.is_empty());
    }

    #[test]
    fn created_session_correlation_resolves_and_prunes() {
        use std::time::{Duration, Instant};

        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine.sessions.push(sample_session("s2", "p1", "feat2"));

        // Recording an op→session pair makes it resolvable by op id.
        engine.record_created_session("op-1".to_string(), "s1".to_string());
        assert_eq!(
            engine.created_session_for_op("op-1"),
            Some("s1".to_string())
        );
        assert_eq!(engine.created_session_for_op("missing"), None);

        // An entry whose session no longer exists is pruned on the next insert,
        // so the map cannot accumulate dead entries on a long-running server.
        engine.record_created_session("op-ghost".to_string(), "gone".to_string());
        engine.record_created_session("op-2".to_string(), "s2".to_string());
        assert_eq!(engine.created_session_for_op("op-ghost"), None);
        assert_eq!(
            engine.created_session_for_op("op-2"),
            Some("s2".to_string())
        );

        // An entry past the TTL reads as absent even before its prune.
        engine.created_session_by_op.insert(
            "op-stale".to_string(),
            (
                "s1".to_string(),
                Instant::now() - (crate::engine::CREATED_SESSION_TTL + Duration::from_secs(1)),
            ),
        );
        assert_eq!(engine.created_session_for_op("op-stale"), None);
    }

    #[test]
    fn create_agent_progress_re_emits_a_busy_on_the_op_id() {
        // The progress event re-emits a busy on the create op's opaque id without
        // consuming the op, so the dispatch busy and every progress render as one
        // in-place toast that the final dismisses.
        let (mut engine, _tmp) = test_engine();
        let op = crate::engine::status_op("Creating a new agent\u{2026}").resolve_in_handler(
            |_: &crate::engine::CreateLaunchOutcome| crate::engine::Final::clear(),
        );
        let op_id = op.id().to_string();
        engine.pending_create_ops.insert(op_id.clone(), op);

        let reaction = engine.process_worker_event(WorkerEvent::CreateAgentProgress {
            status_op_id: op_id.clone(),
            message: "Launching codex in a fresh session...".to_string(),
        });

        let status = unwrap_status(reaction);
        assert_eq!(status.tone, StatusTone::Busy);
        assert_eq!(status.key.as_deref(), Some(op_id.as_str()));
        // The op is NOT consumed by progress.
        assert!(engine.pending_create_ops.contains_key(&op_id));
    }

    #[test]
    fn create_progress_busy_is_dismissed_by_the_keyed_failure() {
        // End-to-end on the keyed controller: a busy progress on the op's id
        // followed by a keyed error on the SAME id replaces it in place, so the
        // controller never strands a busy entry (and the web toast is reused,
        // not duplicated).
        use crate::statusline::{KeyedStatusController, StatusTone as Tone};
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut controller = KeyedStatusController::with_clear_after(Duration::from_secs(6));

        // Drive a real op end-to-end so the progress and the failure share its id.
        let (mut engine, _tmp) = test_engine();
        let op = crate::engine::status_op("Creating a new agent\u{2026}").resolve_in_handler(
            |o: &crate::engine::CreateLaunchOutcome| match o {
                crate::engine::CreateLaunchOutcome::Failed { message } => {
                    crate::engine::Final::error(message.clone())
                }
                _ => crate::engine::Final::clear(),
            },
        );
        let op_id = op.id().to_string();
        engine.pending_create_ops.insert(op_id.clone(), op);

        let progress = unwrap_status(engine.process_worker_event(
            WorkerEvent::CreateAgentProgress {
                status_op_id: op_id.clone(),
                message: "Attaching to existing branch \"x\" for project \"y\"...".to_string(),
            },
        ));
        controller.set(
            now,
            progress.key.clone(),
            Tone::Busy,
            progress.message.clone(),
        );
        assert_eq!(controller.snapshot().len(), 1);
        assert_eq!(controller.snapshot()[0].tone, "busy");

        let failure = engine.process_worker_event(WorkerEvent::CreateAgentFailed {
            status_op_id: op_id.clone(),
            message: "Failed to create a new worktree.".to_string(),
        });
        let failure = unwrap_status(failure);
        controller.set(
            now,
            failure.key.clone(),
            Tone::Error,
            failure.message.clone(),
        );
        // Still one entry on the same id — the busy was replaced, not stacked.
        let snap = controller.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].key.as_deref(), Some(op_id.as_str()));
        assert_eq!(snap[0].tone, "error");
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
        let session = sample_session(session_id, "project-1", branch);
        AgentLaunchFailedData {
            request: AgentLaunchRequest {
                tab_id: session.id.clone(),
                provider: session.provider.clone(),
                session,
                provider_config: ProviderCommandConfig::default(),
                env: Vec::new(),
                identity: Default::default(),
                resume: false,
                pty_size: (24, 80),
                scrollback_lines: 1000,
                kind,
                wants_fullscreen: false,
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
                status_op_id: String::new(),
            },
            "boom",
        );
        let (outcome, _create_final) = engine.process_agent_launch_failed(data);
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
        let (outcome, _create_final) = engine.process_agent_launch_failed(data);
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
        let (outcome, _create_final) = engine.process_agent_launch_failed(data);
        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::Reconnect { session_id, agent_label, message }
                if session_id == "s1" && agent_label == "s1-title" && message == "boom"
        ));
    }

    #[test]
    fn process_agent_launch_failed_startup_auto_reopen_returns_branch_and_message() {
        let (mut engine, _tmp) = test_engine();
        let data = make_failed_data("s1", "feat/x", AgentLaunchKind::StartupAutoReopen, "boom");
        let (outcome, _create_final) = engine.process_agent_launch_failed(data);
        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::StartupAutoReopen { session_id, agent_label, message }
                if session_id == "s1" && agent_label == "s1-title" && message == "boom"
        ));
    }

    /// Build failed-launch data for an extra tab: `tab_id` differs from the
    /// session id, mirroring a real extra-tab launch.
    fn make_tab_failed_data(
        session_id: &str,
        tab_id: &str,
        branch: &str,
        is_fresh: bool,
        message: &str,
    ) -> AgentLaunchFailedData {
        let session = sample_session(session_id, "project-1", branch);
        AgentLaunchFailedData {
            request: AgentLaunchRequest {
                tab_id: tab_id.to_string(),
                provider: session.provider.clone(),
                session,
                provider_config: ProviderCommandConfig::default(),
                env: Vec::new(),
                identity: Default::default(),
                resume: false,
                pty_size: (24, 80),
                scrollback_lines: 1000,
                kind: AgentLaunchKind::Tab {
                    is_fresh,
                    status_message: String::new(),
                },
                wants_fullscreen: false,
            },
            message: message.to_string(),
        }
    }

    #[test]
    fn process_agent_launch_failed_tab_returns_message_when_row_still_exists() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "project-1", "feat/x"));
        let tab = crate::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.agent_tabs.insert(tab.id.clone(), tab);

        let data = make_tab_failed_data("s1", "tab-1", "feat/x", true, "boom");
        let (outcome, _) = engine.process_agent_launch_failed(data);

        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::Tab { tab_id, agent_label, message, .. }
                if tab_id == "tab-1" && agent_label == "s1-title" && message == "boom"
        ));
        // G-T1: a brand-new tab's very first spawn failure (`is_fresh: true`)
        // must delete the row so it doesn't linger as a permanently-broken
        // dormant tab, both in memory and in SQLite.
        assert!(
            !engine.agent_tabs.contains_key("tab-1"),
            "a fresh tab's row must be deleted in memory on first-launch failure"
        );
        assert!(
            engine
                .session_store
                .load_agent_tabs()
                .expect("load tabs")
                .iter()
                .all(|t| t.id != "tab-1"),
            "a fresh tab's row must be deleted in SQLite on first-launch failure"
        );
    }

    #[test]
    fn process_agent_launch_failed_tab_keeps_row_when_not_fresh() {
        // G-T1: the counterpart to the fresh-delete case above. An explicit
        // relaunch of an already-persisted dormant tab (`is_fresh: false`) must
        // KEEP its row on failure so the user can retry, surfacing the real
        // error instead of silently losing the tab.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "project-1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        let data = make_tab_failed_data("s1", "tab-1", "feat/x", false, "boom");
        let (outcome, _) = engine.process_agent_launch_failed(data);

        assert!(matches!(
            outcome,
            AgentLaunchFailedOutcome::Tab { tab_id, agent_label, message, .. }
                if tab_id == "tab-1" && agent_label == "s1-title" && message == "boom"
        ));
        assert!(
            engine.agent_tabs.contains_key("tab-1"),
            "a not-fresh (explicit relaunch) tab's row must survive a failure so the user can retry"
        );
        assert!(
            engine
                .session_store
                .load_agent_tabs()
                .expect("load tabs")
                .iter()
                .any(|t| t.id == "tab-1"),
            "the row must also survive in SQLite"
        );
    }

    #[test]
    fn process_agent_launch_failed_tab_is_silent_for_a_ghost_tab() {
        // An extra tab whose row was deleted (closed by the
        // user) while its launch was in flight must not be treated as a real
        // failure — no ERROR log's worth of user-facing warning, and no
        // redundant `delete_agent_tab` call against a row that is already
        // gone. The engine has no `agent_tabs` row for "tab-1" here, exactly
        // like a tab closed mid-launch.
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "project-1", "feat/x"));

        let data = make_tab_failed_data("s1", "tab-1", "feat/x", true, "boom");
        let (outcome, create_final) = engine.process_agent_launch_failed(data);

        assert!(matches!(outcome, AgentLaunchFailedOutcome::Silent));
        assert!(create_final.is_none());
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
    fn process_project_persistence_completed_remove_closes_project_terminals() {
        // The TUI's remove-project path lands here (not in Command::RemoveProject),
        // so the orphan cascade must live in this arm too: deleting it would keep
        // the rest of the suite green while re-introducing the unkillable
        // orphaned project terminal.
        let (mut engine, _tmp) = test_engine();
        let repo1 = tempfile::tempdir().expect("p1 dir");
        let repo2 = tempfile::tempdir().expect("p2 dir");
        engine.projects.push(sample_project(
            "p1",
            repo1.path().to_string_lossy().as_ref(),
        ));
        engine.projects.push(sample_project(
            "p2",
            repo2.path().to_string_lossy().as_ref(),
        ));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (t1, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("terminal on p1");
        let (t2, _) = engine
            .create_project_terminal("p2", 24, 80)
            .expect("terminal on p2");

        let action = ProjectPersistenceAction::Remove {
            project_id: "p1".to_string(),
            project_name: "Pee One".to_string(),
        };
        engine.process_project_persistence_completed(action, Ok(()), None);

        assert!(
            !engine.companion_terminals.contains_key(&t1),
            "removing a project must close its project terminals"
        );
        assert!(
            engine.terminating_ptys.iter().any(|t| t.id == t1),
            "the closed terminal is reaped gracefully via the terminating set"
        );
        assert!(
            engine.companion_terminals.contains_key(&t2),
            "another project's terminal must be untouched"
        );
    }

    #[test]
    fn process_project_persistence_completed_delete_closes_project_terminals() {
        // The TUI's delete-project path drives the ::Delete arm; same cascade,
        // same orphan risk.
        let (mut engine, _tmp) = test_engine();
        let repo1 = tempfile::tempdir().expect("p1 dir");
        let repo2 = tempfile::tempdir().expect("p2 dir");
        engine.projects.push(sample_project(
            "p1",
            repo1.path().to_string_lossy().as_ref(),
        ));
        engine.projects.push(sample_project(
            "p2",
            repo2.path().to_string_lossy().as_ref(),
        ));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (t1, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("terminal on p1");
        let (t2, _) = engine
            .create_project_terminal("p2", 24, 80)
            .expect("terminal on p2");

        let action = ProjectPersistenceAction::Delete {
            project_id: "p1".to_string(),
            project_name: "Pee One".to_string(),
        };
        engine.process_project_persistence_completed(action, Ok(()), None);

        // Pin the surviving project by name: "p1 is absent" alone is also true
        // of a delete that wiped every project.
        assert_eq!(
            engine
                .projects
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["p2"],
            "only the deleted project is gone",
        );
        assert!(
            !engine.companion_terminals.contains_key(&t1),
            "deleting a project must close its project terminals"
        );
        assert!(
            engine.terminating_ptys.iter().any(|t| t.id == t1),
            "the closed terminal is reaped gracefully via the terminating set"
        );
        assert!(
            engine.companion_terminals.contains_key(&t2),
            "another project's terminal must be untouched"
        );
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
    fn begin_delete_session_refuses_while_a_tab_is_launching() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = sample_tab("tab-1", "s1", "codex", 1);
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);
        // An extra tab whose launch is in flight is marked in-flight but not yet
        // in `providers`, so it is invisible to the live-tab check. Deleting must
        // refuse rather than race the worktree removal against the spawn.
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-1".to_string()));
        let outcome = engine.begin_delete_session("s1", true);
        assert!(matches!(outcome, BeginDeleteSessionOutcome::TabLaunching));
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
        a.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/shared".to_string();
        b.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/shared".to_string();
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
        a.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/shared".to_string();
        b.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/tmp/shared".to_string();
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

    #[test]
    fn do_delete_session_refuses_worktree_removal_while_a_tab_is_launching() {
        // Round-2 fix: a tab whose launch is in flight is marked in-flight but not
        // yet in `providers`, so the pre-kill can't reach it — a worktree-removing
        // delete must refuse rather than race git::remove_worktree against the
        // spawning provider.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        // The session-slot tab's id equals the session id; mark its launch in flight.
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".to_string()));

        let outcome = engine
            .do_delete_session("s1", true)
            .expect("soft-return does not error");
        assert!(
            outcome.is_none(),
            "do_delete_session must soft-return Ok(None) while a tab is launching",
        );
        assert!(
            engine.sessions.iter().any(|s| s.id == "s1"),
            "session should be untouched when the tab-launch guard fires",
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
            copy_uncommitted_changes: false,
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
            tab_id: session.id.clone(),
            provider: session.provider.clone(),
            session,
            provider_config: ProviderCommandConfig::default(),
            env: Vec::new(),
            identity: Default::default(),
            resume: false,
            pty_size: (24, 80),
            scrollback_lines: 1000,
            kind: AgentLaunchKind::Reconnect {
                status_message: String::new(),
            },
            wants_fullscreen: false,
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

        engine.spawn_pr_check_for_session("s1", crate::engine::PR_CHECK_MIN_INTERVAL);

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
        engine.spawn_pr_check_for_session("s1", crate::engine::PR_CHECK_MIN_INTERVAL);

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

    #[test]
    fn foreground_pr_check_uses_tighter_window_than_background() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.sessions.push(sample_session("s1", "p1", "feat/x"));
        // Last checked 5s ago: inside the 10s background window, outside the 3s
        // foreground window.
        let five_ago = Instant::now()
            .checked_sub(std::time::Duration::from_secs(5))
            .unwrap();
        engine.pr_last_checked.insert("s1".to_string(), five_ago);

        // Background window (10s) → suppressed, timestamp unchanged.
        engine.spawn_pr_check_for_session("s1", crate::engine::PR_CHECK_MIN_INTERVAL);
        assert_eq!(engine.pr_last_checked.get("s1").copied(), Some(five_ago));

        // Foreground window (3s) → proceeds, timestamp refreshed.
        engine.spawn_foreground_pr_check("s1");
        assert!(engine.pr_last_checked.get("s1").copied().unwrap() > five_ago);
    }

    fn backoff_map() -> std::sync::Arc<std::sync::Mutex<crate::gh::BackoffSnapshot>> {
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    fn host_signal(host: &str, remaining: Option<i64>, hard_failed: bool) -> crate::gh::HostSignal {
        crate::gh::HostSignal {
            host: host.to_string(),
            rate: remaining.map(|remaining| crate::gh::RateLimitInfo {
                remaining,
                reset_at: Some(Utc::now() + chrono::Duration::seconds(120)),
            }),
            hard_failed,
            rate_limited: false,
        }
    }

    fn rate_limited_signal(host: &str) -> crate::gh::HostSignal {
        crate::gh::HostSignal {
            host: host.to_string(),
            rate: None,
            hard_failed: true,
            rate_limited: true,
        }
    }

    #[test]
    fn apply_pr_backoff_pauses_and_warns_when_low_per_host() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        Engine::apply_pr_backoff(&shared, &[host_signal("github.com", Some(5), false)], &tx);

        // The window is set for that host, ~120s out (the reset), not "now".
        let until = *shared
            .lock()
            .unwrap()
            .get("github.com")
            .expect("backoff set");
        let secs = until.saturating_duration_since(Instant::now()).as_secs();
        assert!((90..=130).contains(&secs), "backoff ~120s, got {secs}s");
        // Info-toned (self-dismissing), keyed per host, and worded as rate-limiting.
        match rx.try_recv() {
            Ok(WorkerEvent::CommandWorkerStarted(s)) => {
                assert!(s.key.is_some());
                assert_eq!(s.tone, crate::statusline::StatusTone::Info);
                assert!(s.message.contains("rate limit"), "got: {}", s.message);
            }
            _ => panic!("expected a keyed quota-low notice"),
        }
    }

    #[test]
    fn apply_pr_backoff_rate_limited_says_so_and_backs_off_longer() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        Engine::apply_pr_backoff(&shared, &[rate_limited_signal("github.com")], &tx);
        // A rate-limit hard failure pauses for the longer window, not the 60s one.
        let until = *shared
            .lock()
            .unwrap()
            .get("github.com")
            .expect("backoff set");
        let secs = until.saturating_duration_since(Instant::now()).as_secs();
        assert!(secs > 120, "rate-limit backoff should be long, got {secs}s");
        match rx.try_recv() {
            Ok(WorkerEvent::CommandWorkerStarted(s)) => {
                assert_eq!(s.tone, crate::statusline::StatusTone::Info);
                assert!(
                    s.message.to_lowercase().contains("rate-limiting"),
                    "got: {}",
                    s.message,
                );
            }
            _ => panic!("expected a keyed rate-limit notice"),
        }
    }

    #[test]
    fn apply_pr_backoff_warns_only_once_while_active() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        let sig = [host_signal("github.com", Some(5), false)];
        Engine::apply_pr_backoff(&shared, &sig, &tx);
        Engine::apply_pr_backoff(&shared, &sig, &tx);
        // First call warns; the second (window still active) must be silent.
        assert!(matches!(
            rx.try_recv(),
            Ok(WorkerEvent::CommandWorkerStarted(_))
        ));
        assert!(rx.try_recv().is_err(), "warning must fire only once");
    }

    #[test]
    fn apply_pr_backoff_hard_failure_not_masked_by_healthy_rate() {
        // A host with a HEALTHY rate reading AND hard_failed=true must still back
        // off (regression guard for the masking bug).
        let (tx, _rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        Engine::apply_pr_backoff(&shared, &[host_signal("github.com", Some(5000), true)], &tx);
        assert!(
            shared.lock().unwrap().contains_key("github.com"),
            "a hard failure must pause even with a healthy quota reading",
        );
    }

    #[test]
    fn apply_pr_backoff_is_per_host() {
        // One bad host must not pause a healthy one.
        let (tx, _rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        Engine::apply_pr_backoff(
            &shared,
            &[
                host_signal("github.com", Some(5000), false),
                host_signal("ghe.corp", None, true),
            ],
            &tx,
        );
        let map = shared.lock().unwrap();
        assert!(!map.contains_key("github.com"), "healthy host not paused");
        assert!(map.contains_key("ghe.corp"), "failing host paused");
    }

    #[test]
    fn apply_pr_backoff_clears_backed_off_host_silently() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        shared.lock().unwrap().insert(
            "github.com".to_string(),
            Instant::now() + std::time::Duration::from_secs(120),
        );
        Engine::apply_pr_backoff(
            &shared,
            &[host_signal("github.com", Some(5000), false)],
            &tx,
        );
        assert!(
            !shared.lock().unwrap().contains_key("github.com"),
            "healthy signal clears that host's backoff",
        );
        // No "resumed" toast: the Info-toned pause notice already auto-cleared, so a
        // fresh message on recovery would be stale.
        assert!(rx.try_recv().is_err(), "recovery must be silent");
    }

    #[test]
    fn apply_pr_backoff_healthy_from_idle_is_silent() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = backoff_map();
        Engine::apply_pr_backoff(
            &shared,
            &[host_signal("github.com", Some(5000), false)],
            &tx,
        );
        assert!(shared.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "no message when never backed off");
    }

    #[test]
    fn pr_check_skips_while_its_host_is_backed_off_and_resumes_after() {
        // A future backoff window for the session's host makes the check a no-op
        // (via the in-flight/host skip inside the sync); an expired window lets it
        // proceed. Here we assert the shared-map contract the sync relies on.
        let shared = backoff_map();
        shared.lock().unwrap().insert(
            "github.com".to_string(),
            Instant::now() + std::time::Duration::from_secs(60),
        );
        let snap = shared.lock().unwrap().clone();
        assert!(
            snap.get("github.com").is_some_and(|u| Instant::now() < *u),
            "an active window must read as future",
        );
        // Expired window: no longer blocks.
        shared.lock().unwrap().insert(
            "github.com".to_string(),
            Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        );
        let snap = shared.lock().unwrap().clone();
        assert!(
            snap.get("github.com").is_none_or(|u| Instant::now() >= *u),
            "an expired window must not block",
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
                    status_op_id: "op-test".to_string(),
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
                let _ = tx.send(WorkerEvent::ResourceStatsReady(Vec::new(), false));
            },
        );
        assert!(matches!(reaction, EventReaction::Nothing));

        let first = try_recv_worker_event(&engine).expect("job must produce a single event");
        assert!(
            matches!(first, WorkerEvent::ResourceStatsReady(ref rows, _) if rows.is_empty()),
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
            result: Ok(crate::worker::PullOutcome::Pulled {
                current_branch: None,
            }),
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
            result: Ok(crate::worker::PullOutcome::Pulled {
                current_branch: None,
            }),
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

    // ── Panic-safety: worktree-remove worker ─────────────────────────────

    /// A panicking worktree-remove worker must still post
    /// `WorktreeRemoveCompleted { result: Err(_) }` so the engine can clear
    /// `pending_deletions` and surface the failure. This test exercises the
    /// `catch_unwind` wrapper added to `begin_delete_session` by spawning an
    /// equivalent thread, triggering a deliberate panic, and asserting that the
    /// synthesised error event arrives on the channel and that
    #[test]
    fn the_deferred_removal_worker_honors_provenance_too() {
        // The async path (a live agent whose PTY must reap first) runs the
        // removal on a worker, so the gate has to travel with the request. The
        // synchronous and deferred paths must not disagree about whose branch
        // it is.
        let (mut engine, tmp) = test_engine();
        let repo = repo_with_branches(tmp.path(), &["develop"]);
        let worktree = attach_worktree(&repo, "develop");

        engine.dispatch_deferred_worktree_removal(crate::engine::DeferredWorktreeRemoval {
            session_id: "s1".to_string(),
            project_path: repo.to_string_lossy().to_string(),
            managed: crate::model::ManagedWorkspace {
                project_id: "p1".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: "develop".to_string(),
                initial_branch: "develop".to_string(),
                branch_provenance: crate::model::BranchProvenance::AttachedExisting,
                worktree_path: worktree.to_string_lossy().to_string(),
            },
            busy_message: "Removing worktree\u{2026}".to_string(),
        });

        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker must report back");
        match event {
            crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } => {
                assert_eq!(
                    result.unwrap(),
                    RemovedBranches::Kept(crate::model::BranchProvenance::AttachedExisting)
                );
            }
            _ => panic!("expected a WorktreeRemoveCompleted event"),
        }
        assert!(!worktree.exists(), "the worktree still goes");
        let branches = branch_list(&repo);
        assert!(
            branches.contains("develop"),
            "the pre-existing branch must survive the deferred removal: {branches}"
        );
    }

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
            let result = std::panic::catch_unwind(AssertUnwindSafe(
                || -> Result<crate::engine::RemovedBranches, String> {
                    panic!("simulated git failure");
                },
            ))
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
