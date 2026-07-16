//! The headless `Engine`: the single owner of dux's domain state. Surfaces (the
//! TUI `App` today, the web server later) embed/drive it. In E2 it is a passive
//! state container; domain operations and workers move into `Engine` methods in E3.

pub mod command;
mod companion;
pub mod config_saver;
mod events;
mod in_flight;
mod lifecycle;
mod resume_fallback;
mod spawn_worker;
pub mod status_op;

#[cfg(test)]
pub(crate) mod test_support;

pub use command::Command;
pub use config_saver::{ConfigSurface, NoopConfigSurface, ReloadCompletionGuard};
pub use events::{
    AgentLaunchFailedOutcome, AgentLaunchReadyOutcome, AgentLaunchReadyView,
    BeginDeleteSessionOutcome, BeginDeleteSessionView, DeleteTerminalView, DetachedSession,
    DispatchAgentLaunchView, DoDeleteSessionOutcome, DoDeleteSessionView, EventReaction,
    FinishDeleteSessionOutcome, FinishDeleteSessionView, ProjectPersistenceOutcome,
    ProjectPersistenceView, StatusUpdate, WorktreeRemoval,
};
pub use in_flight::{InFlightKey, InFlightSet, RenameExpectation};
pub use lifecycle::{
    DeferredWorktreeRemoval, GroupWorktreeRemoval, PrunedPty, PrunedPtyKind, ShutdownReport,
    TerminatingPty, format_shutdown_result, format_shutdown_start,
};
pub use resume_fallback::ResumeFallbackOutcome;
pub use spawn_worker::{
    BackgroundSpawn, BackgroundWorkerSpec, CommandWorkerSpec, LoopControl, LoopWorkerSpec,
    format_panic_payload,
};
pub use status_op::{Final, HandlerStatusOp, ResolvedFinal, StatusOp, status_op};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::{Config, DuxPaths, ProjectConfig};
use crate::config_queue::{ConfigWriteQueue, QuiesceGuard};
use crate::lockfile::SingleInstanceLock;
use crate::model::{
    AgentSession, AgentTab, ChangedFile, CompanionTerminal, GhStatus, PrInfo, Project,
    ProviderKind, SessionStatus,
};
use crate::pty::{ProgressReport, PtyClient};
use crate::storage::SessionStore;
use crate::worker::{
    BranchSyncEntry, PrSyncEntry, ProjectPersistenceAction, ResourceKind, ResourceTarget,
    WorkerEvent,
};

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
    /// Which surface currently owns this engine. Decides how a `terminal_identity`
    /// of `auto` resolves (mirror on the TUI, ghostty on the headless server). The
    /// in-process TUI to server flip flips this to `WebHeadless` before serving so
    /// agents launched under the server get the headless identity; PTYs already
    /// running keep their spawn-time env until they are relaunched.
    pub surface_kind: crate::term_identity::SurfaceKind,
    /// Snapshot of the identity-relevant environment dux inherited, taken once at
    /// construction so [`Engine::resolved_identity`] stays a pure function of it.
    pub host_env: crate::term_identity::HostEnvProbe,
    /// The resource monitor's sampler. It must outlive a single sample: sysinfo
    /// derives per-process CPU from the delta between two refreshes, so a
    /// collector rebuilt per sample reports 0% forever (see
    /// [`crate::resource_stats`]). `spawn_resource_stats_worker` spawns a thread
    /// per sample, hence the `Arc<Mutex<_>>`. The worker locks it for the walk,
    /// and the in-flight guard already serialises those samples.
    pub resource_collector: Arc<Mutex<crate::resource_stats::ResourceCollector>>,

    // Batch B fields
    pub worker_tx: Sender<WorkerEvent>,
    pub worker_rx: Receiver<WorkerEvent>,
    /// The single ordered, off-thread, atomic config writer for this process.
    /// `PersistGlobalEnv` / `UpdateMacros` (and, in later tasks, the other
    /// config-mutating handlers) write through this so saves never block the
    /// engine thread and never race each other. Its `Drop` sends an explicit
    /// `Shutdown` that the writer obeys even while paused, so a `QuiesceGuard`
    /// in `reload_guard` can outlive it without deadlocking — field declaration
    /// order relative to `reload_guard` no longer affects correctness.
    pub config_writer: ConfigWriteQueue,
    /// Front-end-specific config concerns the Engine cannot own itself: reload
    /// (validation + project-sync) and recover rendering. The TUI plugs in a
    /// `RuntimeBindings`-aware impl; the web a plain one.
    pub surface: Box<dyn ConfigSurface>,
    /// True while a `ReloadConfig` barrier is open (between `ReloadConfig`
    /// dispatch and the `ConfigReloadReady` it produces). While set, incoming
    /// config-mutating commands are deferred (see `deferred_commands`) so they
    /// re-apply against the reloaded config instead of racing it. Constructed as
    /// `false`; only the engine's reload handlers mutate it.
    pub reloading: bool,
    /// Config-mutating commands that arrived while `reloading` was set. Drained
    /// (re-applied) when the reload completes. Constructed empty.
    pub deferred_commands: Vec<Command>,
    /// Holds the config-writer quiesce barrier open for the lifetime of a
    /// reload. Dropped (resuming the writer) when `ConfigReloadReady` lands.
    /// Constructed as `None`.
    pub reload_guard: Option<QuiesceGuard>,
    pub providers: HashMap<String, PtyClient>,
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub running_provider_pins: HashMap<String, ProviderKind>,
    pub companion_terminals: HashMap<String, CompanionTerminal>,
    /// Persisted **extra tabs** (secondary provider tabs), keyed by tab id with
    /// the owning `session_id` carried in the value (mirrors `companion_terminals`
    /// so ownership resolves O(1) with no side index). The session-slot tab has no entry —
    /// it is derived from the `AgentSession` row (`tab_id == session_id`). Seeded
    /// from `session_store.load_agent_tabs()` at construction.
    pub agent_tabs: HashMap<String, AgentTab>,
    /// Agent/terminal PTYs that have been SIGTERMed on an individual delete or
    /// close and are being given a grace period to exit before they are
    /// force-killed (SIGKILL) — the non-blocking, per-PTY analogue of
    /// `shutdown_ptys`. They live here (not dropped from their maps) because
    /// `PtyClient::drop` hard-kills; `reap_terminating_ptys`, called each engine
    /// tick on both surfaces, drops them once they exit or their deadline passes.
    pub terminating_ptys: Vec<TerminatingPty>,
    /// Deferred worktree removals from multi-tab deletes, each waiting for a
    /// whole session's tab PTYs to reap before firing (see
    /// [`GroupWorktreeRemoval`]). `reap_terminating_ptys` drains these as their
    /// members reap.
    pub pending_group_removals: Vec<lifecycle::GroupWorktreeRemoval>,
    pub gh_status: GhStatus,
    pub pr_statuses: HashMap<String, PrInfo>,
    pub branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub pr_sync_enabled: Arc<AtomicBool>,
    /// Seconds between blind PR-sync safety polls, shared with the loop thread so
    /// a config reload can retune it live. `0` disables the blind poll (updates
    /// then come only from the refs watcher and foreground focus). Seeded from
    /// `config.ui.pr_poll_interval_seconds` at spawn and in `apply_reloaded_config`.
    pub pr_poll_interval_secs: Arc<AtomicU64>,
    /// Active PR-check backoff windows, keyed by GitHub host (`host -> until`).
    /// A host present with a future instant means every PR-check path (the
    /// batched safety poll AND the event-driven one-shot checks) skips that host
    /// until then, because its GraphQL quota ran low or `gh` is hard-failing.
    /// Per-host so one unreachable GitHub Enterprise host doesn't pause checks
    /// for a healthy github.com. Shared so both the loop and the one-shot checks
    /// read and update it.
    pub pr_backoff: Arc<Mutex<crate::gh::BackoffSnapshot>>,
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
    /// Session IDs whose worktree-removing delete has committed to tearing down
    /// but whose worktree has not yet been removed (the whole grace window from
    /// `begin_delete_session` through `WorktreeRemoveCompleted`). Unlike
    /// `pending_deletions` — which is only set once the async removal worker is
    /// actually dispatched (after the PTYs reap) — this is set synchronously the
    /// moment teardown begins, so `create_tab`/`launch_agent` can refuse to spawn a
    /// fresh provider into a worktree that is about to be removed.
    pub closing_sessions: HashSet<String>,
    /// Maps session IDs to the exact Busy message set by
    /// `begin_delete_session`. Used by the worker event handler to decide
    /// whether the current status-line content was set by this deletion (and
    /// should be cleared) or by an unrelated operation (and should be left
    /// alone). Cleared per-session when the worker event arrives.
    pub deletion_busy_messages: HashMap<String, String>,
    pub watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    /// The session id whose worktree is currently watched for changed files.
    /// Runtime state only (never config/persisted): paired with
    /// `watched_worktree` so the ViewModel can tell a client which session the
    /// global `staged_files`/`unstaged_files` belong to. A web client viewing a
    /// DIFFERENT session than this global watch shows a loading state rather than
    /// the wrong session's files (cross-tab safety). Written exclusively by
    /// [`Engine::set_watched_session`].
    pub watched_session_id: Option<String>,
    pub has_active_processes: Arc<AtomicBool>,
    /// The audience for statuses minted while processing the CURRENT command.
    /// Transient, never persisted: the single-threaded engine actor sets it to
    /// the originating connection's [`StatusScope`] before processing a web
    /// `ApplyWire` and resets it to [`StatusScope::All`] afterwards. The command
    /// mint sites (the synchronous outcome, `op.pending_status()`, `spawn_status_op`,
    /// and `spawn_command_worker`'s busy) stamp `scope = current_origin` so a web
    /// operation's toasts reach only that connection. Defaults to `All`, which is
    /// the TUI's permanent value (it never sets an origin), so TUI behaviour is
    /// unchanged.
    pub current_origin: crate::statusline::StatusScope,
    /// Set of currently-running operations. See `InFlightKey` for the
    /// allowed variants. Inserted by `mark_in_flight` before spawning a
    /// worker; cleared by `clear_in_flight` when the worker's completion
    /// event arrives.
    pub in_flight: InFlightSet,
    /// Expected branch names for in-flight intentional renames, keyed by
    /// session id. Set alongside `InFlightKey::BranchRename` at dispatch and
    /// cleared in `BranchRenameCompleted`. `BranchSyncReady` consults it so it
    /// only silently skips a mid-rename session when the observed branch is the
    /// still-pending old name or the expected new name; an *unexpected* value
    /// while a rename is in flight is logged rather than silently swallowed.
    pub rename_expected: HashMap<String, RenameExpectation>,
    /// Last-checked timestamps for the one-shot PR-check rate-limiter.
    /// Keyed by `session_id`; written by `process_worker_event`'s
    /// `PrStatusReady` arm and read by `spawn_pr_check_for_session` to
    /// skip checks made within the last 10 seconds.
    pub pr_last_checked: HashMap<String, Instant>,
    /// Guard so the long-lived `changed-files` poller is spawned at most once
    /// for the engine's life. Both `App::run` and the web engine-actor spawn
    /// the global workers, and the in-process TUI↔server flip hands the same
    /// engine (with its workers already running) to the other surface, which
    /// re-calls the spawn helpers. Without this guard a flip would start a
    /// second concurrent poller. `AtomicBool` so the `&self` spawn helper can
    /// flip it; never shared across threads (`Engine` is `!Send`).
    pub changed_files_poller_started: AtomicBool,
    /// Guard so the long-lived `branch-sync` poller is spawned at most once.
    /// Same rationale as `changed_files_poller_started`.
    pub branch_sync_worker_started: AtomicBool,
    /// Tracks when each agent's PTY last received data. A single poller
    /// ([`Engine::poll_pty_activity`]) consumes each provider's
    /// `take_received_data` flag once per tick and stamps `now` here; the
    /// streaming/"working" predicate ([`Engine::is_agent_streaming`]) reads it.
    /// Owned by the engine (not a surface) so both the TUI and the web actor
    /// project the same activity state, and because `take_received_data` is a
    /// consuming read that may have exactly one poller. The TUI and web actor
    /// never run simultaneously by construction, so single ownership is safe,
    /// and the in-process flip carries this map across automatically with the
    /// engine.
    pub pty_activity: HashMap<String, Instant>,
    /// Tracks when the user last forwarded keystrokes to each agent's PTY. The
    /// terminal echoes the user's own typing back as PTY output, which would
    /// otherwise read as the agent streaming ([`Engine::is_agent_streaming`])
    /// and falsely light the "working" indicator. Surfaces stamp this via
    /// [`Engine::note_pty_input`] when forwarding interactive input to an agent
    /// (never companion terminals — their output doesn't feed the agent's
    /// working state), and the predicate voids streaming while an entry is
    /// fresh. Engine-owned for the same reason as `pty_activity`: both the TUI
    /// and the web actor project identical working state, and the in-process
    /// flip carries the map across with the engine. Invariant: cleared wherever
    /// `pty_activity` is cleared (session teardown, detach, forced relaunch) so
    /// the two never drift; a new teardown path must drop both entries.
    pub pty_input: HashMap<String, Instant>,
    /// Tabs (keyed by tab id) that have raised a "needs attention" signal that
    /// has not yet been looked at. Memory-only runtime state, never persisted —
    /// like `working`/`has_output`, it does not survive a restart, by tenet.
    /// Populated by [`Engine::poll_agent_signals`] (which suppresses a signal on
    /// a tab the user is engaged with) and cleared when the user looks at or
    /// tears down the tab. The sidebar rolls this up across an agent's tabs.
    pub needs_attention: HashSet<String>,
    /// The most recent `OSC 9;4` progress report per tab (keyed by tab id), with
    /// the moment the engine observed it. [`Engine::is_agent_streaming`] treats a
    /// fresh report as authoritative for the "working" indicator, overriding the
    /// output-activity heuristic; a stale report (older than
    /// [`PROGRESS_AUTHORITY_WINDOW`]) is ignored and the heuristic resumes.
    /// Memory-only; dropped on tab teardown so a crashed agent can never leave a
    /// spinner stuck on.
    pub pty_progress: HashMap<String, ProgressReport>,
    /// Wall-clock timestamp of when the user was last actively looking at each
    /// tab (keyed by tab id): the TUI stamps the focused-interactive agent tab
    /// each tick; the web stamps a tab on PTY subscribe (opening its live view).
    /// A fresh entry (within [`ATTENTION_ENGAGED_WINDOW`]) both suppresses a new
    /// attention signal and clears an existing one, so an agent you are already
    /// looking at never nags you. Typing is handled separately via `pty_input`.
    pub agent_viewed: HashMap<String, Instant>,
    /// Wall-clock timestamp of the last companion-terminal foreground refresh.
    /// [`Engine::refresh_terminal_foregrounds`] throttles itself against this so
    /// callers can invoke it every tick while the actual `tcgetpgrp` probe runs
    /// at most once per [`FOREGROUND_REFRESH_INTERVAL`]. `None` until the first
    /// refresh runs. Wall-clock (not tick counts) per the design tenet.
    pub last_foreground_refresh: Option<Instant>,

    /// Web-side `HandlerStatusOp`s awaiting completion, keyed by the op's opaque
    /// id. These three ops run entirely server-side (the web actor drives them);
    /// the busy is emitted from `apply_wire` carrying the op's id, and the final
    /// is resolved when the operation's worker chain completes. The TUI drives the
    /// same worker chains with `status_op_id == None`, so these registries stay
    /// empty for it. The op is popped (consumed) exactly once at resolution.
    ///
    /// Checkout-default-branch: resolved in `process_worker_event`'s
    /// `NonDefaultBranchCheckoutCompleted` handler (both Ok and Err finals are
    /// produced there).
    pub pending_web_checkout_ops: HashMap<String, HandlerStatusOp<WebCheckoutOutcome>>,
    /// Add-project "Check Out & Add": SUCCESS is resolved in
    /// `drive_add_project_followup` (after the inline add); the switch FAILURE is
    /// resolved in `process_worker_event`'s `NonDefaultBranchCheckoutCompleted`
    /// Err handler. Mutually exclusive, so the op is consumed once.
    pub pending_web_add_project_ops: HashMap<String, HandlerStatusOp<WebAddProjectOutcome>>,
    /// New-agent-from-PR lookup: the SUCCESS handoff (the lookup resolved, the
    /// create dispatch's busy — keyed by the shared create op's opaque id — takes
    /// over) is resolved in `drive_pr_lookup_followup` as a `Final::Clear`; the
    /// lookup FAILURE is resolved in `process_worker_event`'s `PullRequestResolved`
    /// Err handler.
    pub pending_web_pr_lookup_ops: HashMap<String, HandlerStatusOp<WebPrLookupOutcome>>,
    /// Web-side async worktree-deletion ops (the "Removing worktree for agent …"
    /// busy). Keyed by **session id** (the completion `WorktreeRemoveCompleted`
    /// event carries `session_id`, so it is the natural correlation handle), not
    /// the op's opaque id. The busy is emitted from `drive_delete_followup`'s
    /// `AsyncStarted` branch carrying the op's id; the final is resolved in the
    /// same followup's `WorktreeRemoveSucceeded` / `WorktreeRemoveFailed` branches
    /// against a [`WebDeleteOutcome`]. The TUI drives the same worker chain but
    /// keeps its own op in the App layer, so this registry stays empty for it.
    pub pending_delete_ops_web: HashMap<String, HandlerStatusOp<WebDeleteOutcome>>,

    /// Create-agent ops (the "Creating a new agent…" busy and its progress
    /// re-emits). SHARED by both surfaces because the create busy is emitted
    /// engine-side via `spawn_command_worker` and its final wording is
    /// byte-identical on the TUI and the web. Keyed by the op's opaque id, which
    /// threads from the `DispatchCreateAgentRequest` dispatch through
    /// `CreateAgentRequest`/`AgentLaunchKind::Create.status_op_id` so it survives
    /// the worktree-creation → PTY-launch round trip and is still present on the
    /// `AgentLaunchReady`/`AgentLaunchFailed` completion. The op is resolved
    /// ENGINE-SIDE in `process_agent_launch_ready`/`process_agent_launch_failed`
    /// (and on `CreateAgentFailed`) against a [`CreateLaunchOutcome`], producing a
    /// keyed `Status` reaction returned alongside the View as a `Multi` — so
    /// whichever surface is running applies the same final. Progress re-emits via
    /// `op.progress(message)` without consuming the op.
    pub pending_create_ops: HashMap<String, HandlerStatusOp<CreateLaunchOutcome>>,

    /// Web-side reconnect / force-restart launch ops (the "Launching agent…" /
    /// "Starting fresh agent…" busy). The web counterpart to the TUI's
    /// `App.pending_reconnect_ops`: the TUI and web both resolve these from the
    /// `AgentLaunchReady`/`AgentLaunchFailed` View, but each on its OWN surface so
    /// the engine does not double-emit. Keyed by **session id** (the launch
    /// completion carries the session, the natural correlation handle). The busy
    /// is minted in `reconnect_session`; the final is resolved in
    /// `drive_web_launch_followup` against a [`WebLaunchOutcome`]. Empty for the
    /// TUI, which keeps its own op in the App layer.
    pub pending_web_launch_ops: HashMap<String, HandlerStatusOp<WebLaunchOutcome>>,

    /// The opaque create-op id minted by the MOST RECENT synchronous
    /// `DispatchCreateAgentRequest` dispatch within the current `apply_wire`
    /// call, surfaced to the caller as [`crate::wire::WireCommandOutcome::created_op_id`].
    /// `apply_wire` clears this to `None` before dispatching and reads (takes) it
    /// after, so the value reflects exactly this command's create — the engine
    /// actor is single-threaded, so there is no cross-command race. It lets a REST
    /// create handler correlate ITS exact new session via
    /// [`Engine::created_session_for_op`] instead of a racy "first id not in the
    /// pre-snapshot" set-difference (which could return a concurrent create's
    /// session). `None` for every non-create command and for the from-PR create
    /// (whose create op is minted later, inside the PR-lookup followup).
    pub last_created_op_id: Option<String>,

    /// Maps a create op's opaque id to the session it produced (and when), filled
    /// in the launch-ready Create branch once the worker-minted session lands. A
    /// REST create handler holding the op id (from `WireCommandOutcome.created_op_id`)
    /// resolves its exact session here. Bounded: pruned on every insert of entries
    /// past [`CREATED_SESSION_TTL`] or whose session no longer exists, so a
    /// long-running server cannot accumulate stale entries.
    pub created_session_by_op: HashMap<String, (String, Instant)>,
}

/// Handler-computed outcome for a create-agent op (see
/// [`Engine::pending_create_ops`]). The create launch resolves to one of these in
/// the engine's launch-ready / launch-failed handlers; the resolver (declared at
/// the `DispatchCreateAgentRequest` dispatch site) maps it to the final user
/// message, byte-identical to the pre-op wording on both surfaces.
pub enum CreateLaunchOutcome {
    /// The session was committed and the agent surface is ready. `status_message`
    /// is the create-kind success line.
    Committed { status_message: String },
    /// The session committed but its startup command failed; `branch_name` and
    /// `error` build the startup-failure line.
    StartupFailed { branch_name: String, error: String },
    /// `session_store.upsert_session` failed before the session could be
    /// committed; `error` is the persistence error.
    PersistFailed { error: String },
    /// The launch (or the create worker) failed; `message` is the already-formatted
    /// error line.
    Failed { message: String },
}

/// Handler-computed outcome for a web reconnect / force-restart launch op (see
/// [`Engine::pending_web_launch_ops`]). Mirrors the TUI's reconnect outcome; the
/// resolver maps it to the final user message, byte-identical to the web's
/// pre-op `wire_statuses_from_reaction` wording.
pub enum WebLaunchOutcome {
    /// Reconnect / force-reconnect succeeded; `status_message` is the success line.
    Ready { status_message: String },
    /// Reconnect failed; `branch_name`/`message` build the reconnect-failure line.
    ReconnectFailed {
        branch_name: String,
        message: String,
    },
    /// Force-restart failed; `branch_name`/`message` build the fresh-restart line.
    ForceReconnectFailed {
        branch_name: String,
        message: String,
    },
    /// The session vanished between dispatch and launch; the busy is cleared with
    /// no replacement message.
    Missing,
}

/// Handler-computed outcome for a web async worktree-deletion op (see
/// [`Engine::pending_delete_ops_web`]). The completion event knows whether the
/// git removal succeeded; the followup additionally observes whether the session
/// record is still present (driving the FinishDeleteSession cascade vs the
/// already-gone fallback). The resolver (declared at dispatch) maps this to the
/// final user message, byte-identical to the pre-op web wording.
pub enum WebDeleteOutcome {
    /// Git removal succeeded and the session was still present — the
    /// `FinishDeleteSession` cascade ran and produced this status message.
    Succeeded { message: String },
    /// Git removal succeeded but the session was already gone (e.g. its project
    /// was removed) before the worker reported back.
    SucceededGone,
    /// Git removal failed; `message` is the git error.
    Failed { message: String },
    /// Git removal succeeded but the post-removal `FinishDeleteSession` cascade
    /// failed; `message` is the formatted error.
    CleanupFailed { message: String },
}

/// Handler-computed outcome for the web checkout-project-default-branch op. The
/// final message is built by the op's resolver from this plus the project name
/// captured at dispatch. Covers every terminal path of the two-worker chain: the
/// inspection (worker 1) can short-circuit with already-leading / heuristic /
/// inspect-failed before any switch runs, and the switch (worker 2) finishes
/// with success / failure.
pub enum WebCheckoutOutcome {
    /// The `git switch` (worker 2) succeeded onto `target_branch`.
    Ok { target_branch: String },
    /// The `git switch` (worker 2) failed; `repo_path` is the source checkout path.
    Failed {
        target_branch: String,
        repo_path: String,
    },
    /// Worker 1 found the project already on its leading branch; no switch ran.
    AlreadyLeading { current_branch: String },
    /// Worker 1 could only heuristically guess the default branch, so it refused.
    Heuristic { current_branch: String },
    /// Worker 1's inspection itself failed.
    InspectFailed { error: String },
}

/// Handler-computed outcome for the web add-project "Check Out & Add" op.
pub enum WebAddProjectOutcome {
    /// The switch and the inline project-add both succeeded; `status_message` is
    /// the combined "Checked out X and added project Y" line.
    Added { status_message: String },
    /// The `git switch` failed before the add ran.
    SwitchFailed {
        target_branch: String,
        repo_path: String,
    },
    /// The switch succeeded but the inline add was rolled back; `message` is the
    /// already-formatted failure line.
    AddFailed { message: String },
}

/// Handler-computed outcome for the web new-agent-from-PR lookup op.
pub enum WebPrLookupOutcome {
    /// The lookup resolved and the create dispatch took over (its busy, keyed by
    /// the shared create op's opaque id, now owns the spinner), so this op's busy
    /// is cleared with no message.
    HandedOff,
    /// The lookup failed; `message` is the already-formatted error line.
    Failed { message: String },
}

/// How recently an agent must have emitted PTY output to count as actively
/// streaming ("working"). Shared by the TUI spinner and the web ViewModel so
/// both surfaces use an identical window.
///
/// This is a *hysteresis* window, not a precise timestamp exposure: the
/// `working` boolean stays stable while an agent streams steadily, so the
/// change-only ViewModel watch channel only pushes on transitions (idle→working
/// and working→idle), never on every byte. See [`crate::viewmodel::SessionView`].
pub const AGENT_STREAMING_WINDOW: Duration = Duration::from_secs(1);

/// How long after the user forwards keystrokes to an agent's PTY the
/// streaming/"working" indicator stays suppressed. The terminal echoes the
/// user's own typing straight back as PTY output, so without this the act of
/// typing reads as the agent producing output (see [`AGENT_STREAMING_WINDOW`]).
/// The window is slightly longer than the output hysteresis so the trailing
/// echo of the last keystroke fully ages out before the indicator can return;
/// genuine agent output continuing past it re-lights the indicator on the next
/// tick. Shared by the TUI spinner and the web ViewModel through
/// [`Engine::is_agent_streaming`].
pub const AGENT_INPUT_SUPPRESSION_WINDOW: Duration = Duration::from_millis(1250);

/// How long an `OSC 9;4` progress report stays authoritative for the "working"
/// indicator before [`Engine::is_agent_streaming`] falls back to the
/// output-activity heuristic. Agents that emit progress (e.g. Claude Code)
/// stream it many times a second while busy, so any real gap this long means the
/// report channel has gone quiet and the heuristic should take over. This also
/// bounds how long a crashed agent's last "working" report can linger before the
/// spinner settles. Wall-clock, per the design tenet.
pub const PROGRESS_AUTHORITY_WINDOW: Duration = Duration::from_secs(10);

/// How long after the user last looked at a tab (TUI focus, web PTY subscribe)
/// an attention signal on that tab stays suppressed/cleared. Must comfortably
/// exceed a surface's tick cadence so continuously viewing a tab keeps its
/// attention flag from ever rising. Typing suppression is separate
/// ([`AGENT_INPUT_SUPPRESSION_WINDOW`]). Wall-clock, per the design tenet.
pub const ATTENTION_ENGAGED_WINDOW: Duration = Duration::from_secs(3);

/// Whether the user is currently "engaged" with a tab and so should not be
/// nagged about it: they either looked at it within [`ATTENTION_ENGAGED_WINDOW`]
/// (`viewed`) or typed into it within [`AGENT_INPUT_SUPPRESSION_WINDOW`]
/// (`typed`). A free function so [`Engine::poll_agent_signals`] can call it while
/// holding a disjoint mutable borrow of `needs_attention`, and so the windowing
/// is unit-testable without a live PTY.
fn attention_engaged(
    viewed: &HashMap<String, Instant>,
    typed: &HashMap<String, Instant>,
    tab_id: &str,
    now: Instant,
) -> bool {
    viewed
        .get(tab_id)
        .is_some_and(|t| now.duration_since(*t) < ATTENTION_ENGAGED_WINDOW)
        || typed
            .get(tab_id)
            .is_some_and(|t| now.duration_since(*t) < AGENT_INPUT_SUPPRESSION_WINDOW)
}

/// The pure per-tick attention decision, factored out of
/// [`Engine::poll_agent_signals`] so it is testable without a live PTY. Given the
/// tabs whose attention signal fired this tick (`fired`, already gated by the
/// `attention_on_bell` preference at drain time), whether the feature is enabled,
/// and an `engaged` predicate, it updates `needs_attention` in place:
///
/// - Feature off: clear everything (never accumulate; drop anything left over
///   from before a runtime toggle).
/// - Otherwise: clear the flag on any tab the user is now engaged with, then set
///   it for each freshly-fired tab the user is NOT engaged with. Each outcome is
///   logged at debug level so the whole pipeline is diagnosable from `dux.log`
///   without adding default-level noise.
fn apply_attention_decision(
    needs_attention: &mut HashSet<String>,
    fired: &[String],
    enabled: bool,
    engaged: impl Fn(&str) -> bool,
) {
    if !enabled {
        needs_attention.clear();
        return;
    }
    // Looking at (or typing into) a tab clears its flag.
    needs_attention.retain(|tab| !engaged(tab));
    // A newly-fired signal sets the flag unless the user is engaged with it.
    for tab in fired {
        if engaged(tab) {
            crate::logger::debug(&format!(
                "attention signal for tab {tab} suppressed: user is engaged"
            ));
        } else {
            crate::logger::debug(&format!("attention flagged for tab {tab}"));
            needs_attention.insert(tab.clone());
        }
    }
}

/// How often [`Engine::refresh_terminal_foregrounds`] actually probes companion
/// terminals for their foreground command. Calls more frequent than this are
/// no-ops, so every surface can invoke the refresh once per (sub-second) tick
/// and still get the same ~2s cadence. Wall-clock, not tick counts, per the
/// "periodic refreshes use wall-clock time" design tenet.
pub const FOREGROUND_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long an entry in [`Engine::created_session_by_op`] stays addressable by a
/// REST create handler before it is pruned. Comfortably longer than the longest
/// create-await window (the from-PR path waits up to 60s) so a slow create still
/// resolves, but short enough that the map self-trims on a long-running server.
pub const CREATED_SESSION_TTL: Duration = Duration::from_secs(120);

/// Minimum spacing between per-session PR checks for the background triggers
/// (refs watcher, agent exit). Guards against a burst of triggers spawning
/// concurrent `gh` calls for the same session.
pub const PR_CHECK_MIN_INTERVAL: Duration = Duration::from_secs(10);

/// Tighter debounce for foreground-focus PR checks (switching to / activating an
/// agent in the TUI, opening its PTY on the web) so a freshly-focused agent shows
/// current data even if no branch-change event fired recently, without letting
/// focus-thrash hammer `gh`.
pub const PR_FOREGROUND_DEBOUNCE: Duration = Duration::from_secs(3);

/// Correlation key for the "GitHub API quota low / recovered" status so the
/// warning and its eventual recovery/clear replace the same entry.
const PR_QUOTA_STATUS_KEY: &str = "pr-quota";

/// How long to pause a host's PR checks after a hard `gh` failure (spawn error,
/// timeout, or an unparseable response) — the case the quota-number backoff can't
/// see. Short, since a transient network/`gh` error usually clears quickly.
const PR_HARD_FAILURE_BACKOFF_SECS: u64 = 60;

/// How long to pause a host's PR checks when GitHub is rate-limiting us but we
/// have no `resetAt` to time it precisely. Longer than a transient error, since a
/// (secondary) rate limit takes a while to clear and re-hitting it just extends it.
const PR_RATE_LIMIT_BACKOFF_SECS: u64 = 300;

/// The PR-sync loop sleeps in slices of this length so a disable or an interval
/// change is observed within a few seconds rather than after a full (up to
/// multi-hour) interval elapses.
const PR_SYNC_SLICE_SECS: u64 = 3;

/// Rewrite an absolute path under the user's home directory to the portable
/// `$HOME/...` form so config.toml stays machine-independent (the tenet:
/// "Project config is portable desired state"). Paths outside `$HOME`, or when
/// the home directory cannot be resolved, are returned unchanged. `expand_path`
/// is the inverse applied on load. Mirrors the TUI's `portable_project_path` so
/// both surfaces write identical config regardless of which one added the project.
pub(crate) fn portable_project_path(path: &str) -> String {
    let Some(home) = home::home_dir() else {
        return path.to_string();
    };
    match std::path::Path::new(path).strip_prefix(&home) {
        Ok(relative) => {
            let relative = relative.to_string_lossy();
            if relative.is_empty() {
                "$HOME".to_string()
            } else {
                format!("$HOME/{relative}")
            }
        }
        Err(_) => path.to_string(),
    }
}

/// Map a runtime [`Project`] to a portable [`ProjectConfig`] for config.toml.
/// Uses the same field mapping as the persistence worker's `Add` arm so the
/// on-disk shape stays consistent regardless of which path wrote it. The path is
/// stored in the portable `$HOME/...` form (via [`portable_project_path`]) so the
/// config does not pin an absolute, machine-specific path.
fn project_to_project_config(p: &Project) -> ProjectConfig {
    ProjectConfig {
        id: p.id.clone(),
        path: portable_project_path(&p.path),
        name: Some(p.name.clone()),
        default_provider: p
            .explicit_default_provider
            .as_ref()
            .map(|pk| pk.as_str().to_string()),
        leading_branch: p.leading_branch.clone(),
        auto_reopen_agents: p.auto_reopen_agents,
        startup_command: p.startup_command.clone(),
        env: p.env.clone(),
    }
}

impl Engine {
    /// Mark an operation as in-flight. Returns `true` if it was newly
    /// inserted, `false` if it was already present.
    pub fn mark_in_flight(&mut self, key: InFlightKey) -> bool {
        self.in_flight.insert(key)
    }

    /// Clear an in-flight key after a worker's completion event arrives.
    pub fn clear_in_flight(&mut self, key: &InFlightKey) {
        self.in_flight.remove(key);
    }

    /// Record that the create op `op_id` produced session `session_id` (stamped
    /// now), so a REST create handler holding the op id (returned in
    /// `WireCommandOutcome.created_op_id`) can resolve ITS exact session via
    /// [`Engine::created_session_for_op`] rather than a racy set-difference.
    /// Prunes on insert: entries past [`CREATED_SESSION_TTL`] or whose session no
    /// longer exists are dropped, so the map stays bounded on a long-running
    /// server.
    pub fn record_created_session(&mut self, op_id: String, session_id: String) {
        let now = Instant::now();
        // Bind disjoint field borrows so the retain closure can read `sessions`
        // while the map is borrowed mutably.
        let sessions = &self.sessions;
        let map = &mut self.created_session_by_op;
        map.retain(|_, (sid, at)| {
            now.saturating_duration_since(*at) < CREATED_SESSION_TTL
                && sessions.iter().any(|s| &s.id == sid)
        });
        map.insert(op_id, (session_id, now));
    }

    /// The session id produced by create op `op_id`, if it has landed and is still
    /// within [`CREATED_SESSION_TTL`]. `None` while the create is still in flight
    /// or after the entry has expired.
    pub fn created_session_for_op(&self, op_id: &str) -> Option<String> {
        let now = Instant::now();
        self.created_session_by_op.get(op_id).and_then(|(sid, at)| {
            (now.saturating_duration_since(*at) < CREATED_SESSION_TTL).then(|| sid.clone())
        })
    }

    /// Whether `cmd`'s handler must be deferred while a `ReloadConfig` barrier is
    /// open, so it re-applies against the freshly-reloaded config rather than
    /// racing it.
    ///
    /// `PersistGlobalEnv` / `UpdateMacros` write `config.toml` directly through
    /// the engine's config writer. `PersistProject` / `RemoveProject` write
    /// SQLite first and only mirror the change into `config.toml` afterward (via
    /// `persist_projects_to_config`, wired up in Task 6); deferring them is still
    /// correct so that mirror runs against the reloaded project set rather than a
    /// stale one. `ReloadConfig` / `RecoverConfig` drive the barrier themselves
    /// and are deliberately excluded. Provider/theme/pane-width saves are surface
    /// (TUI App) handlers that currently write `config.toml` directly (not through
    /// `Engine::config_writer`), so they are NOT covered by this deferral nor by
    /// the writer's quiesce backstop; routing them through the writer is later-task
    /// work, and until then a save from those paths during a reload is unguarded.
    fn is_config_mutating(cmd: &Command) -> bool {
        matches!(
            cmd,
            Command::PersistGlobalEnv { .. }
                | Command::UpdateMacros { .. }
                | Command::PersistProject { .. }
                | Command::RemoveProject { .. }
        )
    }

    /// Poll each PTY provider for recent data and update the per-agent activity
    /// timestamp used by the streaming/"working" indicator. `take_received_data`
    /// is a consuming read (and suppresses the post-resize redraw burst), so
    /// this must be the only poll site — both the TUI run loop and the web
    /// engine actor call this exactly once per tick, and they never run at the
    /// same time.
    pub fn poll_pty_activity(&mut self) {
        let now = Instant::now();
        // `providers` (and therefore `pty_activity`) is keyed by TAB id, not
        // session id — the session-slot tab's id equals its session id, but an
        // extra tab's does not.
        for (tab_id, provider) in &self.providers {
            if provider.take_received_data() {
                self.pty_activity.insert(tab_id.clone(), now);
            }
        }
    }

    /// Refresh the `foreground_cmd` of every companion terminal by probing its
    /// PTY for the currently-running foreground process (`tcgetpgrp` vs the
    /// shell PID — see [`crate::pty::PtyClient::foreground_process_name`]).
    /// Throttled internally by wall-clock: the probe runs at most once per
    /// [`FOREGROUND_REFRESH_INTERVAL`], so callers may invoke this every tick
    /// and any extra calls within the interval are cheap no-ops. Both the TUI
    /// run loop and the web engine actor call this once per tick; they never run
    /// at the same time.
    ///
    /// Returns `true` only when this call actually probed AND a terminal's
    /// `foreground_cmd` changed (the spine's `foreground_cmd` therefore moved).
    /// A throttled no-op, or a probe that found every foreground unchanged,
    /// returns `false`. The web engine actor uses this to bump its spine-change
    /// version only on a real change; the TUI ignores the result.
    pub fn refresh_terminal_foregrounds(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_foreground_refresh
            && now.duration_since(last) < FOREGROUND_REFRESH_INTERVAL
        {
            return false;
        }
        self.last_foreground_refresh = Some(now);
        let mut changed = false;
        for terminal in self.companion_terminals.values_mut() {
            let next = terminal.client.foreground_process_name();
            if next != terminal.foreground_cmd {
                terminal.foreground_cmd = next;
                changed = true;
            }
        }
        changed
    }

    /// Record that the user just forwarded interactive keystrokes to the given
    /// agent's PTY. [`Engine::is_agent_streaming`] treats such input as voiding
    /// the streaming indicator for [`AGENT_INPUT_SUPPRESSION_WINDOW`] so the
    /// terminal echo of the user's own typing isn't mistaken for the agent
    /// working. Stamp this only for agent PTYs, never companion terminals, and
    /// never for programmatic writes (macros, startup commands) — those should
    /// keep showing the agent as working.
    pub fn note_pty_input(&mut self, tab_id: &str) {
        self.pty_input.insert(tab_id.to_string(), Instant::now());
    }

    /// Returns `true` if the given tab received PTY data within
    /// [`AGENT_STREAMING_WINDOW`], indicating it is actively streaming output —
    /// unless the user forwarded keystrokes to it within
    /// [`AGENT_INPUT_SUPPRESSION_WINDOW`], in which case the recent output is
    /// assumed to be the echo of that typing and the indicator is voided.
    /// Keyed by TAB id (see `poll_pty_activity`), not session id.
    pub fn is_agent_streaming(&self, tab_id: &str) -> bool {
        // A fresh OSC 9;4 progress report is authoritative: the agent is stating
        // its own busy/idle status, which we trust over the output-activity
        // heuristic (and over typing suppression — a progress report is the
        // agent's own signal, not a terminal echo of the user's keystrokes). A
        // stale report is ignored and the heuristic below resumes, so a crashed
        // agent that stopped reporting can never leave the spinner stuck on.
        if let Some(report) = self.pty_progress.get(tab_id)
            && report.at.elapsed() < PROGRESS_AUTHORITY_WINDOW
        {
            return report.working;
        }
        let streaming = self
            .pty_activity
            .get(tab_id)
            .is_some_and(|t| t.elapsed() < AGENT_STREAMING_WINDOW);
        if !streaming {
            return false;
        }
        let typing = self
            .pty_input
            .get(tab_id)
            .is_some_and(|t| t.elapsed() < AGENT_INPUT_SUPPRESSION_WINDOW);
        !typing
    }

    /// The terminal identity dux should apply when launching an agent, resolved
    /// from the configured mode, the owning surface, and the inherited-env probe.
    /// Companion terminals reuse it too so a plain shell sees the same identity.
    pub fn resolved_identity(&self) -> crate::term_identity::TerminalIdentity {
        let mode = crate::term_identity::TerminalIdentityMode::from_config_str(
            &self.config.capabilities.terminal_identity,
        );
        crate::term_identity::resolve_identity(mode, self.surface_kind, &self.host_env)
    }

    /// Poll every provider for attention and progress signals once per tick,
    /// mirroring [`Engine::poll_pty_activity`]. This is the single drain site for
    /// the consuming per-tab attention flag (both the TUI run loop and the web
    /// engine actor call it exactly once per tick, and never at the same time).
    ///
    /// - Progress reports are captured unconditionally (they feed the working
    ///   indicator regardless of the attention preference).
    /// - Attention signals are drained every call so a suppressed one never
    ///   lingers, but only set the flag when `attention_indicator` is on AND the
    ///   user is not currently engaged with that tab (looking at it or typing
    ///   into it). Looking at or typing into a tab also clears an existing flag.
    pub fn poll_agent_signals(&mut self) {
        let now = Instant::now();
        let enabled = self.config.ui.attention_indicator;
        let count_bell = self.config.ui.attention_on_bell;

        // Collect first: iterating `providers` borrows it immutably, so the
        // per-tab map mutations below must happen after the loop.
        let mut progress_updates: Vec<(String, ProgressReport)> = Vec::new();
        let mut fired: Vec<String> = Vec::new();
        for (tab_id, provider) in &self.providers {
            if let Some(report) = provider.progress_report() {
                progress_updates.push((tab_id.clone(), report));
            }
            // Always drain (consume) the flag so a toggle can't replay a stale
            // signal; only record the hit while the feature is enabled.
            if provider.take_attention(count_bell) && enabled {
                fired.push(tab_id.clone());
            }
        }

        for (tab_id, report) in progress_updates {
            self.pty_progress.insert(tab_id, report);
        }

        // Direct field borrows (not a `self` method) so the disjoint mutable
        // borrow of `needs_attention` below is allowed alongside these reads.
        let viewed = &self.agent_viewed;
        let typed = &self.pty_input;
        let engaged = |tab: &str| attention_engaged(viewed, typed, tab, now);
        apply_attention_decision(&mut self.needs_attention, &fired, enabled, engaged);
    }

    /// Drain every provider's captured passthrough ring and return the bytes to
    /// forward to the host terminal, honoring the capability gates. Called once per
    /// tick by the TUI (the web bridges these client-side instead).
    ///
    /// Every ring is drained on every call regardless of the gates ("always drain,
    /// discard when gated") so toggling a switch never replays a stale backlog and
    /// the rings stay bounded. Gating: the `passthrough` master switch drops
    /// everything when off; a `ClipboardSet` is additionally gated by
    /// `clipboard_passthrough` (`focused` forwards only the tab the user is viewing,
    /// `always` forwards any, `off` never); notifications and progress forward from
    /// every tab so a background agent can still raise a desktop notification. When
    /// `wrap_for_tmux` the canonical bytes are re-wrapped in a tmux passthrough
    /// envelope so they survive the tmux dux itself runs under.
    pub fn take_host_passthrough(
        &mut self,
        focused_tab: Option<&str>,
        wrap_for_tmux: bool,
    ) -> Vec<u8> {
        let master = self.config.capabilities.passthrough;
        // Parse without warning on the per-tick path: an unrecognized value is
        // surfaced once at config load/reload (see `ClipboardPassthroughMode`), and
        // falls back to the default here.
        let clipboard_mode = crate::config::ClipboardPassthroughMode::parse(
            &self.config.capabilities.clipboard_passthrough,
        )
        .unwrap_or(crate::config::ClipboardPassthroughMode::Focused);
        let mut out = Vec::new();
        for (tab_id, provider) in &self.providers {
            // Always drain (keeps the ring bounded even for a headless server).
            let seqs = provider.take_passthrough();
            if !master {
                continue;
            }
            for seq in seqs {
                let forward = match seq.kind {
                    crate::attention::CapturedKind::ClipboardSet => match clipboard_mode {
                        crate::config::ClipboardPassthroughMode::Always => true,
                        crate::config::ClipboardPassthroughMode::Off => false,
                        crate::config::ClipboardPassthroughMode::Focused => {
                            focused_tab == Some(tab_id.as_str())
                        }
                    },
                    // Notifications and progress forward from every tab.
                    _ => true,
                };
                if !forward {
                    continue;
                }
                if wrap_for_tmux {
                    out.extend_from_slice(&crate::attention::tmux_wrap(&seq.bytes));
                } else {
                    out.extend_from_slice(&seq.bytes);
                }
            }
        }
        out
    }

    /// Drain and discard every provider's captured passthrough ring without
    /// forwarding anything. Called on a surface flip (TUI to serve, or serve back
    /// to TUI) so the newly-active surface starts from a clean slate: capture stays
    /// always-on across the flip (so nothing is missed mid-flip), but the backlog
    /// that accumulated while the other surface owned the host is dropped rather
    /// than replayed to a terminal that never saw the original context.
    pub fn discard_passthrough_backlog(&mut self) {
        for provider in self.providers.values() {
            let _ = provider.take_passthrough();
        }
    }

    /// Whether the dux process is running under tmux, per the single host-env
    /// probe. This is the one tmux predicate: both the terminal-identity resolver
    /// (see through tmux to the outer terminal) and the TUI's passthrough wrap
    /// decision read it, so they can never disagree. `TMUX` set-but-empty does not
    /// count; an inherited `TERM_PROGRAM=tmux` does.
    pub fn host_under_tmux(&self) -> bool {
        self.host_env.under_tmux()
    }

    /// Record that the user is actively looking at the given tab's live view
    /// (TUI: the focused, interactive agent tab; web: a PTY subscribe). This both
    /// clears an existing attention flag immediately and suppresses a new one for
    /// [`ATTENTION_ENGAGED_WINDOW`] so an agent you are watching never nags you.
    pub fn note_agent_viewed(&mut self, tab_id: &str) {
        self.agent_viewed.insert(tab_id.to_string(), Instant::now());
        self.needs_attention.remove(tab_id);
    }

    /// Like [`Engine::note_agent_viewed`], but only when `tab_id` resolves to a
    /// real agent tab. This is the entry point for surface transports that take an
    /// unvalidated id from a client (the web PTY-subscribe and viewed-ping paths):
    /// gating here, in core, keeps `agent_viewed` from accumulating entries for
    /// stale deep links, retries, or bogus ids, and keeps the check in one place
    /// rather than reimplemented per surface.
    pub fn note_agent_viewed_if_known(&mut self, tab_id: &str) {
        if self.owning_session_for_tab(tab_id).is_some() {
            self.note_agent_viewed(tab_id);
        }
    }

    /// Whether the given tab currently has an unacknowledged attention signal.
    /// Keyed by TAB id; the sidebar rolls this up across an agent's tabs.
    pub fn tab_needs_attention(&self, tab_id: &str) -> bool {
        self.needs_attention.contains(tab_id)
    }

    /// Whether ANY tab of the session (session-slot or extra) currently needs
    /// attention. The any-tab rollup the sidebar row uses, mirroring how
    /// `working` rolls up. Cheap: `needs_attention` is usually empty, so this
    /// short-circuits without scanning tabs.
    pub fn session_needs_attention(&self, session_id: &str) -> bool {
        if self.needs_attention.is_empty() {
            return false;
        }
        if self.needs_attention.contains(session_id) {
            return true;
        }
        self.agent_tabs
            .values()
            .any(|t| t.session_id == session_id && self.needs_attention.contains(&t.id))
    }

    /// True if the given key is currently marked in-flight.
    pub fn is_in_flight(&self, key: &InFlightKey) -> bool {
        self.in_flight.contains(key)
    }

    pub fn spawn_project_persistence(
        &mut self,
        action: ProjectPersistenceAction,
        status_op_id: Option<String>,
    ) {
        let db_path = self.paths.sessions_db_path.clone();
        let action_for_panic = action.clone();
        let status_op_id_for_panic = status_op_id.clone();
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "project-persistence".into(),
                in_flight_key: None,
                panic_event: Some(Box::new(move |reason| {
                    WorkerEvent::ProjectPersistenceCompleted {
                        action: action_for_panic,
                        result: Err(format!("Project-persistence worker panicked: {reason}")),
                        status_op_id: status_op_id_for_panic,
                    }
                })),
            },
            move |tx| {
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
                let _ = tx.send(WorkerEvent::ProjectPersistenceCompleted {
                    action,
                    result,
                    status_op_id,
                });
            },
        );
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

    /// Rebuild config.toml's `[[projects]]` from the current runtime projects and
    /// persist via the shared writer. Surfaces (web) call this after a project
    /// persistence so the portable config stays in sync with SQLite. (The TUI has
    /// its own config-sync path.) Eager synchronous write via the queue; blocks
    /// until the writer confirms or times out.
    pub fn persist_projects_to_config(&mut self) -> anyhow::Result<()> {
        self.config.projects = self
            .projects
            .iter()
            .map(project_to_project_config)
            .collect();
        self.config_writer
            .save_eager(self.config.clone())
            .map_err(|e| anyhow::anyhow!(e))
    }

    /// Apply a freshly-reloaded config to the running engine (headless subset of
    /// the TUI's apply): refresh GitHub-integration flag, re-merge projects from
    /// the store under the new config, swap the config, and refresh derived
    /// project/branch-sync state. View concerns (theme, keybindings, panes) are
    /// the surface's responsibility and are not touched here.
    pub fn apply_reloaded_config(&mut self, config: Config) -> anyhow::Result<()> {
        self.github_integration_enabled = config.ui.github_integration;
        self.projects = crate::project_browser::load_projects(
            &self.session_store.load_projects()?,
            &self.session_store.load_project_created_ats()?,
            &config,
        );
        self.config = config;
        // Retune the live PR-sync poll interval (the loop reads this each tick).
        self.pr_poll_interval_secs.store(
            u64::from(crate::config::normalized_pr_poll_interval(
                self.config.ui.pr_poll_interval_seconds,
            )),
            Ordering::Relaxed,
        );
        self.refresh_project_defaults();
        self.update_branch_sync_sessions();
        Ok(())
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
        // Idempotent: a long-lived poller must never be duplicated. The flip
        // hands a live engine to the other surface, which re-calls this; a
        // second call is a no-op. `swap` is the atomic test-and-set, placed
        // after the disabled check so the flag means "a poller thread is live".
        if self
            .branch_sync_worker_started
            .swap(true, Ordering::Relaxed)
        {
            return;
        }
        let interval = Duration::from_secs(u64::from(interval_secs));
        let sessions = Arc::clone(&self.branch_sync_sessions);
        self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "branch-sync".into(),
            },
            move |tx| {
                thread::sleep(interval);
                let snapshot = match sessions.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => return LoopControl::Continue,
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
                    return LoopControl::Break; // receiver dropped, app is shutting down
                }
                LoopControl::Continue
            },
        );
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

    /// Whether the new-agent-from-PR flow is available: GitHub integration is
    /// enabled in config AND the `gh` CLI is installed and authenticated. Mirrors
    /// the TUI's `github_pr_agent_command_available`. Surfaced on the ViewModel
    /// (`gh_available`) so the web dialog can hide/disable the PR mode rather
    /// than letting the user submit a command that the server will reject.
    pub fn pr_agent_command_available(&self) -> bool {
        self.github_integration_enabled
            && matches!(self.gh_status, crate::model::GhStatus::Available)
    }

    pub fn spawn_gh_status_check(&mut self) {
        // Not guarded against re-spawn: this is a one-shot job (check PATH +
        // auth, post one `GhStatusChecked` event, exit). Re-running it on a
        // flip-back is harmless and desirable — a fresh check picks up any
        // `gh login` the user did while the other surface was active.
        if !self.github_integration_enabled {
            return;
        }
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "gh-status-check".into(),
                in_flight_key: None,
                panic_event: Some(Box::new(|_reason| {
                    // Fall back to `NotInstalled` on panic so the UI does not
                    // sit in an indeterminate state — the worst case is a
                    // harmless "gh CLI not found" message.
                    WorkerEvent::GhStatusChecked(crate::model::GhStatus::NotInstalled)
                })),
            },
            move |tx| {
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
                // Step 2: Is `gh` authenticated? Bounded so a wedged credential
                // helper can't park the boot probe indefinitely.
                let authed = matches!(
                    crate::gh::run_gh_with_timeout(&["auth", "status"], crate::gh::GH_CALL_TIMEOUT),
                    crate::gh::GhCallOutcome::Completed(o) if o.status.success()
                );
                if !authed {
                    crate::logger::info("[gh-integration] gh CLI found but not authenticated");
                    let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotAuthenticated));
                    return;
                }
                crate::logger::info("[gh-integration] gh CLI available and authenticated");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::Available));
            },
        );
    }

    /// Point the changed-files watch at a session's worktree, or clear it. This
    /// is the CHEAP half (no git): it only resolves the session and updates the
    /// watch state, returning the worktree to compute changed files for (if any).
    ///
    /// It is the engine half of the TUI's `App::reload_changed_files`: it sets
    /// `watched_worktree` (which the background poller reads every 2–10s) and
    /// `watched_session_id`, then ALWAYS empties the staged/unstaged lists so the
    /// pane never shows the PREVIOUS watch's files between this call and the
    /// compute landing (preserving the `watched_session_id` cross-tab invariant).
    /// The web layer calls this when a browser selects a session (the TUI never
    /// set it for the web, which is why the web changed-files pane stayed empty).
    ///
    /// IMPORTANT: the actual changed-files compute (`git::changed_files`) must NOT
    /// be done on the engine actor thread — it shells out to several git
    /// subprocesses and would freeze every web client on a slow repo / git-lock
    /// stall. The web path follows this call with `spawn_changed_files_refresh`
    /// (off-thread); the single-user TUI computes inline on its own App thread.
    ///
    /// - `None` (or an UNKNOWN id) → clear the watch and the lists, return `None`.
    /// - `Some(id)` for a known session → watch its worktree, record the id, empty
    ///   the lists, and return `Some(worktree)` to compute changed files for.
    #[must_use]
    pub fn set_watched_session(&mut self, session_id: Option<&str>) -> Option<PathBuf> {
        let resolved = session_id.and_then(|id| {
            self.sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| (id.to_string(), PathBuf::from(&s.worktree_path)))
        });
        let worktree = resolved.as_ref().map(|(_, path)| path.clone());
        // Keep the background poller in sync with the watched worktree.
        if let Ok(mut guard) = self.watched_worktree.lock() {
            *guard = worktree.clone();
        }
        self.watched_session_id = resolved.map(|(id, _)| id);
        // Always clear so the pane shows "no changes yet" (never the previous
        // watch's stale files) until the off-thread/inline compute lands.
        self.staged_files = Vec::new();
        self.unstaged_files = Vec::new();
        worktree
    }

    /// Compute the changed files for `worktree` OFF the engine actor thread and
    /// post them back as a `ChangedFilesReady` event. The one-shot worker mirrors
    /// `spawn_pr_check_for_session`'s spawn shape and the changed-files poller's
    /// git call. The event carries the `worktree` it was computed for, so the
    /// `ChangedFilesReady` drain in `process_worker_event` automatically drops a
    /// result whose watch has since moved (the 4faf872 stale-poll guard).
    ///
    /// A `git::changed_files` error yields empty lists (matching the TUI's
    /// `unwrap_or_default`) so the pane resolves to "no changes" rather than
    /// hanging. The web layer calls this right after `set_watched_session`; the
    /// TUI does NOT (it computes inline — single user, single thread).
    pub fn spawn_changed_files_refresh(&self, worktree: PathBuf) {
        let label = format!("changed-files-refresh:{}", worktree.display());
        self.spawn_loop_worker(LoopWorkerSpec { label }, move |tx| {
            let (staged, unstaged) = crate::git::changed_files(&worktree).unwrap_or_default();
            let _ = tx.send(WorkerEvent::ChangedFilesReady {
                staged,
                unstaged,
                worktree: worktree.clone(),
            });
            // One-shot: compute once and stop. The drain side is race-safe
            // (it path-checks `worktree` against the live watch).
            LoopControl::Break
        });
    }

    pub fn spawn_changed_files_poller(&self) {
        // Idempotent: a long-lived poller must never be duplicated. The flip
        // hands a live engine to the other surface, which re-calls this; a
        // second call is a no-op. `swap` is the atomic test-and-set.
        if self
            .changed_files_poller_started
            .swap(true, Ordering::Relaxed)
        {
            return;
        }
        let watched = Arc::clone(&self.watched_worktree);
        let has_agent = Arc::clone(&self.has_active_processes);
        self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "changed-files-poller".into(),
            },
            move |tx| {
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
                        .send(WorkerEvent::ChangedFilesReady {
                            staged,
                            unstaged,
                            // Tag the event with the worktree it was computed for so a
                            // poll that finished after the watch moved gets dropped
                            // instead of clobbering the current session's files.
                            worktree: worktree_path.clone(),
                        })
                        .is_err()
                {
                    return LoopControl::Break; // receiver dropped, app is shutting down
                }
                LoopControl::Continue
            },
        );
    }

    pub fn spawn_browser_entries(&mut self, dir: &Path) {
        let dir = dir.to_path_buf();
        let dir_for_panic = dir.clone();
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: format!("browser-entries:{}", dir.display()),
                in_flight_key: None,
                panic_event: Some(Box::new(move |_reason| {
                    // Synthesise an empty entries list so the browser prompt
                    // exits its loading state rather than spinning forever.
                    WorkerEvent::BrowserEntriesReady {
                        dir: dir_for_panic,
                        entries: Vec::new(),
                    }
                })),
            },
            move |tx| {
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
            },
        );
    }

    pub fn spawn_project_worktrees_worker(
        &mut self,
        project: Project,
        status_op_id: Option<String>,
    ) {
        let paths = self.paths.clone();
        let sessions = self.sessions.clone();
        let project_id_for_panic = project.id.clone();
        let status_op_id_for_panic = status_op_id.clone();
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: format!("project-worktrees:{}", project.id),
                in_flight_key: None,
                panic_event: Some(Box::new(move |reason| WorkerEvent::ProjectWorktreesReady {
                    project_id: project_id_for_panic,
                    result: Err(format!("Project-worktrees worker panicked: {reason}")),
                    status_op_id: status_op_id_for_panic,
                })),
            },
            move |tx| {
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
                    status_op_id,
                });
            },
        );
    }

    pub fn spawn_project_branch_status_checks(&mut self) {
        // Not guarded against re-spawn: each project's check is a one-shot
        // background job (post per-branch events, exit). Re-running on a
        // flip-back is harmless and desirable — a fresh check reflects branch
        // movement that happened while the other surface was active.
        //
        // Snapshot the project list before iterating: `spawn_background_worker`
        // takes `&mut self`, so we cannot hold a borrow of `self.projects`
        // across the per-project spawn calls.
        let projects: Vec<Project> = self
            .projects
            .iter()
            .filter(|project| !project.path_missing)
            .cloned()
            .collect();
        for project in projects {
            let label = format!("project-branch-status:{}", project.id);
            self.spawn_background_worker(
                BackgroundWorkerSpec {
                    label,
                    in_flight_key: None,
                    // `run_project_branch_status_job` posts per-branch events
                    // internally and has no single completion event we could
                    // synthesise on panic. Log-only is the right policy.
                    panic_event: None,
                },
                move |tx| {
                    crate::project_browser::run_project_branch_status_job(project, tx);
                },
            );
        }
    }

    // -- GitHub PR integration workers --

    pub fn spawn_pr_sync_worker(&self) {
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let enabled = Arc::clone(&self.pr_sync_enabled);
        let interval_secs = Arc::clone(&self.pr_poll_interval_secs);
        // Seed the shared interval from config so the first iteration honors it,
        // and signal the worker is running BEFORE spawning so the kill switch
        // observes the live state on the first iteration.
        interval_secs.store(
            u64::from(crate::config::normalized_pr_poll_interval(
                self.config.ui.pr_poll_interval_seconds,
            )),
            Ordering::Relaxed,
        );
        enabled.store(true, Ordering::Relaxed);
        let backoff = Arc::clone(&self.pr_backoff);
        self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "pr-sync".into(),
            },
            move |tx| {
                let secs = interval_secs.load(Ordering::Relaxed);
                // Sleep in short slices so a disable (`enabled=false`) or a
                // retuned interval is observed within a few seconds rather than
                // after a full (up to multi-hour) interval elapses. `0` = the
                // blind poll is disabled; we still nap (events drive updates).
                let nap = if secs == 0 { 60 } else { secs };
                let mut slept = 0u64;
                while slept < nap {
                    let slice = PR_SYNC_SLICE_SECS.min(nap - slept);
                    thread::sleep(Duration::from_secs(slice));
                    slept += slice;
                    if !enabled.load(Ordering::Relaxed) {
                        return LoopControl::Break;
                    }
                    if interval_secs.load(Ordering::Relaxed) != secs {
                        // Interval retuned (incl. 0<->N) — restart the wait.
                        return LoopControl::Continue;
                    }
                }
                if secs == 0 {
                    return LoopControl::Continue;
                }
                // Backed-off hosts are skipped inside run_pr_sync via this
                // snapshot (their sessions keep last-known PRs), so no global
                // pause is needed here.
                let snapshot = backoff.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let (results, signals) = crate::gh::run_pr_sync(&sessions, &snapshot);
                Self::apply_pr_backoff(&backoff, &signals, tx);
                if !results.is_empty() && tx.send(WorkerEvent::PrStatusReady(results)).is_err() {
                    return LoopControl::Break;
                }
                LoopControl::Continue
            },
        );
    }

    /// Update the shared per-host PR-check backoff from a sync's per-host signals
    /// and surface a keyed, per-host status. Rate-limiting (approaching the points
    /// limit, or a 403/secondary limit) and plain network/`gh` errors each pause
    /// that host with an appropriately-worded, INFO-toned notice that auto-clears
    /// (the pause is temporary and self-resolving, so it must not sit on screen as
    /// a stuck warning); a healthy signal clears the host's pause silently. The
    /// notice fires once per pause window (`already_active`). Only queried hosts
    /// appear in `signals`, so a skipped/backed-off host is never spuriously
    /// touched.
    fn apply_pr_backoff(
        shared: &Arc<Mutex<crate::gh::BackoffSnapshot>>,
        signals: &[crate::gh::HostSignal],
        tx: &Sender<WorkerEvent>,
    ) {
        for sig in signals {
            let key = format!("{PR_QUOTA_STATUS_KEY}:{}", sig.host);
            // Decide (pause-window, message). Priority: approaching the GraphQL
            // points limit (we know the reset time) → GitHub rate-limiting us (a
            // 403/secondary limit, checked independently so a healthy host can't
            // mask it) → a plain network/`gh` error → healthy (clear).
            let decision: Option<(Instant, String)> = if let Some(r) = sig
                .rate
                .as_ref()
                .filter(|r| r.remaining < crate::gh::RATE_LIMIT_BACKOFF_FLOOR)
            {
                let secs_until = r
                    .reset_at
                    .map(|t| (t - Utc::now()).num_seconds().clamp(0, 3600) as u64)
                    .unwrap_or(PR_RATE_LIMIT_BACKOFF_SECS);
                let when = r
                    .reset_at
                    .map(|t| {
                        format!(
                            " around {}",
                            t.with_timezone(&chrono::Local).format("%H:%M")
                        )
                    })
                    .unwrap_or_default();
                Some((
                    Instant::now() + Duration::from_secs(secs_until),
                    format!(
                        "GitHub's API rate limit for {} is nearly used up ({} points left). dux \
                         paused PR status checks; they resume automatically{when}.",
                        sig.host, r.remaining,
                    ),
                ))
            } else if sig.rate_limited {
                Some((
                    Instant::now() + Duration::from_secs(PR_RATE_LIMIT_BACKOFF_SECS),
                    format!(
                        "GitHub is rate-limiting API requests on {}. dux paused PR status checks; \
                         they resume automatically once the limit clears.",
                        sig.host,
                    ),
                ))
            } else if sig.hard_failed {
                Some((
                    Instant::now() + Duration::from_secs(PR_HARD_FAILURE_BACKOFF_SECS),
                    format!(
                        "dux couldn't reach GitHub for PR status on {} (network or `gh` error); \
                         it will retry shortly.",
                        sig.host,
                    ),
                ))
            } else {
                None
            };

            match decision {
                Some((until, message)) => {
                    let already_active = {
                        let mut map = shared.lock().unwrap_or_else(|e| e.into_inner());
                        let already = map.get(&sig.host).is_some_and(|u| Instant::now() < *u);
                        map.insert(sig.host.clone(), until);
                        already
                    };
                    // Info-toned (not a persistent warning) so it self-dismisses:
                    // the pause is temporary and resolves on its own, so the notice
                    // auto-clears instead of sitting on screen. Emitted once per
                    // pause window (the `already_active` gate), keyed per host.
                    if !already_active {
                        let _ = tx.send(WorkerEvent::CommandWorkerStarted(StatusUpdate::keyed(
                            key,
                            crate::statusline::StatusTone::Info,
                            message,
                        )));
                    }
                }
                None => {
                    // Host is healthy again: clear its backoff so it is queried
                    // normally. No "resumed" message — the Info-toned pause notice
                    // already auto-cleared, so a fresh toast now would be stale.
                    let mut map = shared.lock().unwrap_or_else(|e| e.into_inner());
                    map.remove(&sig.host);
                }
            }
        }
    }

    pub fn spawn_initial_pr_refresh(&mut self) {
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let backoff = Arc::clone(&self.pr_backoff);
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "initial-pr-refresh".into(),
                in_flight_key: None,
                // A panic here has no completion event to synthesize; the next
                // poll cycle re-attempts regardless. `apply_pr_backoff` below
                // already surfaces call failures as a keyed warning. Log-only.
                panic_event: None,
            },
            move |tx| {
                let snapshot = backoff.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let (results, signals) = crate::gh::run_pr_sync(&sessions, &snapshot);
                Self::apply_pr_backoff(&backoff, &signals, &tx);
                if !results.is_empty() {
                    let _ = tx.send(WorkerEvent::PrStatusReady(results));
                }
            },
        );
    }

    /// Gather the process trees the resource monitor should report on: every
    /// live agent tab and companion terminal. The worker aggregates the full
    /// process tree under each root pid.
    ///
    /// Each target carries the spine id it was resolved from (a tab id, or a
    /// terminal id) so a surface can join a sampled row back to the row it
    /// renders. `label` is for display only and must never be parsed back into
    /// its parts: a title containing `): ` would break the parse, and two agents
    /// may share a title.
    pub fn resource_monitor_targets(&self) -> Vec<ResourceTarget> {
        let mut targets = Vec::new();
        // `providers` is keyed by tab id (session-slot tab == session id). Iterate it so
        // every live tab's process is a target, not just one per session.
        for (tab_id, pty) in &self.providers {
            let Some(pid) = pty.child_process_id() else {
                continue;
            };
            if let Some(session) = self.sessions.iter().find(|s| s.id == *tab_id) {
                // session-slot tab.
                let title = session.title.as_deref().unwrap_or(&session.branch_name);
                let provider = self.running_provider_for(session);
                targets.push(ResourceTarget {
                    id: tab_id.clone(),
                    kind: ResourceKind::Agent,
                    label: format!("Agent ({}): {title}", provider.as_str()),
                    pid,
                });
            } else if let Some(tab) = self.agent_tabs.get(tab_id) {
                // extra tab: resolve its owning session for a readable label.
                let title = self
                    .sessions
                    .iter()
                    .find(|s| s.id == tab.session_id)
                    .map(|s| s.title.as_deref().unwrap_or(&s.branch_name).to_string())
                    .unwrap_or_else(|| tab.session_id.clone());
                let provider = self
                    .running_provider_pins
                    .get(tab_id)
                    .cloned()
                    .unwrap_or_else(|| tab.provider.clone());
                targets.push(ResourceTarget {
                    id: tab_id.clone(),
                    kind: ResourceKind::Agent,
                    label: format!("Tab ({}): {title}", provider.as_str()),
                    pid,
                });
            }
        }
        for (terminal_id, terminal) in &self.companion_terminals {
            if let Some(pid) = terminal.client.child_process_id() {
                let label = match &terminal.foreground_cmd {
                    Some(cmd) => format!("Terminal ({cmd}): {}", terminal.label),
                    None => format!("Terminal: {}", terminal.label),
                };
                targets.push(ResourceTarget {
                    id: terminal_id.clone(),
                    kind: ResourceKind::Terminal,
                    label,
                    pid,
                });
            }
        }
        targets
    }

    pub fn spawn_resource_stats_worker(&mut self) {
        // The resource monitor refreshes itself periodically; an
        // already-in-flight refresh is a silent skip rather than a
        // user-visible warning, so we short-circuit before invoking the
        // primitive's already-running path.
        if self.is_in_flight(&InFlightKey::ResourceStats) {
            return;
        }
        let targets = self.resource_monitor_targets();
        // Hand the worker the long-lived collector rather than a fresh one: the
        // CPU reading is a delta against the previous sample's refresh.
        let collector = Arc::clone(&self.resource_collector);
        let reaction = self.spawn_command_worker(
            CommandWorkerSpec {
                label: "resource-stats".into(),
                in_flight_key: Some(InFlightKey::ResourceStats),
                busy_status: None,
                already_running_status: None,
                panic_event: Some(Box::new(|_reason| {
                    // No error variant exists for resource stats; an empty
                    // refresh is the most defensible signal — the in-flight
                    // key clears and the next refresh runs normally.
                    WorkerEvent::ResourceStatsReady(Vec::new(), false)
                })),
            },
            move |tx| {
                // Poison-tolerant: the collector is a plain sampler whose only
                // state is sysinfo's process snapshot, re-established by the very
                // next refresh, so recovering the guard beats propagating a panic
                // into every later sample.
                let mut collector = collector.lock().unwrap_or_else(|e| e.into_inner());
                let (rows, was_baseline) = collector.sample(targets);
                let _ = tx.send(WorkerEvent::ResourceStatsReady(rows, was_baseline));
            },
        );
        // Historical signature is `&mut self` → `()`. The primitive returns
        // `EventReaction::Nothing` on the happy path. Forward the rare
        // synchronous spawn failure through the worker channel so the
        // status line still surfaces it via the existing
        // `CommandWorkerStarted` handler.
        if let EventReaction::Status(status) = reaction {
            let _ = self
                .worker_tx
                .send(WorkerEvent::CommandWorkerStarted(status));
        }
    }

    /// Trigger a foreground PR check (agent brought to the foreground): the same
    /// one-shot check as [`Self::spawn_pr_check_for_session`] but with the tighter
    /// [`PR_FOREGROUND_DEBOUNCE`] window, so focusing an agent shows fresh data.
    pub fn spawn_foreground_pr_check(&mut self, session_id: &str) {
        self.spawn_pr_check_for_session(session_id, PR_FOREGROUND_DEBOUNCE);
    }

    /// Trigger a one-shot PR check for a single session, unless it was checked
    /// more recently than `min_interval` ago. Background triggers (refs watcher,
    /// agent exit) pass [`PR_CHECK_MIN_INTERVAL`]; foreground focus passes the
    /// tighter [`PR_FOREGROUND_DEBOUNCE`] via [`Self::spawn_foreground_pr_check`].
    ///
    /// The timestamp is recorded BEFORE the worker thread is spawned so a burst
    /// of triggers within a single event-loop tick — e.g. several callers each
    /// invoking this for the same session before the first worker's
    /// `PrStatusReady` event has been processed — does not bypass the
    /// rate-limit and spawn N concurrent `gh` subprocesses.
    pub fn spawn_pr_check_for_session(&mut self, session_id: &str, min_interval: Duration) {
        if !self.github_integration_enabled
            || !matches!(self.gh_status, crate::model::GhStatus::Available)
        {
            return;
        }
        // Don't stack concurrent gh subprocesses for the same session: a call can
        // run up to GH_CALL_TIMEOUT, which exceeds the debounce, so guard on an
        // in-flight check before the debounce stamp (a skipped call must not push
        // the debounce forward). Backed-off hosts are skipped inside the sync
        // itself (per-host), so no host check is needed here.
        if self.is_in_flight(&InFlightKey::PrCheck(session_id.to_string())) {
            return;
        }
        // Rate-limit: skip if checked more recently than `min_interval` ago.
        if let Some(last) = self.pr_last_checked.get(session_id)
            && last.elapsed() < min_interval
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
        let label = format!("pr-check:{}", entry.session_id);
        let backoff = Arc::clone(&self.pr_backoff);
        let abort_sid = entry.session_id.clone();
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label,
                in_flight_key: Some(InFlightKey::PrCheck(entry.session_id.clone())),
                // On panic, clear the in-flight key without wiping the badge (a
                // synthesized `PrStatusReady(None)` would). The next trigger
                // re-attempts.
                panic_event: Some(Box::new(move |_reason| {
                    WorkerEvent::PrCheckAborted(abort_sid.clone())
                })),
            },
            move |tx| {
                let snapshot = backoff.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let (result, signals) = crate::gh::check_pr_for_entry(&entry, &snapshot);
                // Event-driven checks feed the shared backoff too, so a sustained
                // failure arms the pause (and clears it on recovery) even when the
                // blind poll is disabled.
                Self::apply_pr_backoff(&backoff, &signals, &tx);
                let _ = tx.send(WorkerEvent::PrStatusReady(vec![(entry.session_id, result)]));
            },
        );
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
            if let Err(err) = self.session_store.upsert_session(session) {
                crate::logger::error(&format!(
                    "failed to persist session status update for {}: {err}",
                    session.id,
                ));
            }
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
            if let Err(err) = self.session_store.upsert_session(session) {
                crate::logger::error(&format!(
                    "failed to persist session desired_running for {}: {err}",
                    session.id,
                ));
            }
        } else if let Err(err) = self.session_store.set_desired_running(session_id, desired) {
            crate::logger::error(&format!(
                "failed to persist desired_running override for {session_id}: {err}",
            ));
        }
    }

    /// Record that `provider` has launched in this session's worktree. Takes the
    /// launched provider explicitly (rather than reading `session.provider`) so a
    /// extra tab that ran a *different* provider than the session default still
    /// records directory-scoped resume state under the provider that actually ran.
    pub fn mark_session_provider_started(&mut self, session_id: &str, provider: &ProviderKind) {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        else {
            return;
        };

        if !session.mark_provider_started(provider) {
            return;
        }

        session.updated_at = Utc::now();
        if let Err(err) = self.session_store.upsert_session(session) {
            crate::logger::error(&format!(
                "failed to persist provider_started state for {}: {err}",
                session.id,
            ));
        }
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

    /// Roll back the optimistic state that a rename call site set up before
    /// dispatching the branch-rename worker, for the rare case where the
    /// worker never started (a synchronous thread-spawn failure). Normally
    /// `BranchRenameCompleted` clears this state and reverts the title on both
    /// success and failure, but that event only fires if the worker actually
    /// ran — so on a spawn failure the caller must unwind here or the Busy
    /// hangs forever, `rename_expected` is orphaned, and the optimistic title
    /// is never reverted (permanently deferring drift detection). Removes the
    /// expected-branch stash, clears the in-flight marker, and restores
    /// `previous_title`. Idempotent.
    pub fn revert_optimistic_rename(&mut self, session_id: &str, previous_title: Option<String>) {
        self.rename_expected.remove(session_id);
        self.clear_in_flight(&InFlightKey::BranchRename(session_id.to_string()));
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.title = previous_title;
            session.updated_at = Utc::now();
            if let Err(err) = self.session_store.upsert_session(session) {
                crate::logger::error(&format!(
                    "failed to persist rename revert for {session_id}: {err}"
                ));
            }
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

    /// The status message shown when an existing agent's provider becomes ready
    /// after a reconnect. Shared by the TUI and the web so both report the SAME
    /// completion message (1:1) instead of a frontend echoing its own
    /// "attaching…" placeholder back to the user. `resume` is the result of
    /// [`should_resume_session`]. Callers may append extra context (e.g. a
    /// detached-worktree note) to the returned string.
    pub fn agent_reconnect_status_message(&self, session: &AgentSession, resume: bool) -> String {
        let proj_name = self.project_name_for_session(session);
        if resume {
            format!(
                "Resumed {} agent \"{}\" in project \"{}\".",
                session.provider.as_str(),
                session.branch_name,
                proj_name
            )
        } else {
            format!(
                "Started fresh {} session for agent \"{}\" in project \"{}\". Use /sessions inside the agent to restore a prior conversation.",
                session.provider.as_str(),
                session.branch_name,
                proj_name
            )
        }
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
        self.should_resume_provider(session, &session.provider)
    }

    /// Whether a tab launching `session` with `provider` may resume that
    /// provider's prior conversation in the worktree: the provider must support
    /// resume and must have started here before. Resume history is per-provider
    /// (each CLI keeps its own conversation in the directory), so eligibility is
    /// judged against `provider`, not `session.provider`.
    pub fn should_resume_provider(&self, session: &AgentSession, provider: &ProviderKind) -> bool {
        let cfg = crate::config::provider_config(&self.config, provider);
        cfg.supports_session_resume() && session.has_started_provider(provider)
    }

    /// The provider whose *live* conversation a tab currently owns, for
    /// resume-collision purposes. A retarget-while-running tab keeps owning its
    /// pinned (still-running) provider until it exits; otherwise it owns its
    /// configured provider — the session-slot tab's is `session.provider`, an
    /// extra tab's is its `agent_tabs` row provider.
    pub fn tab_running_provider(&self, session: &AgentSession, tab_id: &str) -> ProviderKind {
        if let Some(pinned) = self.running_provider_pins.get(tab_id) {
            return pinned.clone();
        }
        if tab_id == session.id {
            return session.provider.clone();
        }
        self.agent_tabs
            .get(tab_id)
            .map(|t| t.provider.clone())
            .unwrap_or_else(|| session.provider.clone())
    }

    /// The single source of truth for "does this tab launch resume?". Resume is
    /// per-provider: it holds only when the caller requested it, the effective
    /// `provider` is resume-eligible for this session, and no OTHER live or
    /// launching tab of the agent currently owns that same provider's
    /// conversation. A claude tab and an opencode tab of one agent can therefore
    /// both resume; only two tabs of the SAME provider collide. The
    /// single-threaded engine makes this atomic against concurrent launches: the
    /// in-flight `AgentLaunch` key is marked synchronously at dispatch, so two
    /// same-provider tabs can never both observe an empty slot and both resume.
    pub fn tab_resume_decision(
        &self,
        session: &AgentSession,
        tab_id: &str,
        provider: &ProviderKind,
        requested: bool,
    ) -> bool {
        if !requested || !self.should_resume_provider(session, provider) {
            return false;
        }
        let others_same_provider = self.tab_ids_for_session(&session.id).into_iter().any(|id| {
            id != tab_id
                && (self.providers.contains_key(&id)
                    || self.is_in_flight(&InFlightKey::AgentLaunch(id.clone())))
                && self.tab_running_provider(session, &id) == *provider
        });
        !others_same_provider
    }

    /// Every runtime-map key owned by a session: its session-slot tab (== session id) plus
    /// every extra tab id. The single source of truth for teardown fan-out — a
    /// full-session teardown must clear all of these, not just the session id.
    pub fn tab_ids_for_session(&self, session_id: &str) -> Vec<String> {
        let mut ids = vec![session_id.to_string()];
        ids.extend(
            self.agent_tabs
                .values()
                .filter(|t| t.session_id == session_id)
                .map(|t| t.id.clone()),
        );
        ids
    }

    /// True if ANY tab of the session currently has a live provider PTY or an
    /// in-flight launch. Since no tab is privileged, this is what "the agent is
    /// still running" means: the session-slot row stays Active until its LAST
    /// tab is gone, and it drives resume liveness (whoever comes up alone
    /// resumes; everyone launched alongside a live/launching sibling is fresh).
    pub fn any_tab_active(&self, session_id: &str) -> bool {
        self.tab_ids_for_session(session_id).into_iter().any(|id| {
            self.providers.contains_key(&id)
                || self.is_in_flight(&InFlightKey::AgentLaunch(id.clone()))
        })
    }

    /// The first LIVE tab of a session, in display order: the session-slot tab
    /// first, then extra tabs ordered by `(sort_order, created_at)` (the same
    /// order the TUI's tab strip renders). A tab counts as live when it has a
    /// provider PTY or an in-flight `AgentLaunch`. Returns `None` when every
    /// tab is dormant, so callers know to fall back to the session-slot tab
    /// rather than land on a dormant tab that would relaunch on the next
    /// activation. Kept in core (not the TUI) because the liveness predicate
    /// must stay identical to `any_tab_active`'s.
    pub fn first_live_tab(&self, session_id: &str) -> Option<String> {
        let mut extras: Vec<&AgentTab> = self
            .agent_tabs
            .values()
            .filter(|t| t.session_id == session_id)
            .collect();
        extras.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        std::iter::once(session_id.to_string())
            .chain(extras.into_iter().map(|t| t.id.clone()))
            .find(|id| {
                self.providers.contains_key(id)
                    || self.is_in_flight(&InFlightKey::AgentLaunch(id.clone()))
            })
    }

    /// Resolve a tab id back to the session that owns it. A Main-tab id resolves
    /// to itself (it has no `agent_tabs` row); a Support-tab id resolves via the
    /// map. Returns `None` for an unknown id.
    pub fn owning_session_for_tab(&self, tab_id: &str) -> Option<String> {
        if let Some(tab) = self.agent_tabs.get(tab_id) {
            return Some(tab.session_id.clone());
        }
        self.sessions
            .iter()
            .any(|s| s.id == tab_id)
            .then(|| tab_id.to_string())
    }

    /// Swap which provider (CLI) an agent session uses on its NEXT launch.
    ///
    /// This is the engine half of the TUI's `apply_change_agent_provider`. It
    /// does NOT kill or relaunch a running agent: it changes the persisted
    /// provider so the next launch (reconnect) uses it, and, when a provider is
    /// still running on the session's PTY, pins the previously-running provider
    /// so UI labels keep telling the truth until the user exits and relaunches.
    ///
    /// Returns the data each surface needs to format its own status message
    /// (the TUI references a rebindable keybinding label; the web does not), so
    /// message wording stays surface-side. An unknown session is an error; the
    /// caller is responsible for the no-op "already uses this provider" case,
    /// since only the surface knows the session's display label for that copy.
    pub fn change_agent_provider(
        &mut self,
        session_id: &str,
        provider: ProviderKind,
    ) -> anyhow::Result<ChangeAgentProviderOutcome> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;

        let running = self.providers.contains_key(session_id);
        let previous = self.sessions[index].provider.clone();

        let session = &mut self.sessions[index];
        session.provider = provider.clone();
        session.updated_at = Utc::now();
        let updated = session.clone();
        self.session_store.upsert_session(&updated)?;

        // Pin the still-running provider so UI labels stay truthful until the
        // user exits and relaunches the agent. Only set on the first
        // swap-while-running — later swaps don't change what's spawned.
        if running {
            self.running_provider_pins
                .entry(session_id.to_string())
                .or_insert_with(|| previous.clone());
        }

        let resume_available = self.tab_resume_decision(&updated, &updated.id, &provider, true);

        Ok(ChangeAgentProviderOutcome {
            previous,
            running,
            resume_available,
        })
    }

    /// The effective per-agent tab cap (clamped, default-substituted).
    pub fn agent_tabs_max(&self) -> u16 {
        crate::config::normalized_agent_tabs_max(self.config.ui.agent_tabs_max)
    }

    /// Create a new extra tab for `session_id` running `provider`, persist its
    /// row, and dispatch a FRESH launch (extra tabs never resume). Returns the
    /// new tab id synchronously; the spawn itself is asynchronous — a spawn
    /// failure lands in `process_agent_launch_failed`'s `Tab` arm, which removes
    /// this just-created row (it is `is_fresh`). The per-agent cap is enforced
    /// here, in one synchronous call (the single-threaded engine makes the
    /// check-then-insert atomic); it counts the session-slot tab.
    pub fn create_tab(
        &mut self,
        session_id: &str,
        provider: ProviderKind,
        pty_size: (u16, u16),
    ) -> anyhow::Result<String> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;

        // Refuse if this agent is mid-deletion: its worktree is about to be
        // removed, and spawning a fresh provider into it would race
        // `git::remove_worktree` (cwd-deleted-under-fork / git-lock).
        if self.closing_sessions.contains(session_id) {
            anyhow::bail!("this agent is being deleted; cannot start a new tab");
        }

        if !self
            .config
            .providers
            .commands
            .contains_key(provider.as_str())
        {
            anyhow::bail!("provider \"{}\" is not configured", provider.as_str());
        }

        // Per-agent cap (the `+ 1` accounts for the session-slot tab, which has no row).
        let max_per_agent = i64::from(self.agent_tabs_max());
        if self.session_store.count_agent_tabs(session_id)? + 1 >= max_per_agent {
            anyhow::bail!("this agent already has the maximum of {max_per_agent} tabs",);
        }

        let tab_id = uuid::Uuid::new_v4().to_string();
        let sort_order = self
            .session_store
            .max_tab_sort_order(session_id)?
            .unwrap_or(0)
            + 1;
        let tab = crate::model::AgentTab {
            id: tab_id.clone(),
            session_id: session_id.to_string(),
            provider: provider.clone(),
            sort_order,
            created_at: Utc::now(),
        };
        self.session_store.insert_agent_tab(&tab)?;
        self.agent_tabs.insert(tab_id.clone(), tab);

        let status_message = format!("Started a fresh {} tab.", provider.as_str());
        let request = self.build_tab_launch_request(
            tab_id.clone(),
            Some(provider),
            session,
            false,
            pty_size,
            crate::worker::AgentLaunchKind::Tab {
                is_fresh: true,
                status_message,
            },
        );
        // The launch itself runs on a worker (ready/failed arrives later), but the
        // dispatch is synchronous. If the worker thread fails to even start (e.g.
        // near an OS thread limit), no `WorkerEvent` is ever posted, so
        // `process_agent_launch_failed`'s `Tab` cleanup never runs — leaving this
        // just-inserted row a permanent ghost. Detect that synchronous failure and
        // clean up the fresh row here so the caller gets a real error instead.
        let reaction = self.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        });
        let dispatch_failed = match &reaction {
            Ok(EventReaction::DispatchAgentLaunchView(view)) => !view.launched,
            Err(_) => true,
            Ok(_) => false,
        };
        if dispatch_failed {
            // Persist-first: only drop the in-memory entry once the row is
            // actually gone, so a failed DB delete leaves a visible/closeable tab
            // rather than an invisible ghost that still consumes a cap slot
            // (mirrors `close_tab` and `process_agent_launch_failed`'s Tab arm).
            match self.session_store.delete_agent_tab(&tab_id) {
                Ok(()) => {
                    self.agent_tabs.remove(&tab_id);
                }
                Err(err) => crate::logger::error(&format!(
                    "failed to delete ghost extra tab {tab_id}: {err}",
                )),
            }
            let message = match reaction {
                Ok(EventReaction::DispatchAgentLaunchView(view)) => view
                    .status
                    .map(|s| s.message)
                    .unwrap_or_else(|| "the tab could not be launched".to_string()),
                Err(err) => err.to_string(),
                Ok(_) => "the tab could not be launched".to_string(),
            };
            anyhow::bail!("could not start tab: {message}");
        }

        Ok(tab_id)
    }

    /// Close an extra tab: delete its row first (so a persistence failure leaves
    /// in-memory state untouched), then gracefully tear down its PTY and clear all
    /// of its runtime-map entries. The session-slot tab's close is a separate path
    /// (`KillSessionPty`) — this is only for extra tabs (`tab_id != session_id`).
    pub fn close_tab(&mut self, session_id: &str, tab_id: &str) -> anyhow::Result<()> {
        if tab_id == session_id {
            anyhow::bail!("the session-slot tab cannot be closed as an extra tab");
        }
        match self.agent_tabs.get(tab_id) {
            Some(tab) if tab.session_id == session_id => {}
            Some(_) => anyhow::bail!("tab {tab_id} does not belong to session {session_id}"),
            None => anyhow::bail!("unknown tab: {tab_id}"),
        }

        // Persist first.
        self.session_store.delete_agent_tab(tab_id)?;

        // Graceful PTY teardown (SIGTERM into the terminating set), then clear
        // every runtime map this tab keyed via the shared `clear_tab_runtime`
        // (begin_close_provider only drops `providers`, which clear_tab_runtime
        // then finds already gone — a harmless no-op).
        let label = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.title.clone().unwrap_or_else(|| s.branch_name.clone()))
            .unwrap_or_else(|| tab_id.to_string());
        // No worktree removal is deferred on a tab close, so the return is None.
        let _ = self.begin_close_provider(tab_id, label, None);
        self.clear_tab_runtime(tab_id);
        self.agent_tabs.remove(tab_id);
        // Closing this tab may have removed the agent's last live process (its
        // session-slot tab already dormant). No tab is privileged, so recompute:
        // the agent detaches once nothing of it is live/launching.
        if !self.any_tab_active(session_id) {
            self.mark_session_status(session_id, crate::model::SessionStatus::Detached);
        }
        Ok(())
    }

    /// Retarget a tab's provider (effective on its next launch). For the session-slot tab
    /// (`tab_id == session_id`) this delegates to the untouched
    /// [`Engine::change_agent_provider`]; for an extra tab it updates only that
    /// tab's row, pinning the previously-running provider if it is live.
    pub fn change_tab_provider(
        &mut self,
        session_id: &str,
        tab_id: &str,
        provider: ProviderKind,
    ) -> anyhow::Result<ChangeAgentProviderOutcome> {
        if tab_id == session_id {
            return self.change_agent_provider(session_id, provider);
        }

        if !self
            .config
            .providers
            .commands
            .contains_key(provider.as_str())
        {
            anyhow::bail!("provider \"{}\" is not configured", provider.as_str());
        }

        let running = self.providers.contains_key(tab_id);
        // Persist first: read the previous value read-only and verify ownership,
        // write the DB, and only mutate the in-memory row after the write
        // succeeds — so a persistence failure leaves memory and SQLite in sync
        // (mirroring `create_tab`/`close_tab`).
        let previous = self
            .agent_tabs
            .get(tab_id)
            .filter(|t| t.session_id == session_id)
            .map(|t| t.provider.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown tab: {tab_id}"))?;
        self.session_store
            .update_agent_tab_provider(tab_id, provider.as_str())?;
        if let Some(tab) = self.agent_tabs.get_mut(tab_id) {
            tab.provider = provider.clone();
        }

        if running {
            self.running_provider_pins
                .entry(tab_id.to_string())
                .or_insert_with(|| previous.clone());
        }

        // Resume eligibility for an extra tab retarget follows the same rule
        // as an actual launch: the newly-selected provider must be
        // resume-eligible for this session AND no other live/launching tab of
        // the session currently owns that provider's conversation. Compute it
        // with `tab_resume_decision` (requested=true) rather than hardcoding
        // false, so a retarget to a previously-started, not-live-elsewhere
        // provider correctly reports that it will resume on next launch.
        let resume_available = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|session| self.tab_resume_decision(session, tab_id, &provider, true))
            .unwrap_or(false);

        Ok(ChangeAgentProviderOutcome {
            previous,
            running,
            resume_available,
        })
    }

    /// Remember (or clear) the tab the user last focused on `session_id`.
    /// Normalizes `tab_id` to `None` when it is absent, equal to `session_id`
    /// (the session-slot tab, represented as absence), or does not name a live
    /// extra tab belonging to this session — so a stale/foreign id can never be
    /// persisted. DB-first (mirroring `ToggleAgentAutoReopen`): the storage
    /// write happens before the in-memory session is updated, so a persistence
    /// failure leaves memory and SQLite in sync.
    pub fn set_last_focused_tab(
        &mut self,
        session_id: &str,
        tab_id: Option<&str>,
    ) -> anyhow::Result<()> {
        if !self.sessions.iter().any(|s| s.id == session_id) {
            anyhow::bail!("unknown session: {session_id}");
        }
        let normalized = match tab_id {
            Some(id)
                if id != session_id
                    && self
                        .agent_tabs
                        .get(id)
                        .is_some_and(|t| t.session_id == session_id) =>
            {
                Some(id.to_string())
            }
            _ => None,
        };
        self.session_store
            .set_last_focused_tab(session_id, normalized.as_deref())?;
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.last_focused_tab = normalized;
        }
        Ok(())
    }
}

/// Result of [`Engine::change_agent_provider`]: the data a surface needs to
/// craft its own user-facing status message after a successful swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeAgentProviderOutcome {
    /// The provider the session used before the swap.
    pub previous: ProviderKind,
    /// Whether a provider was still running on the session's PTY at swap time.
    /// When true, the swap takes effect only after the user exits and relaunches.
    pub running: bool,
    /// Whether the newly-selected provider can resume a prior conversation on
    /// this worktree (it supports resume and has been launched here before).
    pub resume_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    #[test]
    fn is_agent_streaming_honors_the_hysteresis_window() {
        let (mut engine, _tmp) = test_engine();

        // Fresh activity → streaming.
        engine
            .pty_activity
            .insert("fresh".to_string(), Instant::now());
        assert!(engine.is_agent_streaming("fresh"));

        // Stamped past the window → not streaming.
        engine.pty_activity.insert(
            "stale".to_string(),
            Instant::now() - (AGENT_STREAMING_WINDOW + Duration::from_millis(50)),
        );
        assert!(!engine.is_agent_streaming("stale"));

        // No entry at all → not streaming.
        assert!(!engine.is_agent_streaming("absent"));
    }

    #[test]
    fn recent_typing_voids_the_streaming_indicator() {
        let (mut engine, _tmp) = test_engine();

        // Fresh output activity reads as streaming on its own.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        assert!(engine.is_agent_streaming("s1"));

        // The user typing into the agent echoes back as that same output, so a
        // keystroke within the suppression window voids the indicator.
        engine.note_pty_input("s1");
        assert!(
            !engine.is_agent_streaming("s1"),
            "recent typing must void the streaming indicator"
        );
    }

    #[test]
    fn suppression_window_outlasts_streaming_window() {
        // The feature relies on the input-suppression window being strictly
        // longer than the output hysteresis: the echo of the last keystroke
        // must fully age out of pty_activity before suppression lifts, or the
        // indicator would flicker back on right after typing. Guard the
        // invariant so a future tweak to either constant can't silently break
        // it.
        assert!(AGENT_INPUT_SUPPRESSION_WINDOW > AGENT_STREAMING_WINDOW);
    }

    #[test]
    fn streaming_returns_once_the_input_window_lapses() {
        let (mut engine, _tmp) = test_engine();

        // Output is fresh, but the last keystroke is older than the suppression
        // window — genuine ongoing output must read as streaming again.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_input.insert(
            "s1".to_string(),
            Instant::now() - (AGENT_INPUT_SUPPRESSION_WINDOW + Duration::from_millis(50)),
        );
        assert!(
            engine.is_agent_streaming("s1"),
            "once the input window lapses, ongoing output reads as streaming"
        );
    }

    #[test]
    fn progress_report_overrides_working_true() {
        let (mut engine, _tmp) = test_engine();
        // No output activity at all, but a fresh progress report says "working".
        engine.pty_progress.insert(
            "s1".to_string(),
            ProgressReport {
                working: true,
                at: Instant::now(),
            },
        );
        assert!(
            engine.is_agent_streaming("s1"),
            "a fresh working progress report must light the indicator"
        );
    }

    #[test]
    fn progress_report_overrides_working_false_even_with_activity() {
        let (mut engine, _tmp) = test_engine();
        // Output activity would read as streaming, but the agent's own progress
        // report states it is idle — the report wins.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_progress.insert(
            "s1".to_string(),
            ProgressReport {
                working: false,
                at: Instant::now(),
            },
        );
        assert!(
            !engine.is_agent_streaming("s1"),
            "a fresh idle progress report must override the activity heuristic"
        );
    }

    #[test]
    fn stale_progress_report_falls_back_to_heuristic() {
        let (mut engine, _tmp) = test_engine();
        // The last progress report is older than the authority window, so the
        // output-activity heuristic takes over again.
        engine.pty_progress.insert(
            "s1".to_string(),
            ProgressReport {
                working: true,
                at: Instant::now() - (PROGRESS_AUTHORITY_WINDOW + Duration::from_millis(50)),
            },
        );
        assert!(
            !engine.is_agent_streaming("s1"),
            "a stale report grants no authority; with no activity the agent is idle"
        );

        // With fresh activity and the same stale report, the heuristic lights it.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        assert!(
            engine.is_agent_streaming("s1"),
            "a stale report must not suppress genuine ongoing output"
        );
    }

    #[test]
    fn note_agent_viewed_clears_attention_immediately() {
        let (mut engine, _tmp) = test_engine();
        engine.needs_attention.insert("s1".to_string());
        assert!(engine.tab_needs_attention("s1"));
        engine.note_agent_viewed("s1");
        assert!(
            !engine.tab_needs_attention("s1"),
            "looking at a tab must clear its attention flag at once"
        );
    }

    #[test]
    fn attention_engaged_covers_viewing_and_typing() {
        let now = Instant::now();
        let mut viewed = HashMap::new();
        let mut typed = HashMap::new();

        // Nothing recorded → not engaged.
        assert!(!attention_engaged(&viewed, &typed, "s1", now));

        // Fresh view → engaged.
        viewed.insert("s1".to_string(), now);
        assert!(attention_engaged(&viewed, &typed, "s1", now));

        // Stale view → not engaged.
        viewed.insert(
            "s1".to_string(),
            now - (ATTENTION_ENGAGED_WINDOW + Duration::from_millis(50)),
        );
        assert!(!attention_engaged(&viewed, &typed, "s1", now));

        // Fresh typing alone → engaged.
        typed.insert("s1".to_string(), now);
        assert!(attention_engaged(&viewed, &typed, "s1", now));

        // Stale typing → not engaged.
        typed.insert(
            "s1".to_string(),
            now - (AGENT_INPUT_SUPPRESSION_WINDOW + Duration::from_millis(50)),
        );
        assert!(!attention_engaged(&viewed, &typed, "s1", now));
    }

    #[test]
    fn clear_tab_runtime_drops_attention_and_progress() {
        let (mut engine, _tmp) = test_engine();
        engine.needs_attention.insert("s1".to_string());
        engine.pty_progress.insert(
            "s1".to_string(),
            ProgressReport {
                working: true,
                at: Instant::now(),
            },
        );
        engine.agent_viewed.insert("s1".to_string(), Instant::now());
        engine.clear_tab_runtime("s1");
        assert!(!engine.tab_needs_attention("s1"));
        assert!(!engine.pty_progress.contains_key("s1"));
        assert!(!engine.agent_viewed.contains_key("s1"));
    }

    #[test]
    fn note_agent_viewed_if_known_gates_on_a_real_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine.needs_attention.insert("s1".to_string());

        // A bogus id (stale deep link, race, retry) is a no-op: it neither stamps
        // `agent_viewed` nor touches the flag.
        engine.note_agent_viewed_if_known("does-not-exist");
        assert!(
            engine.agent_viewed.is_empty(),
            "a bogus id must not leak an agent_viewed entry"
        );
        assert!(engine.tab_needs_attention("s1"));

        // A real session id both stamps engagement and clears the flag.
        engine.note_agent_viewed_if_known("s1");
        assert!(engine.agent_viewed.contains_key("s1"));
        assert!(!engine.tab_needs_attention("s1"));
    }

    #[test]
    fn apply_attention_decision_sets_flag_when_not_engaged() {
        let mut set = HashSet::new();
        apply_attention_decision(&mut set, &["s1".to_string()], true, |_| false);
        assert!(set.contains("s1"), "a fired, unengaged tab must be flagged");
    }

    #[test]
    fn apply_attention_decision_suppresses_and_clears_when_engaged() {
        let mut set = HashSet::new();
        set.insert("s1".to_string());
        // Engaged: the fired signal is suppressed AND the existing flag is cleared.
        apply_attention_decision(&mut set, &["s1".to_string()], true, |_| true);
        assert!(
            !set.contains("s1"),
            "engagement must both suppress the new signal and clear the old flag"
        );
    }

    #[test]
    fn apply_attention_decision_clears_stale_flag_without_a_new_signal() {
        let mut set = HashSet::new();
        set.insert("s1".to_string());
        // No new signal this tick, but the user is now engaged → clear.
        apply_attention_decision(&mut set, &[], true, |tab| tab == "s1");
        assert!(!set.contains("s1"));
    }

    #[test]
    fn apply_attention_decision_retains_flag_while_unengaged() {
        let mut set = HashSet::new();
        set.insert("s1".to_string());
        apply_attention_decision(&mut set, &[], true, |_| false);
        assert!(
            set.contains("s1"),
            "an unviewed flag must persist across ticks"
        );
    }

    #[test]
    fn apply_attention_decision_disabled_clears_everything() {
        let mut set = HashSet::new();
        set.insert("s1".to_string());
        apply_attention_decision(&mut set, &["s2".to_string()], false, |_| false);
        assert!(set.is_empty(), "the feature being off must drop every flag");
    }

    // A one-shot child that emits, in stream order: a bell, an OSC 9 notification,
    // then an OSC 9;4 working progress report (progress LAST so observing it means
    // the earlier signals were already scanned). Used by the real-PTY smoke tests.
    const EMIT_SIGNALS: &str = "printf '\\007\\033]9;hi\\007\\033]9;4;1;50\\007X'";

    /// Spawn a tracked PTY that emits all three signals under `tab` and block until
    /// its progress report lands (proving the reader thread scanned the payload).
    fn seed_signaling_provider(engine: &mut Engine, tab: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("worktree dir");
        let args = vec!["-c".to_string(), EMIT_SIGNALS.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            tmp.path(),
            5,
            40,
            100,
            crate::pty::PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn signaling pty");
        engine.providers.insert(tab.to_string(), client);
        for _ in 0..200 {
            if engine
                .providers
                .get(tab)
                .and_then(|p| p.progress_report())
                .is_some()
            {
                return tmp;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for a progress report on {tab}");
    }

    #[test]
    fn poll_agent_signals_flags_attention_and_records_progress() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.attention_indicator = true;
        engine.config.ui.attention_on_bell = true;
        let _wt = seed_signaling_provider(&mut engine, "s1");

        engine.poll_agent_signals();

        assert!(
            engine.tab_needs_attention("s1"),
            "an unengaged agent that signalled must be flagged"
        );
        assert!(
            engine
                .pty_progress
                .get("s1")
                .expect("progress recorded")
                .working,
            "the 9;4;1 progress report must feed the working override"
        );
    }

    #[test]
    fn poll_agent_signals_suppresses_when_engaged() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.attention_indicator = true;
        engine.config.ui.attention_on_bell = true;
        let _wt = seed_signaling_provider(&mut engine, "s1");

        // The user is looking at this tab right now.
        engine.note_agent_viewed("s1");
        engine.poll_agent_signals();

        assert!(
            !engine.tab_needs_attention("s1"),
            "a tab the user is viewing must not be flagged"
        );
        // Progress is still captured regardless of engagement.
        assert!(engine.pty_progress.contains_key("s1"));
    }

    #[test]
    fn poll_agent_signals_disabled_records_progress_but_not_attention() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.attention_indicator = false;
        let _wt = seed_signaling_provider(&mut engine, "s1");

        engine.poll_agent_signals();

        assert!(
            !engine.tab_needs_attention("s1"),
            "attention_indicator=false must record no attention"
        );
        assert!(
            engine
                .pty_progress
                .get("s1")
                .expect("progress recorded")
                .working,
            "progress still feeds the working indicator even with attention off"
        );
    }

    // A clipboard SET, a notification, and a progress report in one payload. The
    // trailing progress lets the seed helper wait for the reader thread to have
    // scanned (and thus filled the passthrough ring).
    const EMIT_PASSTHROUGH: &str =
        "printf '\\033]52;c;aGVsbG8=\\007\\033]9;hi\\007\\033]9;4;1;50\\007X'";

    fn seed_passthrough_provider(engine: &mut Engine, tab: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("worktree dir");
        let args = vec!["-c".to_string(), EMIT_PASSTHROUGH.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            tmp.path(),
            5,
            40,
            100,
            crate::pty::PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn passthrough pty");
        engine.providers.insert(tab.to_string(), client);
        // Gate readiness on the passthrough RING being non-empty, not on the
        // progress report. The reader loop sets the progress slot earlier in the
        // same scan pass than it pushes captures into the ring, so a progress-based
        // wait can observe a half-applied pass where the ring is still empty
        // (FIX-F15). A non-empty ring proves the capture push completed.
        for _ in 0..200 {
            if engine
                .providers
                .get(tab)
                .map(|p| p.passthrough_pending())
                .unwrap_or(0)
                > 0
            {
                return tmp;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for the passthrough emitter on {tab}");
    }

    #[test]
    fn take_host_passthrough_master_switch_drains_and_discards() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = false;
        let _wt = seed_passthrough_provider(&mut engine, "s1");

        let out = engine.take_host_passthrough(Some("s1"), false);
        assert!(out.is_empty(), "master off forwards nothing");
        // The ring was still drained, so a later enable does not replay the backlog.
        engine.config.capabilities.passthrough = true;
        assert!(
            engine.take_host_passthrough(Some("s1"), false).is_empty(),
            "no stale replay after toggling the master switch on"
        );
    }

    #[test]
    fn take_host_passthrough_clipboard_focused_gating() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;
        engine.config.capabilities.clipboard_passthrough = "focused".to_string();
        let _wt = seed_passthrough_provider(&mut engine, "s1");

        // Not the focused tab: notify + progress forward, clipboard does not.
        let bg = engine.take_host_passthrough(Some("other"), false);
        assert!(bg.windows(4).any(|w| w == b"]9;h"), "notify forwarded");
        assert!(
            !bg.windows(4).any(|w| w == b"]52;"),
            "background clipboard must not forward under focused"
        );

        // Focused tab: clipboard forwards too.
        let _wt2 = seed_passthrough_provider(&mut engine, "s1");
        let fg = engine.take_host_passthrough(Some("s1"), false);
        assert!(
            fg.windows(4).any(|w| w == b"]52;"),
            "focused clipboard must forward"
        );
    }

    #[test]
    fn take_host_passthrough_clipboard_off_and_always() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;

        engine.config.capabilities.clipboard_passthrough = "off".to_string();
        let _a = seed_passthrough_provider(&mut engine, "s1");
        let off = engine.take_host_passthrough(Some("s1"), false);
        assert!(
            !off.windows(4).any(|w| w == b"]52;"),
            "clipboard off never forwards even for the focused tab"
        );

        engine.config.capabilities.clipboard_passthrough = "always".to_string();
        let _b = seed_passthrough_provider(&mut engine, "s1");
        let always = engine.take_host_passthrough(Some("other"), false);
        assert!(
            always.windows(4).any(|w| w == b"]52;"),
            "clipboard always forwards even for a background tab"
        );
    }

    #[test]
    fn discard_passthrough_backlog_drops_without_forwarding() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;
        let _wt = seed_passthrough_provider(&mut engine, "s1");

        // Discard drains the ring but forwards nothing.
        engine.discard_passthrough_backlog();
        // A subsequent forward sees an empty ring: the backlog was not replayed.
        assert!(
            engine.take_host_passthrough(Some("s1"), false).is_empty(),
            "discarded backlog must not be forwarded on the next drain"
        );
    }

    #[test]
    fn take_host_passthrough_tmux_wraps() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;
        let _wt = seed_passthrough_provider(&mut engine, "s1");

        let out = engine.take_host_passthrough(Some("s1"), true);
        assert!(
            out.starts_with(b"\x1bPtmux;"),
            "wrap_for_tmux must emit a tmux passthrough envelope"
        );
    }

    #[test]
    fn agent_reconnect_status_message_reads_as_completed() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feature");

        // Resume → a completed-action message naming provider, agent, project.
        assert_eq!(
            engine.agent_reconnect_status_message(&session, true),
            "Resumed claude agent \"feature\" in project \"p1-name\"."
        );

        // Fresh → the no-resume variant with the /sessions hint.
        assert_eq!(
            engine.agent_reconnect_status_message(&session, false),
            "Started fresh claude session for agent \"feature\" in project \"p1-name\". \
             Use /sessions inside the agent to restore a prior conversation."
        );
    }

    #[test]
    fn refresh_terminal_foregrounds_is_a_noop_without_terminals() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.last_foreground_refresh.is_none());

        // First call stamps the timestamp even with nothing to probe, so the
        // throttle starts ticking from the first invocation.
        engine.refresh_terminal_foregrounds();
        let first = engine
            .last_foreground_refresh
            .expect("first refresh stamps the timestamp");

        // An immediate second call is throttled: the timestamp does not advance.
        engine.refresh_terminal_foregrounds();
        assert_eq!(
            engine.last_foreground_refresh,
            Some(first),
            "a second immediate refresh must be a throttled no-op"
        );
    }

    #[test]
    fn refresh_terminal_foregrounds_throttles_by_wall_clock() {
        let (mut engine, _tmp) = test_engine();

        // Spawn a real `cat` companion terminal: `foreground_process_name`
        // requires a live PTY master fd to call `tcgetpgrp`. `cat` is spawned
        // directly (no shell), so tcgetpgrp == the child pid and the foreground
        // probe returns None (shell-is-foreground). We assert on the THROTTLE,
        // not on the probe's value — faking tcgetpgrp is out of scope.
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        // First refresh runs and stamps the timestamp.
        engine.refresh_terminal_foregrounds();
        let first = engine
            .last_foreground_refresh
            .expect("first refresh stamps the timestamp");

        // Within the interval the refresh is skipped (timestamp unchanged).
        engine.refresh_terminal_foregrounds();
        assert_eq!(
            engine.last_foreground_refresh,
            Some(first),
            "a refresh within the interval must not run"
        );

        // Rewind the timestamp past the interval to simulate elapsed wall-clock
        // time, then the next call runs again and re-stamps.
        engine.last_foreground_refresh =
            Some(first - (FOREGROUND_REFRESH_INTERVAL + Duration::from_millis(50)));
        engine.refresh_terminal_foregrounds();
        let second = engine
            .last_foreground_refresh
            .expect("refresh after the interval re-stamps");
        assert!(
            second > first,
            "a refresh after the interval lapses must run and advance the timestamp"
        );
    }

    #[test]
    fn persist_projects_to_config_round_trips_runtime_projects() {
        let (mut engine, _tmp) = test_engine();
        // The patch path requires an existing file; create a minimal one.
        std::fs::write(&engine.paths.config_path, "# dux config\n").expect("seed config");

        let mut p1 = sample_project("p1", "/repo/one");
        p1.startup_command = Some("npm install".to_string());
        p1.env.insert("KEY".to_string(), "value".to_string());
        let mut p2 = sample_project("p2", "/repo/two");
        p2.explicit_default_provider = Some(ProviderKind::new("codex"));
        engine.projects.push(p1);
        engine.projects.push(p2);

        engine
            .persist_projects_to_config()
            .expect("persist projects to config");

        let saved = std::fs::read_to_string(&engine.paths.config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.projects.len(), 2);

        let one = parsed
            .projects
            .iter()
            .find(|p| p.id == "p1")
            .expect("p1 present");
        assert_eq!(one.startup_command.as_deref(), Some("npm install"));
        assert_eq!(one.env.get("KEY").map(String::as_str), Some("value"));

        let two = parsed
            .projects
            .iter()
            .find(|p| p.id == "p2")
            .expect("p2 present");
        assert_eq!(two.default_provider.as_deref(), Some("codex"));
    }

    #[test]
    fn persist_projects_to_config_writes_portable_home_path() {
        // A project under $HOME must be written to config.toml in the portable
        // `$HOME/...` form, not a machine-specific absolute path (the "portable
        // desired state" tenet). The inline-Add handler is now the single config
        // writer, so this guards against it pinning an absolute path.
        let Some(home) = home::home_dir() else {
            return; // no resolvable home in this environment; nothing to assert.
        };
        let (mut engine, _tmp) = test_engine();
        std::fs::write(&engine.paths.config_path, "# dux config\n").expect("seed config");

        let abs = home.join("code/myproject");
        let mut p = sample_project("ph", abs.to_string_lossy().as_ref());
        // sample_project sets an absolute path; ensure it is exactly under home.
        p.path = abs.to_string_lossy().into_owned();
        engine.projects.push(p);

        engine
            .persist_projects_to_config()
            .expect("persist projects to config");

        let saved = std::fs::read_to_string(&engine.paths.config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        let entry = parsed
            .projects
            .iter()
            .find(|p| p.id == "ph")
            .expect("project present");
        assert_eq!(
            entry.path, "$HOME/code/myproject",
            "config must store the portable $HOME form, not an absolute path"
        );
    }

    #[test]
    fn apply_reloaded_config_swaps_config_and_refreshes_state() {
        let (mut engine, _tmp) = test_engine();
        // Baseline differs from the values we'll reload.
        engine.config.ui.github_integration = false;
        engine.config.defaults.provider = "claude".to_string();
        engine.github_integration_enabled = false;

        let mut new_config = Config::default();
        new_config.ui.github_integration = true;
        new_config.defaults.provider = "codex".to_string();

        engine
            .apply_reloaded_config(new_config)
            .expect("apply reloaded config");

        assert!(engine.config.ui.github_integration);
        assert!(engine.github_integration_enabled);
        assert_eq!(engine.config.defaults.provider, "codex");
    }

    // -- Config writer on the engine: env/macros now save through the queue. --

    #[test]
    fn persist_global_env_writes_through_queue() {
        let (mut engine, _tmp) = test_engine();
        let mut env = std::collections::BTreeMap::new();
        env.insert("API".into(), "k".into());
        engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("apply");
        assert_eq!(engine.config.env.get("API").map(String::as_str), Some("k"));
        engine.config_writer.flush();
        assert!(
            std::fs::read_to_string(&engine.paths.config_path)
                .unwrap()
                .contains("API = \"k\"")
        );
    }

    #[test]
    fn persist_global_env_rolls_back_on_write_failure() {
        // Eager save through a dead writer fails; the in-memory env must roll back
        // so it never diverges from disk.
        let (mut engine, _tmp) = test_engine();
        engine.config.env.insert("OLD".into(), "v".into());
        engine.config_writer = crate::config_queue::ConfigWriteQueue::with_dead_writer(
            engine.paths.config_path.clone(),
        );
        let mut env = std::collections::BTreeMap::new();
        env.insert("NEW".into(), "x".into());

        let reaction = engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, crate::statusline::StatusTone::Error)
            }
            _ => panic!("expected Error status"),
        }
        // Rolled back to the previous env.
        assert_eq!(engine.config.env.get("OLD").map(String::as_str), Some("v"));
        assert!(!engine.config.env.contains_key("NEW"));
    }

    // -- Project add: SQLite rollback on config-write failure. --

    #[test]
    fn project_add_config_failure_removes_sqlite_row() {
        // Force a config-write failure by pointing the writer at a nonexistent
        // directory so save_eager gets an I/O error. The SQLite insert that
        // happens first must be rolled back so the project row does not survive.
        let (mut engine, _tmp) = test_engine();
        engine.config_writer =
            crate::config_queue::ConfigWriteQueue::new("/nonexistent/dir/cfg.toml".into());
        let before = engine.session_store.load_projects().unwrap().len();
        let project = test_support::sample_project("p1", "/tmp/p1");
        let _ = engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project,
                status_message: "added".into(),
            }),
            status_op_id: None,
        });
        assert_eq!(
            engine.session_store.load_projects().unwrap().len(),
            before,
            "a failed config write must not leave a SQLite row"
        );
    }

    #[test]
    fn project_add_config_failure_does_not_resurrect_phantom_on_next_save() {
        // Prove that a rolled-back add does NOT pollute self.config.projects so
        // the phantom project cannot be written to disk by the next unrelated
        // eager save. Without the fix, persist_projects_to_config() rewrote
        // self.config.projects before failing, and the next eager save (e.g.
        // PersistGlobalEnv) would clone that mutated config and write the ghost.
        let (mut engine, tmp) = test_engine();

        // Point the writer at a nonexistent path to force the initial add to fail.
        engine.config_writer =
            crate::config_queue::ConfigWriteQueue::new("/nonexistent/dir/cfg.toml".into());
        let project = test_support::sample_project("ghost", "/tmp/ghost");
        let _ = engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project,
                status_message: "added".into(),
            }),
            status_op_id: None,
        });

        // Both self.projects and self.config.projects must not contain "ghost".
        assert!(
            !engine.projects.iter().any(|p| p.id == "ghost"),
            "ghost must not be in engine.projects after rollback"
        );
        assert!(
            !engine.config.projects.iter().any(|p| p.id == "ghost"),
            "ghost must not be in engine.config.projects after rollback"
        );

        // Swap in a working writer pointed at the real config path and fire an
        // unrelated eager save (PersistGlobalEnv). This clones self.config and
        // writes it — the ghost must NOT appear in the resulting file.
        let config_path = tmp.path().join("config.toml");
        engine.config_writer = crate::config_queue::ConfigWriteQueue::new(config_path.clone());
        let mut env = std::collections::BTreeMap::new();
        env.insert("K".into(), "v".into());
        engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("PersistGlobalEnv must succeed");
        engine.config_writer.flush();

        let on_disk = std::fs::read_to_string(&config_path).expect("config written");
        assert!(
            !on_disk.contains("ghost"),
            "ghost project must not appear in config after an unrelated save: {on_disk}"
        );
    }

    // -- Reload command deferral. --

    /// A test `ConfigSurface` whose `reload` posts a config carrying a known,
    /// distinguishing marker (`defaults.provider`) so a test can prove the
    /// reloaded config actually landed (and is not just `Config::default()`).
    /// Drives completion through `ReloadCompletionGuard`, the real F5-safe path.
    struct MarkerReloadSurface {
        provider: String,
    }

    impl crate::engine::ConfigSurface for MarkerReloadSurface {
        fn reload(
            &self,
            _paths: DuxPaths,
            worker_tx: std::sync::mpsc::Sender<crate::worker::WorkerEvent>,
        ) {
            let mut config = Config::default();
            config.defaults.provider = self.provider.clone();
            crate::engine::ReloadCompletionGuard::new(worker_tx).complete(Ok(config));
        }

        fn recover_render(&self, config: &Config) -> String {
            crate::config_write::render_config_plain(config)
        }
    }

    #[test]
    fn config_mutating_commands_defer_while_reloading() {
        let (mut engine, _tmp) = test_engine();
        // Drive a REAL reload so the barrier is opened by the engine itself
        // (quiesce + `reloading`), not hand-set — a missing-quiesce or
        // wiring regression would then be visible (F13). The surface's reload
        // posts immediately, but the engine only drains on the next
        // `process_worker_event`, so the command dispatched here still defers.
        engine.surface = Box::new(MarkerReloadSurface {
            provider: "codex".to_string(),
        });
        let reaction = engine.apply(Command::ReloadConfig).expect("reload");
        assert!(matches!(reaction, EventReaction::Nothing));
        assert!(engine.reloading, "ReloadConfig must open the barrier");
        assert!(
            engine.reload_guard.is_some(),
            "ReloadConfig must hold the writer quiesce open"
        );

        let mut env = std::collections::BTreeMap::new();
        env.insert("API".into(), "k".into());
        let reaction = engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("apply");
        // Deferred: no state change, no status.
        assert!(matches!(reaction, EventReaction::Nothing));
        assert!(!engine.config.env.contains_key("API"));
        assert_eq!(engine.deferred_commands.len(), 1);
    }

    #[test]
    fn config_reload_ready_drains_deferred_commands() {
        let (mut engine, _tmp) = test_engine();
        // Baseline provider differs from the value the reload will deliver, so we
        // can prove the reloaded config actually landed (F12: the reloaded config
        // must DIFFER from the initial one, not both be `Config::default()`).
        engine.config.defaults.provider = "claude".to_string();

        // Drive a REAL `ReloadConfig` (F13): the engine opens the barrier and the
        // surface posts a `ConfigReloadReady` carrying the codex-marked config.
        engine.surface = Box::new(MarkerReloadSurface {
            provider: "codex".to_string(),
        });
        engine.apply(Command::ReloadConfig).expect("reload");
        assert!(engine.reloading);

        // Defer a PersistGlobalEnv during the in-flight reload.
        let mut env = std::collections::BTreeMap::new();
        env.insert("API".into(), "k".into());
        engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("apply");
        assert_eq!(engine.deferred_commands.len(), 1);

        // The surface already posted the completion; drain it through the real
        // worker-event path so the barrier closes and the deferred command drains.
        let event = engine.worker_rx.recv().expect("reload completion");
        let reaction = engine.process_worker_event(event);

        // Deferral folds the reload + the deferred save into one Multi.
        assert!(matches!(reaction, EventReaction::Multi(_)));
        assert!(!engine.reloading);
        assert!(engine.reload_guard.is_none(), "barrier must be released");
        assert!(engine.deferred_commands.is_empty());

        // The reloaded config landed (provider swapped from claude → codex).
        assert_eq!(
            engine.config.defaults.provider, "codex",
            "the reloaded config must be applied"
        );

        // The deferred env change survives IN MEMORY after the reload — this is
        // the F1 regression guard. Simulate the surface re-applying the reaction
        // it was handed: under the F1 bug that reaction carried the BARE reloaded
        // config (no env), so re-applying would wipe the env back out of memory.
        let EventReaction::Multi(reactions) = reaction else {
            panic!("expected Multi");
        };
        let surfaced_config = reactions
            .into_iter()
            .find_map(|r| match r {
                EventReaction::ApplyReloadedConfig(cfg) => Some(*cfg),
                _ => None,
            })
            .expect("Multi must carry an ApplyReloadedConfig for the surface");
        engine
            .apply_reloaded_config(surfaced_config)
            .expect("surface re-apply");
        assert_eq!(
            engine.config.env.get("API").map(String::as_str),
            Some("k"),
            "the deferred env change must survive the surface's re-apply (F1)"
        );
        // …and the reloaded provider must still be present after that re-apply.
        assert_eq!(engine.config.defaults.provider, "codex");

        // The deferred env save also landed on disk (the LAST write wins).
        engine.config_writer.flush();
        assert!(
            std::fs::read_to_string(&engine.paths.config_path)
                .unwrap()
                .contains("API = \"k\"")
        );
    }

    /// A test `ConfigSurface` whose `reload` reports a validation FAILURE (posts an
    /// `Err` completion) through the F5-safe guard.
    struct FailingReloadSurface;

    impl crate::engine::ConfigSurface for FailingReloadSurface {
        fn reload(
            &self,
            _paths: DuxPaths,
            worker_tx: std::sync::mpsc::Sender<crate::worker::WorkerEvent>,
        ) {
            crate::engine::ReloadCompletionGuard::new(worker_tx)
                .complete(Err("invalid config".to_string()));
        }

        fn recover_render(&self, config: &Config) -> String {
            crate::config_write::render_config_plain(config)
        }
    }

    #[test]
    fn failed_reload_still_drains_deferred_and_surfaces_the_failure() {
        // F6 + the failure-with-deferral ordering: when a reload FAILS while
        // commands were deferred, the deferred commands must still be applied
        // against the unchanged (current) config rather than dropped, AND the
        // reload-failed reaction must be the LAST element so its error wins the
        // surface's status line over the deferred save's success message.
        let (mut engine, _tmp) = test_engine();
        engine.surface = Box::new(FailingReloadSurface);

        engine.apply(Command::ReloadConfig).expect("reload");
        assert!(engine.reloading);

        let mut env = std::collections::BTreeMap::new();
        env.insert("API".into(), "k".into());
        engine
            .apply(Command::PersistGlobalEnv { env })
            .expect("apply");
        assert_eq!(engine.deferred_commands.len(), 1);

        let event = engine.worker_rx.recv().expect("reload completion");
        let reaction = engine.process_worker_event(event);

        // Barrier closed, deferred drained.
        assert!(!engine.reloading);
        assert!(engine.reload_guard.is_none());
        assert!(engine.deferred_commands.is_empty());

        // The deferred env command was applied against the still-current config
        // (NOT dropped) and persisted.
        assert_eq!(engine.config.env.get("API").map(String::as_str), Some("k"));
        engine.config_writer.flush();
        assert!(
            std::fs::read_to_string(&engine.paths.config_path)
                .unwrap()
                .contains("API = \"k\"")
        );

        // The Multi carries no ApplyReloadedConfig (the reload failed), and the
        // LAST reaction is the reload-failed modal so its error survives.
        let EventReaction::Multi(reactions) = reaction else {
            panic!("expected Multi");
        };
        assert!(
            !reactions
                .iter()
                .any(|r| matches!(r, EventReaction::ApplyReloadedConfig(_))),
            "a failed reload must not surface an ApplyReloadedConfig"
        );
        assert!(
            matches!(
                reactions.last(),
                Some(EventReaction::OpenConfigReloadFailedModal(_))
            ),
            "the reload-failed reaction must be LAST so its error wins the status line"
        );
    }

    /// A test `ConfigSurface` whose `reload` does NOTHING (never posts a
    /// completion), leaving the engine's reload barrier open. Lets a test observe
    /// the in-flight state and exercise the reentrancy/recover-during-reload
    /// rejections without a worker race.
    struct StuckReloadSurface;

    impl crate::engine::ConfigSurface for StuckReloadSurface {
        fn reload(
            &self,
            _paths: DuxPaths,
            _worker_tx: std::sync::mpsc::Sender<crate::worker::WorkerEvent>,
        ) {
        }

        fn recover_render(&self, config: &Config) -> String {
            crate::config_write::render_config_plain(config)
        }
    }

    #[test]
    fn reentrant_reload_is_rejected_and_keeps_the_first_barrier() {
        let (mut engine, _tmp) = test_engine();
        engine.surface = Box::new(StuckReloadSurface);

        engine.apply(Command::ReloadConfig).expect("first reload");
        assert!(engine.reloading);
        assert!(engine.reload_guard.is_some());

        // A second reload while one is in flight must be refused — it must NOT
        // drop the live guard or spawn a second worker (F4).
        let reaction = engine.apply(Command::ReloadConfig).expect("second reload");
        match reaction {
            EventReaction::Status(update) => {
                assert!(
                    update.message.contains("already in progress"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected an 'already in progress' status"),
        }
        // The first barrier is intact.
        assert!(engine.reloading);
        assert!(engine.reload_guard.is_some());
    }

    #[test]
    fn recover_config_is_rejected_during_a_reload() {
        let (mut engine, _tmp) = test_engine();
        engine.surface = Box::new(StuckReloadSurface);

        engine.apply(Command::ReloadConfig).expect("reload");
        assert!(engine.reloading);

        // Recovery during an open reload would, on its own quiesce-guard drop,
        // resume the writer while the reload still holds it. It must be refused
        // instead (F7), and the reload barrier must stay open.
        let reaction = engine.apply(Command::RecoverConfig).expect("recover");
        match reaction {
            EventReaction::Status(update) => {
                assert!(
                    update.message.contains("reload is in progress"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected a 'reload is in progress' status"),
        }
        assert!(engine.reloading, "the reload barrier must remain open");
        assert!(engine.reload_guard.is_some());
    }

    #[test]
    fn reload_worker_panic_still_closes_the_barrier() {
        // F5: the reload completion guard guarantees a `ConfigReloadReady` is
        // posted even when the reload worker drops without calling `complete`.
        // Build the guard and drop it without completing (the panic/early-return
        // shape) — it must post an Err completion.
        let (mut engine, _tmp) = test_engine();
        engine.apply(Command::ReloadConfig).expect("reload");
        assert!(engine.reloading);
        // Drain the NoopConfigSurface's completion that ReloadConfig already
        // posted so the channel is clean, then simulate a DIFFERENT worker that
        // dies: a guard that drops without completing.
        let _ = engine.worker_rx.recv().expect("noop completion");

        drop(crate::engine::ReloadCompletionGuard::new(
            engine.worker_tx.clone(),
        ));
        let event = engine
            .worker_rx
            .recv()
            .expect("drop-guard must post a completion");
        let reaction = engine.process_worker_event(event);
        // The Err completion opens the reload-failed modal and closes the barrier.
        assert!(matches!(
            reaction,
            EventReaction::OpenConfigReloadFailedModal(_)
        ));
        assert!(
            !engine.reloading,
            "the barrier must close on the Err completion"
        );
        assert!(engine.reload_guard.is_none());
    }

    // -- Global worker spawn idempotence (lifecycle flip: the flipped engine
    //    arrives with these workers already running, and the other surface
    //    re-calls the spawn helpers, so a second call must NOT start a second
    //    concurrent poller). The guard flag is the observable: a long-lived
    //    poller sleeps before posting events, so counting events would be slow
    //    and flaky; the flag flips false->true on the first real spawn and a
    //    blocked second call leaves it unchanged.

    #[test]
    fn changed_files_poller_spawns_once() {
        let (engine, _tmp) = test_engine();
        assert!(
            !engine.changed_files_poller_started.load(Ordering::Relaxed),
            "guard starts false"
        );
        engine.spawn_changed_files_poller();
        assert!(
            engine.changed_files_poller_started.load(Ordering::Relaxed),
            "first spawn flips the guard"
        );
        // A second call must be a no-op (the flip re-invokes this on a live
        // engine). The guard stays set and no second poller is created.
        engine.spawn_changed_files_poller();
        assert!(
            engine.changed_files_poller_started.load(Ordering::Relaxed),
            "second call stays guarded, no second poller"
        );
    }

    #[test]
    fn branch_sync_worker_spawns_once() {
        let (mut engine, _tmp) = test_engine();
        // Ensure the poller is enabled so the guard path is exercised.
        engine.config.ui.branch_sync_interval = 30;
        assert!(!engine.branch_sync_worker_started.load(Ordering::Relaxed));
        engine.spawn_branch_sync_worker();
        assert!(
            engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "first spawn flips the guard"
        );
        engine.spawn_branch_sync_worker();
        assert!(
            engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "second call stays guarded, no second poller"
        );
    }

    #[test]
    fn branch_sync_worker_disabled_leaves_guard_unset() {
        let (mut engine, _tmp) = test_engine();
        // `0` disables the poller; nothing is spawned, so the guard stays
        // false and a later enable+re-call would still be able to start it.
        engine.config.ui.branch_sync_interval = 0;
        engine.spawn_branch_sync_worker();
        assert!(
            !engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "disabled config spawns nothing, so the guard means 'thread live' and stays false"
        );
    }

    // -- change_agent_provider (the extracted engine half of the TUI's
    //    apply_change_agent_provider) ----------------------------------------

    #[test]
    fn change_agent_provider_swaps_and_persists_when_stopped() {
        let (mut engine, _tmp) = test_engine();
        // sample_session ships with provider "claude"; swap to "codex".
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("codex"))
            .expect("swap provider");

        assert!(!outcome.running, "no PTY is running for this session");
        assert_eq!(outcome.previous.as_str(), "claude");
        // codex was never launched here, so resume is unavailable.
        assert!(!outcome.resume_available);
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        // No pin is created when nothing is running.
        assert!(engine.running_provider_pins.is_empty());

        // Persisted: a fresh load from the same SQLite file sees the new provider.
        let reloaded = engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.provider.as_str(), "codex");
    }

    #[test]
    fn change_agent_provider_reports_resume_for_previously_started_provider() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        // codex was launched here before; codex supports resume_args, so the
        // swap back should advertise resume.
        session.provider = ProviderKind::new("claude");
        session.started_providers = vec!["codex".to_string()];
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("codex"))
            .expect("swap provider");
        assert!(
            outcome.resume_available,
            "codex ran here earlier and supports resume"
        );
    }

    #[test]
    fn change_agent_provider_pins_previous_when_running() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.provider = ProviderKind::new("claude");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY so the session counts as running.
        let client = crate::pty::PtyClient::spawn_with_env(
            "cat",
            &[],
            worktree.path(),
            24,
            80,
            engine.config.ui.agent_scrollback_lines,
            &[],
        )
        .expect("spawn cat provider");
        engine.providers.insert("s1".to_string(), client);

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("codex"))
            .expect("swap provider while running");

        assert!(outcome.running, "a PTY is live for this session");
        assert_eq!(outcome.previous.as_str(), "claude");
        // The persisted provider is the new one...
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        // ...but the previously-running provider is pinned so labels stay true.
        assert_eq!(
            engine.running_provider_pins.get("s1").map(|p| p.as_str()),
            Some("claude")
        );

        // A second swap while still running must NOT overwrite the pin: the PTY
        // is still the original provider until the user relaunches.
        engine
            .change_agent_provider("s1", ProviderKind::new("gemini"))
            .expect("second swap while running");
        assert_eq!(
            engine.running_provider_pins.get("s1").map(|p| p.as_str()),
            Some("claude"),
            "the pin records what's actually spawned, not the latest selection"
        );

        // Clean up so the PTY doesn't outlive the test.
        engine.providers.remove("s1");
    }

    #[test]
    fn change_agent_provider_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .change_agent_provider("ghost", ProviderKind::new("codex"))
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn set_last_focused_tab_persists_a_live_extra_tab() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        engine
            .set_last_focused_tab("s1", Some("tab-1"))
            .expect("persist focus");

        assert_eq!(
            engine.sessions[0].last_focused_tab.as_deref(),
            Some("tab-1")
        );
        let reloaded = engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.last_focused_tab.as_deref(), Some("tab-1"));
    }

    #[test]
    fn set_last_focused_tab_normalizes_session_id_to_none() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine.set_last_focused_tab("s1", Some("tab-1")).ok();
        engine
            .set_last_focused_tab("s1", Some("s1"))
            .expect("normalize session-slot id");

        assert_eq!(engine.sessions[0].last_focused_tab, None);
        let reloaded = engine.session_store.load_sessions().expect("reload");
        assert_eq!(reloaded[0].last_focused_tab, None);
    }

    #[test]
    fn set_last_focused_tab_normalizes_a_foreign_or_unknown_tab_to_none() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let other_session = sample_session("s2", "p1", "other");
        engine.session_store.upsert_session(&other_session).unwrap();
        engine.sessions.push(other_session);
        let foreign_tab = AgentTab {
            id: "tab-2".to_string(),
            session_id: "s2".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&foreign_tab).unwrap();
        engine
            .agent_tabs
            .insert(foreign_tab.id.clone(), foreign_tab);

        engine
            .set_last_focused_tab("s1", Some("tab-2"))
            .expect("foreign tab normalizes, does not error");
        assert_eq!(engine.sessions[0].last_focused_tab, None);

        engine
            .set_last_focused_tab("s1", Some("does-not-exist"))
            .expect("unknown tab normalizes, does not error");
        assert_eq!(engine.sessions[0].last_focused_tab, None);
    }

    #[test]
    fn set_last_focused_tab_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .set_last_focused_tab("ghost", Some("tab-1"))
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }
}

#[cfg(test)]
mod tab_ops_tests {
    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};
    use crate::model::{AgentTab, SessionStatus};
    use crate::pty::PtyClient;
    use crate::worker::{AgentLaunchKind, AgentLaunchReadyData};

    fn spawn_cat(cwd: &std::path::Path) -> PtyClient {
        PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
    }

    fn support_tab(id: &str, session_id: &str, provider: &str) -> AgentTab {
        AgentTab {
            id: id.to_string(),
            session_id: session_id.to_string(),
            provider: ProviderKind::new(provider),
            sort_order: 1,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn build_tab_launch_request_resume_is_per_provider() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        // Both providers have run in this worktree, so both are resume-eligible.
        session.started_providers = vec!["codex".into(), "claude".into()];
        engine.sessions.push(session.clone());
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        let mk = |engine: &Engine| {
            engine.build_tab_launch_request(
                "tab-1".into(),
                Some(ProviderKind::new("codex")),
                session.clone(),
                true,
                (24, 80),
                AgentLaunchKind::Tab {
                    is_fresh: false,
                    status_message: "x".into(),
                },
            )
        };
        // The only codex tab coming up resumes when resume is requested.
        let req = mk(&engine);
        assert!(req.resume, "the sole codex tab resumes");
        assert_eq!(req.provider.as_str(), "codex");

        // A DIFFERENT-provider tab launching alongside does NOT block resume:
        // each CLI keeps its own directory-scoped conversation. The session-slot
        // id "s1" runs the session's own provider (claude).
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".into()));
        assert!(
            mk(&engine).resume,
            "a claude tab in flight does not block a codex tab's resume"
        );

        // A SECOND codex tab already launching makes this one start fresh.
        let other = support_tab("tab-2", "s1", "codex");
        engine.agent_tabs.insert(other.id.clone(), other);
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-2".into()));
        assert!(!mk(&engine).resume, "a second live codex tab starts fresh");
    }

    #[test]
    fn claude_and_opencode_tabs_of_one_agent_both_resume() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        // The session-slot tab is claude; an extra tab is opencode. Both providers
        // have history in the shared worktree.
        session.started_providers = vec!["claude".into(), "opencode".into()];
        engine.sessions.push(session.clone());
        let oc = support_tab("tab-oc", "s1", "opencode");
        engine.session_store.insert_agent_tab(&oc).unwrap();
        engine.agent_tabs.insert(oc.id.clone(), oc);

        // The opencode tab is already up when the claude session-slot launches.
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-oc".into()));
        let main = engine.build_agent_launch_request(
            session.clone(),
            true,
            (24, 80),
            AgentLaunchKind::Reconnect {
                status_message: "x".into(),
            },
        );
        assert!(
            main.resume,
            "claude session-slot resumes despite a live opencode tab"
        );

        // ...and the opencode tab itself resumes despite the live claude one.
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".into()));
        let oc_req = engine.build_tab_launch_request(
            "tab-oc".into(),
            Some(ProviderKind::new("opencode")),
            session,
            true,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: false,
                status_message: "x".into(),
            },
        );
        assert!(
            oc_req.resume,
            "opencode tab resumes despite a live claude session-slot"
        );
    }

    #[test]
    fn session_slot_tab_starts_fresh_beside_a_same_provider_tab() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.started_providers = vec!["claude".into()];
        engine.sessions.push(session.clone());
        // Session-slot tab (claude), no other tab live → resumes.
        let main = engine.build_agent_launch_request(
            session.clone(),
            true,
            (24, 80),
            AgentLaunchKind::Reconnect {
                status_message: "x".into(),
            },
        );
        assert!(main.resume);
        // A live SAME-provider (claude) tab makes the session-slot tab start fresh.
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-x".into()));
        let tab = support_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        let main2 = engine.build_agent_launch_request(
            session,
            true,
            (24, 80),
            AgentLaunchKind::Reconnect {
                status_message: "x".into(),
            },
        );
        assert!(
            !main2.resume,
            "a second live claude tab makes the session-slot claude tab fresh too"
        );
    }

    #[test]
    fn should_resume_session_and_tab_resume_decision_diverge_under_collision() {
        // The web session-slot reconnect toast must derive from
        // `tab_resume_decision`, NOT `should_resume_session`: with a live
        // same-provider extra tab, the latter still reports resume-eligible while
        // the actual dispatch downgrades to fresh. This locks that divergence so a
        // regression back to `should_resume_session` for the toast is caught.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.started_providers = vec!["claude".into()];
        engine.sessions.push(session.clone());
        let tab = support_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-x".into()));

        assert!(
            engine.should_resume_session(&session),
            "the session provider is resume-eligible on its own"
        );
        assert!(
            !engine.tab_resume_decision(&session, &session.id, &session.provider, true),
            "a live same-provider extra tab downgrades the session-slot launch to fresh"
        );
    }

    #[test]
    fn change_agent_provider_resume_available_matches_tab_resume_decision_under_collision() {
        // G2 regression: `change_agent_provider` used to derive resume_available
        // from `should_resume_session`, which is collision-blind. With a live
        // same-provider extra tab, it must report `false` like
        // `tab_resume_decision` (and like its sibling `change_tab_provider`).
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.started_providers = vec!["claude".into()];
        session.provider = ProviderKind::new("claude");
        engine.sessions.push(session.clone());
        let tab = support_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-x".into()));

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("claude"))
            .unwrap();

        assert!(
            engine.should_resume_session(&engine.sessions[0]),
            "the session provider is resume-eligible on its own"
        );
        assert!(
            !outcome.resume_available,
            "a live same-provider extra tab must downgrade resume_available to false"
        );
    }

    #[test]
    fn mark_session_provider_started_records_the_passed_provider() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine.mark_session_provider_started("s1", &ProviderKind::new("codex"));
        assert!(engine.sessions[0].has_started_provider(&ProviderKind::new("codex")));
        assert!(!engine.sessions[0].has_started_provider(&ProviderKind::new("opencode")));
    }

    #[test]
    fn create_tab_rejects_an_unconfigured_provider() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .create_tab(
                "s1",
                ProviderKind::new("definitely-not-a-provider"),
                (24, 80),
            )
            .unwrap_err();
        assert!(err.to_string().contains("not configured"), "err: {err}");
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 0);
    }

    #[test]
    fn create_tab_rejects_when_the_per_agent_cap_is_reached() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        // Default cap is 20 incl. Main → 19 Support rows fills it.
        for i in 0..19 {
            let tab = support_tab(&format!("t{i}"), "s1", "codex");
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(tab.id.clone(), tab);
        }
        let err = engine
            .create_tab("s1", ProviderKind::new("codex"), (24, 80))
            .unwrap_err();
        assert!(err.to_string().contains("maximum"), "err: {err}");
    }

    #[test]
    fn create_tab_refuses_while_the_session_is_closing() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        // The session is mid-deletion: its worktree is about to be removed, so a
        // fresh provider must not be spawned into it (would race remove_worktree).
        engine.closing_sessions.insert("s1".to_string());
        let err = engine
            .create_tab("s1", ProviderKind::new("codex"), (24, 80))
            .unwrap_err();
        assert!(err.to_string().contains("being deleted"), "err: {err}");
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 0);
    }

    #[test]
    fn change_tab_provider_main_delegates_to_change_agent_provider() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine
            .change_tab_provider("s1", "s1", ProviderKind::new("codex"))
            .unwrap();
        // Delegation mutates the session's own provider (the session-slot tab).
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
    }

    #[test]
    fn change_tab_provider_support_updates_only_the_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = support_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        engine
            .change_tab_provider("s1", "tab-1", ProviderKind::new("codex"))
            .unwrap();
        assert_eq!(engine.agent_tabs["tab-1"].provider.as_str(), "codex");
        // The session's own provider is untouched.
        assert_eq!(engine.sessions[0].provider.as_str(), "claude");
    }

    #[test]
    fn change_tab_provider_reports_resume_available_when_eligible() {
        // Retargeting an extra tab to a provider that has already started in
        // this worktree, and is not live/launching under any other tab, must
        // report resume_available: true rather than the old hardcoded false.
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "claude"));
        engine.mark_session_provider_started("s1", &ProviderKind::new("codex"));
        let tab = support_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        let outcome = engine
            .change_tab_provider("s1", "tab-1", ProviderKind::new("codex"))
            .unwrap();

        assert!(outcome.resume_available);
    }

    #[test]
    fn change_tab_provider_reports_no_resume_when_never_started() {
        // A provider that has never started in this worktree cannot resume,
        // even if it otherwise supports the --continue-style flag.
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "claude"));
        let tab = support_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);

        let outcome = engine
            .change_tab_provider("s1", "tab-1", ProviderKind::new("codex"))
            .unwrap();

        assert!(!outcome.resume_available);
    }

    #[test]
    fn close_tab_deletes_the_row_and_clears_runtime_maps() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine
            .pty_activity
            .insert("tab-1".to_string(), Instant::now());

        engine.close_tab("s1", "tab-1").unwrap();
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 0);
        assert!(!engine.agent_tabs.contains_key("tab-1"));
        assert!(!engine.pty_activity.contains_key("tab-1"));
    }

    #[test]
    fn close_tab_detaches_the_agent_when_it_was_the_last_live_tab() {
        // G-T5: the last-tab-detach branch of `close_tab` had no test asserting
        // the resulting session status. Seed a real live PTY for the extra tab
        // (and none for the session-slot tab) so the session starts Active, then
        // assert closing that tab flips it to Detached.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        session.status = SessionStatus::Active;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine
            .providers
            .insert("tab-1".to_string(), spawn_cat(worktree.path()));

        engine.close_tab("s1", "tab-1").unwrap();

        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Detached,
            "closing the agent's only live tab must detach it"
        );
    }

    #[test]
    fn close_tab_with_a_live_sibling_stays_active() {
        // G-T5 companion case: closing a tab while another tab of the SAME
        // agent is still live must leave the session Active, not detach it.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        session.status = SessionStatus::Active;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine
            .providers
            .insert("tab-1".to_string(), spawn_cat(worktree.path()));
        // The session-slot tab (id == "s1") is the live sibling that survives.
        engine
            .providers
            .insert("s1".to_string(), spawn_cat(worktree.path()));

        engine.close_tab("s1", "tab-1").unwrap();

        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Active,
            "a still-live sibling tab must keep the agent Active"
        );
    }

    #[test]
    fn close_tab_refuses_the_main_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        assert!(engine.close_tab("s1", "s1").is_err());
    }

    #[test]
    fn support_tab_launch_ready_does_not_flip_session_state() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(tab.id.clone(), tab);
        let session = engine.sessions[0].clone();

        let request = engine.build_tab_launch_request(
            "tab-1".into(),
            Some(ProviderKind::new("codex")),
            session,
            false,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: false,
                status_message: "x".into(),
            },
        );
        engine.process_agent_launch_ready(AgentLaunchReadyData {
            request,
            client: spawn_cat(tmp.path()),
        });

        // The extra tab's PTY is tracked under its own key...
        assert!(engine.providers.contains_key("tab-1"));
        // ...but the session stays Main-scoped: not flipped to Active.
        assert_eq!(engine.sessions[0].status, SessionStatus::Detached);
    }

    #[test]
    fn ghost_support_tab_launch_is_dropped_when_the_row_is_gone() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let session = engine.sessions[0].clone();
        // Note: NO agent_tabs row for "tab-1" — simulates a close that raced the
        // in-flight launch.
        let request = engine.build_tab_launch_request(
            "tab-1".into(),
            Some(ProviderKind::new("codex")),
            session,
            false,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: false,
                status_message: "x".into(),
            },
        );
        engine.process_agent_launch_ready(AgentLaunchReadyData {
            request,
            client: spawn_cat(tmp.path()),
        });
        // The ghost launch must not resurrect a provider under the dead tab id.
        assert!(!engine.providers.contains_key("tab-1"));
    }

    #[test]
    fn first_live_tab_returns_none_when_every_tab_is_dormant() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = support_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(tab.id.clone(), tab);

        assert_eq!(engine.first_live_tab("s1"), None);
    }

    #[test]
    fn first_live_tab_skips_a_dormant_session_slot_for_a_live_extra_tab() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = support_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        // Session-slot "s1" has no provider; the extra tab does.
        engine
            .providers
            .insert("tab-1".into(), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("tab-1".to_string()));
    }

    #[test]
    fn first_live_tab_prefers_session_slot_when_it_is_live() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = support_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine.providers.insert("s1".into(), spawn_cat(tmp.path()));
        engine
            .providers
            .insert("tab-1".into(), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("s1".to_string()));
    }

    #[test]
    fn first_live_tab_honors_sort_order_among_live_extras() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let mut earlier = support_tab("tab-2", "s1", "codex");
        earlier.sort_order = 2;
        let mut later = support_tab("tab-1", "s1", "codex");
        later.sort_order = 1;
        engine.agent_tabs.insert(earlier.id.clone(), earlier);
        engine.agent_tabs.insert(later.id.clone(), later);
        // Both are live; the lower sort_order ("tab-1") should win, not
        // insertion/HashMap order.
        engine
            .providers
            .insert("tab-2".into(), spawn_cat(tmp.path()));
        engine
            .providers
            .insert("tab-1".into(), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("tab-1".to_string()));
    }

    #[test]
    fn first_live_tab_counts_an_in_flight_launch_as_live() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = support_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch("tab-1".into()));

        assert_eq!(engine.first_live_tab("s1"), Some("tab-1".to_string()));
    }
}

#[cfg(test)]
mod resource_monitor_targets_tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::model::{AgentTab, CompanionTerminal};
    use crate::pty::PtyClient;
    use crate::worker::ResourceKind;

    fn spawn_cat(cwd: &std::path::Path) -> PtyClient {
        PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
    }

    #[test]
    fn resource_monitor_targets_is_empty_with_no_live_ptys() {
        let (engine, _tmp) = test_engine();
        assert!(
            engine.resource_monitor_targets().is_empty(),
            "no providers and no terminals means nothing to sample"
        );
    }

    #[test]
    fn resource_monitor_targets_labels_agent_tabs_and_terminals_with_ids() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        session.title = Some("fix-auth".to_string());
        engine.sessions.push(session);

        // The session-slot tab (tab id == session id) plus one extra tab.
        engine
            .providers
            .insert("s1".to_string(), spawn_cat(worktree.path()));
        engine.agent_tabs.insert(
            "tab-2".to_string(),
            AgentTab {
                id: "tab-2".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: Utc::now(),
            },
        );
        engine
            .providers
            .insert("tab-2".to_string(), spawn_cat(worktree.path()));
        engine.companion_terminals.insert(
            "term-1".to_string(),
            CompanionTerminal {
                owner: crate::model::TerminalOwner::Session("s1".to_string()),
                label: "dev server".to_string(),
                foreground_cmd: Some("npm".to_string()),
                client: spawn_cat(worktree.path()),
            },
        );

        let targets = engine.resource_monitor_targets();
        assert_eq!(targets.len(), 3, "two tabs and one terminal");

        // Every target carries the id the UI joins on, not just a label.
        let slot = targets
            .iter()
            .find(|t| t.id == "s1")
            .expect("the session-slot tab is a target keyed by session id");
        assert_eq!(slot.kind, ResourceKind::Agent);
        assert!(slot.label.contains("fix-auth"), "label: {}", slot.label);
        assert!(slot.pid > 0);

        let extra = targets
            .iter()
            .find(|t| t.id == "tab-2")
            .expect("the extra tab is a target keyed by tab id");
        assert_eq!(extra.kind, ResourceKind::Agent);
        assert!(extra.label.contains("codex"), "label: {}", extra.label);

        let terminal = targets
            .iter()
            .find(|t| t.id == "term-1")
            .expect("the companion terminal is a target keyed by terminal id");
        assert_eq!(terminal.kind, ResourceKind::Terminal);
        assert!(
            terminal.label.contains("dev server"),
            "label: {}",
            terminal.label
        );
    }

    #[test]
    fn collect_resource_stats_marks_kinds_and_ids() {
        let mut collector = crate::resource_stats::ResourceCollector::new();
        let (rows, _) = collector.sample(vec![ResourceTarget {
            id: "term-1".to_string(),
            kind: ResourceKind::Terminal,
            label: "Terminal (npm): dev server".to_string(),
            pid: std::process::id(),
        }]);

        let dux = rows
            .iter()
            .find(|r| r.kind == ResourceKind::Dux)
            .expect("the dux row is always present");
        assert_eq!(dux.id, None, "the dux row has no spine identity");

        let target = rows
            .iter()
            .find(|r| r.kind == ResourceKind::Terminal)
            .expect("the target row keeps its kind");
        assert_eq!(
            target.id.as_deref(),
            Some("term-1"),
            "the target row carries the id the UI joins on"
        );

        let total = rows
            .iter()
            .find(|r| r.kind == ResourceKind::Total)
            .expect("the total row is always last");
        assert_eq!(total.id, None, "the total row has no spine identity");
        assert_eq!(
            rows.last().map(|r| r.kind),
            Some(ResourceKind::Total),
            "the total row is pinned last"
        );
    }

    #[test]
    fn collect_resource_stats_with_empty_targets_returns_dux_and_total_only() {
        let mut collector = crate::resource_stats::ResourceCollector::new();
        let (rows, _) = collector.sample(Vec::new());
        let kinds: Vec<ResourceKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![ResourceKind::Dux, ResourceKind::Total]);
    }
}
