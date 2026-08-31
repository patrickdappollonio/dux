//! The headless `Engine`: the single owner of dux's domain state. Surfaces (the
//! TUI `App` today, the web server later) embed/drive it. In E2 it is a passive
//! state container; domain operations and workers move into `Engine` methods in E3.

pub mod command;
mod companion;
pub mod config_saver;
mod events;
mod followup;
mod in_flight;
mod lifecycle;
mod pr_sync_control;
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
    ProjectPersistenceView, RemovedBranches, StatusUpdate, WorktreeRemoval,
};
pub use followup::{FollowupOwner, WebFollowupOps, WebFollowupOpsView, owner_of_reaction};
pub use in_flight::{
    BranchRenameDispatch, BranchRenamePlan, BranchRenameRejection, InFlightKey, InFlightSet,
    RenameExpectation,
};
pub use lifecycle::{
    DeferredWorktreeRemoval, GroupWorktreeRemoval, PrunedPty, PrunedPtyKind, ShutdownReport,
    TerminatingPty, clean_exit_closes_tab_row, format_shutdown_result, format_shutdown_start,
};
pub use pr_sync_control::PrSyncControl;
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
use crate::ids::{SessionIdRef, TabId, TabIdRef};
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

/// Engine-side state of the `gh` host probe.
///
/// It lives on the engine and is passed EXPLICITLY to the things that need it,
/// rather than being a process-global any code can reach into, which is how the
/// name-based heuristic it replaces ended up duplicated.
#[derive(Clone, Debug)]
pub struct GhProbeState {
    /// Which hosts dux may name when it calls `gh`. Starts denying everything:
    /// before any probe completes no host qualifies and the GitHub features are
    /// off. Replaced atomically on a decisive result, so it is last-known-good
    /// rather than immutable for the process lifetime.
    ///
    /// Shared, because the pull-request poller is long-lived and outlives any
    /// number of probes: it snapshots this once per cycle so a re-probe reaches
    /// it, rather than being handed a copy at spawn time that could only ever
    /// be as good as the answer dux held when the poller started. Read through
    /// [`Engine::github_host_policy`]; the lock is never held across a `gh`
    /// call.
    pub policy: Arc<Mutex<crate::gh::GithubHostPolicy>>,
    /// Monotonic stamp for ordering overlapping probes. Bumped before each
    /// spawn; a result carrying an older stamp is discarded.
    pub generation: u64,
    /// The program the probe runs. `gh` in production; a test points it at a
    /// stand-in so the wiring can be exercised without a network call.
    pub program: std::ffi::OsString,
    /// When the most recent probe was SPAWNED, which is what the periodic
    /// re-check counts from. Stamped at spawn rather than at completion so a
    /// probe that is still running cannot be joined by a second one every tick,
    /// and it is wall-clock elapsed time per the animation/refresh tenet.
    /// `None` until the first probe, which the surfaces spawn at startup.
    pub last_probe_at: Option<Instant>,
    /// Set when the probe now in flight was asked for BY THE USER, so its
    /// outcome is reported even when nothing changed. A scheduled re-check that
    /// answers the same as last time says nothing; a re-check somebody pressed
    /// a button for has to answer.
    pub announce_outcome: bool,
}

impl Default for GhProbeState {
    fn default() -> Self {
        Self {
            policy: Arc::new(Mutex::new(crate::gh::GithubHostPolicy::DenyAll)),
            generation: 0,
            program: std::ffi::OsString::from("gh"),
            last_probe_at: None,
            announce_outcome: false,
        }
    }
}

/// What ONE live tab's process launched with, as far as a dropped file's path
/// is concerned. Held per tab in [`Engine::launched_drop_paste`] and projected
/// into [`crate::viewmodel::AgentTabView::drop_paste`].
///
/// The two fields travel together and are resolved together, because they answer
/// to the same question (which CLI is on the other end of this paste) and each is
/// wrong when taken from a different source: reading the form off a live process
/// while reading the length limit off current config would describe a CLI that is
/// not running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchedDropPaste {
    /// The provider NAME the tab launched as. Diagnostic only and deliberately
    /// NOT published: the browser already knows which tab a pane is showing, and
    /// the name would only invite folding these entries back onto a provider key,
    /// which is the collapse this map exists to avoid.
    pub provider: String,
    /// The form the path takes when pasted.
    pub form: crate::config::WebDragDropPaste,
    /// The FILE NAME of the command that was spawned, which is the only thing
    /// that identifies WHICH CLI is receiving the paste (see
    /// [`crate::config::ProviderCommandConfig::command_file_name`]). The
    /// provider's block name does not: it is free text.
    pub command_name: String,
}

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
    /// How many commands [`Engine::apply`] has taken since the engine was built.
    ///
    /// Read, never interpreted: the only consumer compares it against the value
    /// it saw last time to answer "did this surface change anything?". That
    /// question exists because the web layer's spine gate is driven by mutations
    /// IT can see, and the terminal UI applies commands straight to the engine
    /// through channels the web layer never observes. Counting centrally here is
    /// what lets a single call site answer for all of the TUI's apply sites at
    /// once, instead of each of them remembering to announce itself.
    ///
    /// Wraps rather than saturating, because a difference is all anyone asks for.
    pub command_applies: u64,
    /// Holds the config-writer quiesce barrier open for the lifetime of a
    /// reload. Dropped (resuming the writer) when `ConfigReloadReady` lands.
    /// Constructed as `None`.
    pub reload_guard: Option<QuiesceGuard>,
    pub providers: HashMap<TabId, PtyClient>,
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub running_provider_pins: HashMap<TabId, ProviderKind>,
    /// What each LIVE tab's process launched with, as far as a dropped file's
    /// path is concerned: the paste FORM and the COMMAND that identifies the CLI
    /// receiving it. Keyed by tab id.
    ///
    /// Keyed by TAB, not by provider name, because a provider name cannot carry
    /// the answer. Launch a tab, edit that provider's `web_dragdrop_paste`,
    /// launch another: both processes are live, both report the same provider
    /// name, and each needs the form it started with. It is published per tab in
    /// [`crate::viewmodel::AgentTabView::drop_paste`] and the browser resolves it
    /// from the pane's own tab, so the LAUNCHED profile wins for a live tab and a
    /// config edit takes effect on that tab's next launch.
    ///
    /// It also covers the case it was first written for: the user renames or
    /// deletes a `[providers.<name>]` block while a tab is still running that
    /// provider. The tab keeps reporting the name it launched as (that is what is
    /// actually on screen), so a browser looking that name up in the configured
    /// map would find nothing and fall back to `bare`, changing how a dropped path
    /// is quoted under a running agent that never changed. Keeping the launched
    /// profile with the PROCESS is the fix, and it retires when the process does,
    /// through `clear_tab_runtime`.
    ///
    /// The alternative considered was to refuse a rename or a removal while a
    /// process is live. It was rejected because `config.toml` is a file the user
    /// edits in their own editor and dux reloads: there is no point at which a
    /// refusal could be delivered, and the reload would either have to be
    /// abandoned wholesale or silently keep a block the file no longer contains.
    pub launched_drop_paste: HashMap<TabId, LaunchedDropPaste>,
    pub companion_terminals: HashMap<String, CompanionTerminal>,
    /// Persisted **extra tabs** (secondary provider tabs), keyed by tab id with
    /// the owning `session_id` carried in the value (mirrors `companion_terminals`
    /// so ownership resolves O(1) with no side index). The session-slot tab has no entry —
    /// it is reached through the session's stored pointer (see `AgentSession::slot_tab_id`). Seeded
    /// from `session_store.load_extra_agent_tabs()` at construction.
    pub agent_tabs: HashMap<TabId, AgentTab>,
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
    /// Test-only injection point for the rare synchronous worker-spawn failure
    /// (PID/RLIMIT exhaustion in production). Consumed by the next
    /// `spawn_background_worker` call, which then behaves exactly as the real
    /// failure does: it logs, clears any in-flight key, and returns
    /// `BackgroundSpawn::SpawnFailed` without starting a thread. There is no way
    /// to provoke that failure honestly from a test without exhausting the
    /// machine's process table, which this project does not do.
    #[cfg(test)]
    pub(crate) force_worker_spawn_failure: bool,
    /// The same injection point for LOOP workers, which spawn through
    /// `spawn_loop_worker` and take `&self`, hence the atomic rather than a
    /// plain bool. Consumed (reset to false) by the next loop-worker spawn,
    /// which then returns `false` without starting a thread, exactly as a real
    /// `thread::Builder::spawn` failure does. It exists because a loop worker's
    /// caller may be holding a single-instance slot that only it can release.
    #[cfg(test)]
    pub(crate) force_loop_worker_spawn_failure: AtomicBool,
    /// State owned by the `gh` host probe: which hosts qualify, the generation
    /// stamp that keeps overlapping probes ordered, and the program to run.
    pub gh_probe: GhProbeState,
    pub pr_statuses: HashMap<String, PrInfo>,
    /// Manually attached ("pinned") pull requests, keyed by session id: the
    /// in-memory mirror of the `session_pr_overrides` table. While a session
    /// has an entry here, PR sync queries ONLY the pinned PR (see
    /// [`crate::worker::PinnedPr`]) and the `PrStatusReady` identity guard
    /// drops any result that does not match the pin. Written by
    /// `AttachPullRequest`/`ClearPullRequestOverride` and loaded at boot by
    /// [`Engine::seed_pr_statuses_from_store`].
    pub pr_overrides: HashMap<String, crate::storage::StoredPr>,
    /// Sessions whose pull-request autodetection the user switched off by
    /// detaching: the in-memory mirror of the `session_pr_suppressions` table.
    /// A suppressed session is left out of the sync entries entirely, skipped
    /// by the one-shot check, and any non-pin `PrStatusReady` result for it is
    /// dropped before it can reach the store or the badge (the in-flight race
    /// guard). Written by `ClearPullRequestOverride`, `AttachPullRequest` and
    /// `ResumePullRequestAutodetection`, and loaded at boot by
    /// [`Engine::seed_pr_statuses_from_store`].
    pub pr_suppressions: HashSet<String>,
    pub branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    /// Arm state and single-instance guard for pull-request background work.
    /// See [`PrSyncControl`]: a bare `AtomicBool` could not keep the poller
    /// single-instance across a fast off-then-on.
    pub pr_sync: Arc<PrSyncControl>,
    /// Seconds between blind PR-sync safety polls, shared with the loop thread so
    /// a config reload can retune it live. `0` disables the blind poll (updates
    /// then come only from the refs watcher and foreground focus). Seeded from
    /// `config.ui.pr_poll_interval_seconds` at spawn and in `apply_reloaded_config`.
    pub pr_poll_interval_secs: Arc<AtomicU64>,
    /// Seconds between branch-sync sweeps, shared with the loop thread so a
    /// config reload can retune it live. `0` reaching the loop means "nap and
    /// look again", never "exit": the thread stays live so
    /// `branch_sync_worker_started` keeps meaning "a thread is live". Seeded
    /// from `config.ui.branch_sync_interval` at spawn and in
    /// [`Engine::retune_after_config_swap`].
    pub branch_sync_interval_secs: Arc<AtomicU64>,
    /// Observation points on the branch-sync wait, shared with the loop thread.
    pub branch_sync_wait: Arc<BranchSyncWait>,
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
    pub resume_fallback_candidates: HashMap<TabId, Instant>,
    /// Session IDs whose worktree is currently being removed by a background
    /// worker. Prevents duplicate delete requests from spawning a second
    /// worker while the first is still running; also drives the dimmed
    /// visual cue on the left pane row so the user can see the in-flight
    /// state.
    pub pending_deletions: HashSet<String>,
    /// The live repository verdict for each STANDALONE agent's folder, keyed by
    /// session id. A managed agent never has an entry: its worktree is a
    /// repository by construction.
    ///
    /// The verdict is decided here, on the engine, rather than re-derived by
    /// each surface, because it shells out to git and both surfaces would
    /// otherwise ask the same question at different moments and disagree. It is
    /// filled by a background probe (never on the engine thread) at load and at
    /// creation, refreshed when the changes panel opens, and carried on the
    /// wire so the browser renders the same answer the server acted on.
    ///
    /// An absent entry means "not probed yet", which every reader must treat as
    /// [`crate::git::FolderRepoStatus::Indeterminate`]: quiet, honest, and no
    /// mutations. Read it through [`Engine::folder_repo_status`], never
    /// directly, so that default can never be forgotten.
    pub folder_repo_statuses: HashMap<String, crate::git::FolderRepoStatus>,
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
    /// Tracks when a POINTER report (a wheel notch or a click) was last
    /// forwarded to each PTY, and for how long it suppresses. Separate from
    /// `pty_input` because the two answer opposite questions about the same
    /// byte: a forwarded wheel must NEVER read as the user typing (design
    /// tenet: selecting or scrolling a terminal is not typing), but it DOES
    /// make the child repaint, and a repaint the user asked for is not the
    /// agent working. While an entry here is fresh
    /// [`Engine::is_agent_streaming`] stops inferring "working" from output
    /// text and defers to the agent's own `OSC 9;4` report. The window is
    /// carried per entry rather than being one constant because a scroll and a
    /// click suppress for very different lengths of time (see
    /// [`pointer_suppression_window`]). Stamped through
    /// [`Engine::note_pty_write`] by both surfaces. Invariant, exactly as for
    /// `pty_input`: cleared wherever `pty_activity` is cleared, so the maps
    /// never drift; the teardown tests pin it.
    pub pty_pointer: HashMap<String, PointerStamp>,
    /// Tabs (keyed by tab id) that have raised a "needs attention" signal that
    /// has not yet been looked at. Memory-only runtime state, never persisted —
    /// like `working`/`has_output`, it does not survive a restart, by tenet.
    /// Populated by [`Engine::poll_agent_signals`] (which suppresses a signal on
    /// a tab the user is engaged with) and cleared when the user looks at or
    /// tears down the tab. The sidebar rolls this up across an agent's tabs.
    pub needs_attention: HashSet<TabId>,
    /// Tabs (keyed by tab id) whose LAST run ended badly: a launch that failed
    /// outright, or a process that exited non-zero. Published per tab as
    /// [`crate::viewmodel::AgentTabView::last_run_failed`], where it is what
    /// stops a surface from launching a dormant tab on selection alone: a tab
    /// that keeps failing (a resume against a conversation that isn't there, a
    /// provider that is no longer on PATH) would otherwise relaunch every time
    /// the user looks at it, with no way out but to look somewhere else.
    ///
    /// Deliberately NOT part of [`Engine::clear_tab_runtime`]: every other
    /// tab-keyed map describes a LIVE process and dies with it, while this one
    /// exists precisely to outlive the process it describes. It is cleared when
    /// a launch is actually dispatched for the tab (any launch is somebody
    /// asking for one, so the next failure is a fresh verdict) and forgotten
    /// when the tab's row goes away.
    ///
    /// Memory-only, like `needs_attention`: after a restart every tab comes back
    /// dormant with a clean slate, which is what makes a restart a way out of a
    /// tab that was failing before it.
    pub failed_tab_runs: HashSet<TabId>,
    /// The most recent `OSC 9;4` progress report per tab (keyed by tab id), with
    /// the moment the engine observed it. [`Engine::is_agent_streaming`] treats a
    /// fresh report as authoritative for the "working" indicator, overriding the
    /// output-activity heuristic; a stale report (older than
    /// [`PROGRESS_AUTHORITY_WINDOW`]) is ignored and the heuristic resumes.
    /// Memory-only; dropped on tab teardown so a crashed agent can never leave a
    /// spinner stuck on.
    pub pty_progress: HashMap<TabId, ProgressReport>,
    /// Wall-clock timestamp of when the user was last actively looking at each
    /// tab (keyed by tab id): the TUI stamps the focused-interactive agent tab
    /// each tick; the web stamps a tab on PTY subscribe (opening its live view).
    /// A fresh entry (within [`ATTENTION_ENGAGED_WINDOW`]) both suppresses a new
    /// attention signal and clears an existing one, so an agent you are already
    /// looking at never nags you. Typing is handled separately via `pty_input`.
    pub agent_viewed: HashMap<TabId, Instant>,
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
    /// Manual PR-attach ops (the "Resolving PR to attach…" busy). SHARED by
    /// both surfaces, because the whole resolve→attach flow completes
    /// engine-side: the busy is minted in
    /// [`Engine::dispatch_attach_pull_request`] and the final (attached, or the
    /// failure) is resolved in `process_worker_event`'s `PullRequestResolved`
    /// attach arm. One keyed op spans resolve→attach; the `AttachPullRequest`
    /// wire command itself mints no second busy.
    pub pending_pr_attach_ops: HashMap<String, HandlerStatusOp<PrAttachOutcome>>,
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
    /// `drive_web_launch_followup` against a [`LaunchOutcome`]. Empty for the
    /// TUI, which keeps its own op in the App layer.
    pub pending_web_launch_ops: HashMap<String, HandlerStatusOp<LaunchOutcome>>,

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

/// Handler-computed outcome for a reconnect / force-restart launch op, shared by
/// BOTH surfaces (the web's [`Engine::pending_web_launch_ops`] and the TUI's
/// `App::pending_reconnect_ops`). The resolver maps it to the final user message
/// via [`launch_outcome_final`], the ONE mapper both surfaces call so the wording
/// cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
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

/// Map a [`LaunchOutcome`] to the final wording shared by web and TUI reconnect
/// operations.
pub fn launch_outcome_final(o: &LaunchOutcome) -> Final {
    match o {
        LaunchOutcome::Ready { status_message } => Final::info(status_message.clone()),
        LaunchOutcome::ReconnectFailed {
            branch_name,
            message,
        } => Final::error(format!(
            "Reconnect failed for agent \"{branch_name}\": {message}"
        )),
        LaunchOutcome::ForceReconnectFailed {
            branch_name,
            message,
        } => Final::error(format!(
            "Fresh restart failed for agent \"{branch_name}\": {message}"
        )),
        LaunchOutcome::Missing => Final::clear(),
    }
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

/// Handler-computed outcome for a manual PR-attach op (both surfaces).
pub enum PrAttachOutcome {
    /// The lookup resolved and the pin was applied; `message` is the
    /// already-formatted confirmation from [`Engine::apply_pr_attach`].
    Attached { message: String },
    /// The lookup failed, the session vanished mid-resolve, or applying the
    /// pin failed; `message` is the already-formatted error line.
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

/// How long after a WHEEL notch is forwarded to a PTY the output-activity
/// heuristic stays suppressed for it. A child with mouse reporting on repaints
/// its whole grid in response, and that repaint is output the USER caused, not
/// work the agent started, so reading it as "working" lights the indicator for
/// the entire time somebody scrolls. While this window is open the working
/// decision defers to the agent's own `OSC 9;4` progress report instead (see
/// [`Engine::is_agent_streaming`]).
///
/// Be precise about what that deferral buys, because only some providers can
/// answer: Claude Code and Copilot emit `OSC 9;4` progress, and Codex and
/// OpenCode do not (see `website/docs/attention-indicators.md`). For the
/// providers that report, a busy agent keeps reading busy while it is
/// scrolled. For the ones that do not, a genuinely busy agent reads IDLE for
/// the length of this window, which is the price of not lighting the indicator
/// every time somebody scrolls an idle one.
///
/// Same length as [`AGENT_INPUT_SUPPRESSION_WINDOW`] and for the same reason:
/// it must comfortably outlast the trailing repaint of the last notch, while a
/// scroll that has stopped hands the heuristic back promptly. Wall-clock, per
/// the design tenet.
pub const POINTER_REPAINT_WINDOW: Duration = Duration::from_millis(1250);

/// The same suppression after a BUTTON press or release (a click, a phone tap,
/// the start or end of a selection drag), and deliberately much shorter than
/// [`POINTER_REPAINT_WINDOW`].
///
/// A scroll is continuous: notches keep arriving and the child keeps repainting,
/// so its window has to outlast the gap between notches. A click is one
/// discrete act that causes one repaint and is over. It is also the gesture
/// most likely to START work (clicking a menu entry, confirming a prompt), and
/// a user who clicks and then watches for the Working cue should see it appear
/// promptly rather than after more than a second of a blank row. Half a second
/// covers the repaint a click provokes with room to spare while keeping that
/// wait short. Wall-clock, per the design tenet.
pub const POINTER_CLICK_REPAINT_WINDOW: Duration = Duration::from_millis(500);

/// How long a forwarded pointer report suppresses the output-activity
/// heuristic, or `None` if it suppresses nothing at all.
///
/// MOTION suppresses nothing, and that is a deliberate choice rather than a
/// user instruction, so it should be easy to reverse if it proves wrong: a move
/// is not a discrete user action, an app using any-event tracking (DECSET 1003)
/// emits one for every cell the pointer crosses, and arming a window on each of
/// them would keep a genuinely busy agent reading idle for as long as somebody
/// drifted the mouse across its pane. Nobody asked for that.
///
/// Pure, so both surfaces and the tests agree without an engine.
pub fn pointer_suppression_window(report: crate::pty::PointerReport) -> Option<Duration> {
    match report {
        crate::pty::PointerReport::Wheel => Some(POINTER_REPAINT_WINDOW),
        crate::pty::PointerReport::Button => Some(POINTER_CLICK_REPAINT_WINDOW),
        crate::pty::PointerReport::Motion => None,
    }
}

/// A forwarded pointer report: when it reached the PTY, and for how long it
/// suppresses the working inference. The window travels with the stamp because
/// a wheel notch and a click suppress for different lengths of time and the
/// map holds both kinds (see [`pointer_suppression_window`]).
#[derive(Debug, Clone, Copy)]
pub struct PointerStamp {
    /// When the report was forwarded.
    pub at: Instant,
    /// How long from `at` the suppression lasts.
    pub window: Duration,
}

impl PointerStamp {
    /// Whether the suppression this stamp armed is still in force.
    pub fn is_fresh(&self) -> bool {
        self.at.elapsed() < self.window
    }
}

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
    viewed: &HashMap<TabId, Instant>,
    // `pty_input` spans the whole PTY keyspace (tabs AND companion terminals),
    // so it stays string-keyed and is probed with the tab id's raw string.
    typed: &HashMap<String, Instant>,
    tab_id: &TabIdRef,
    now: Instant,
) -> bool {
    viewed
        .get(tab_id)
        .is_some_and(|t| now.duration_since(*t) < ATTENTION_ENGAGED_WINDOW)
        || typed
            .get(tab_id.as_str())
            .is_some_and(|t| now.duration_since(*t) < AGENT_INPUT_SUPPRESSION_WINDOW)
}

/// The pure per-tick attention decision behind [`Engine::poll_agent_signals`].
/// Given the tabs whose attention signal fired this tick (`fired`, already gated by the
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
    needs_attention: &mut HashSet<TabId>,
    fired: &[TabId],
    enabled: bool,
    engaged: impl Fn(&TabIdRef) -> bool,
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

/// The distinctive phrase every "an attach is already resolving for this
/// agent" refusal carries. The engine is the authority on the refusal; the
/// surfaces match on this phrase to classify it (the web REST routes turn it
/// into a `409 CONFLICT`, the way they classify "unknown session" as a `404`),
/// so it lives here rather than being retyped per surface.
pub const PR_ATTACH_IN_FLIGHT_MARKER: &str = "already being attached";

/// The refusal a session's pull-request operations give while a manual attach
/// is still resolving for it. Actionable on purpose: the wait is bounded,
/// because every attach ends in a success or a failure that unblocks the
/// agent.
pub(crate) fn pr_attach_in_flight_message(agent_name: &str) -> String {
    format!(
        "A pull request is {PR_ATTACH_IN_FLIGHT_MARKER} to agent \"{agent_name}\". Wait for \
         that attach to finish or fail, then try again."
    )
}

/// What git dux may do for one agent, resolved in a single engine round trip so
/// a route cannot ask half the question and act on the other half.
///
/// The three-way split IS the design: the branch-identity features (push, pull,
/// fork, pull requests, branch rename, provenance, the worktree manager) are
/// AGENT driven and never exist for a standalone agent whatever its folder
/// contains, while the changes panel is FOLDER driven and works whenever the
/// folder is itself a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionGitAccess {
    /// A managed working copy: the whole git surface, as always.
    Full { worktree: PathBuf },
    /// A standalone agent whose folder IS a repository's top level: the changes
    /// panel works here exactly as anywhere, and nothing branch-shaped exists.
    ChangesOnly { directory: PathBuf },
    /// A standalone agent with no repository to work in: no branch features and
    /// a quiet changes region. `quiet_reason` says why, in the user's terms,
    /// and never "the repository is busy".
    NoRepository {
        directory: PathBuf,
        quiet_reason: &'static str,
    },
}

impl SessionGitAccess {
    /// The directory this agent occupies, whichever answer applies.
    pub fn directory(&self) -> &Path {
        match self {
            Self::Full { worktree } => worktree,
            Self::ChangesOnly { directory } | Self::NoRepository { directory, .. } => directory,
        }
    }

    /// Whether the changes panel shows a real repository view.
    pub fn changes_panel_works(&self) -> bool {
        match self {
            Self::Full { .. } | Self::ChangesOnly { .. } => true,
            Self::NoRepository { .. } => false,
        }
    }

    /// Whether staging, unstaging, discarding and committing are allowed.
    /// Identical to [`Self::changes_panel_works`] today and kept as its own
    /// question because they are different ones: a future read-only repository
    /// view would show files it must not let you stage.
    pub fn mutations_allowed(&self) -> bool {
        match self {
            Self::Full { .. } | Self::ChangesOnly { .. } => true,
            Self::NoRepository { .. } => false,
        }
    }

    /// Whether the branch-identity features exist for this agent.
    pub fn supports_branch_git(&self) -> bool {
        match self {
            Self::Full { .. } => true,
            Self::ChangesOnly { .. } | Self::NoRepository { .. } => false,
        }
    }

    /// Why the changes region is quiet, or `None` when it is not.
    pub fn quiet_reason(&self) -> Option<&'static str> {
        match self {
            Self::Full { .. } | Self::ChangesOnly { .. } => None,
            Self::NoRepository { quiet_reason, .. } => Some(quiet_reason),
        }
    }
}

/// The refusal every branch-identity git feature gives for a standalone agent.
///
/// Purposeful, not accidental: hiding a button is not an answer when the same
/// action is an HTTP route and a palette command, so the refusal has to be a
/// sentence that explains the shape of the thing rather than a git error about
/// a repository nobody named. `feature` names the action in the user's terms
/// ("push", "attach a pull request", "fork"), and `remedy` says what to do
/// instead, because "no" with no way forward is where a user gets stuck.
pub fn standalone_agent_refusal(agent_name: &str, feature: &str, remedy: &str) -> String {
    format!(
        "Agent \"{agent_name}\" is a standalone agent: it runs in a folder you chose and has \
         no branch of its own, so there is nothing to {feature}. {remedy}"
    )
}

/// The remedy sentence shared by the branch-identity refusals: adding the
/// folder as a project is the shape dux is built for, and it brings the branch
/// features (and tabs) along.
pub const STANDALONE_ADD_AS_PROJECT_REMEDY: &str =
    "Add its folder as a project if you want dux to manage branches and worktrees for it.";

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

/// Default sleep granularity of the branch-sync wait, for the same reason as
/// [`PR_SYNC_SLICE_SECS`]: a retuned or disabled interval must be observed in a
/// few seconds, not after a full one.
pub const BRANCH_SYNC_SLICE_MS: u64 = 3_000;

/// The branch-sync wait's tunable granularity and its progress counter.
///
/// `slice_ms` is the sleep granularity; `waits_started` counts waits the loop
/// has begun. Both exist so a test can shrink the slice and then retune only
/// once the loop is provably waiting on the old interval, which is the whole
/// claim: a running loop adopts a new interval mid-wait.
#[derive(Debug)]
pub struct BranchSyncWait {
    pub slice_ms: AtomicU64,
    pub waits_started: AtomicU64,
}

impl Default for BranchSyncWait {
    fn default() -> Self {
        Self {
            slice_ms: AtomicU64::new(BRANCH_SYNC_SLICE_MS),
            waits_started: AtomicU64::new(0),
        }
    }
}

/// What the branch-sync loop naps for while its interval is `0`.
const BRANCH_SYNC_IDLE_NAP_SECS: u64 = 60;

/// How long one branch-sync wait lasts. A `0` interval naps instead of ending
/// the thread, so a later retune to N is picked up by this same loop.
fn branch_sync_nap_secs(interval_secs: u64) -> u64 {
    if interval_secs == 0 {
        BRANCH_SYNC_IDLE_NAP_SECS
    } else {
        interval_secs
    }
}

/// Whether a completed branch-sync wait ends in a sweep or in another nap.
fn branch_sync_should_poll(interval_secs: u64) -> bool {
    interval_secs != 0
}

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
    /// `persist_projects_to_config`); deferring them is still correct so that
    /// mirror runs against the reloaded project set rather than a stale one.
    /// `ReloadConfig` / `RecoverConfig` drive the barrier themselves and are
    /// deliberately excluded. Provider/theme/pane-width saves are surface
    /// (TUI App) handlers that currently write `config.toml` directly (not through
    /// `Engine::config_writer`), so they are NOT covered by this deferral nor by
    /// the writer's quiesce backstop, and a save from those paths during a reload
    /// is unguarded.
    fn is_config_mutating(cmd: &Command) -> bool {
        matches!(
            cmd,
            Command::PersistGlobalEnv { .. }
                | Command::UpdateMacros { .. }
                | Command::PersistProject { .. }
                | Command::RemoveProject { .. }
                | Command::DeleteProject { .. }
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
        // session id. An agent's own key is whatever `AgentSession::slot_tab_id`
        // points at, never the session id read off the record.
        for (tab_id, provider) in &self.providers {
            if provider.take_received_data() {
                self.pty_activity.insert(tab_id.as_str().to_string(), now);
            }
        }
        // Companion terminals share the same activity map, keyed by their
        // terminal id (`term-N`), which is disjoint from tab ids. Their PtyClient
        // sets `received_data` on visible grid change exactly like an agent's, so
        // the same consuming read drives a terminal's "working" indicator through
        // `is_agent_streaming`.
        for (terminal_id, terminal) in &self.companion_terminals {
            if terminal.client.take_received_data() {
                self.pty_activity.insert(terminal_id.clone(), now);
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

    /// Record that a forwarded POINTER report just reached this PTY. This is
    /// deliberately NOT `note_pty_input`: a pointer report must never light the
    /// Typing state, but it does make the child repaint, and that repaint must
    /// not be mistaken for the agent working. See [`Engine::is_agent_streaming`].
    ///
    /// How long it suppresses depends on the gesture, and motion suppresses
    /// nothing, so a `Motion` report stamps no entry at all (see
    /// [`pointer_suppression_window`]).
    pub fn note_pty_pointer(&mut self, id: &str, report: crate::pty::PointerReport) {
        if let Some(window) = pointer_suppression_window(report) {
            self.note_pty_pointer_window(id, window);
        }
    }

    /// Stamp the pointer window directly, for a caller that already resolved
    /// the window (the TUI folds several reports from one input drain into the
    /// longest of their windows before stamping once).
    pub fn note_pty_pointer_window(&mut self, id: &str, window: Duration) {
        self.pty_pointer.insert(
            id.to_string(),
            PointerStamp {
                at: Instant::now(),
                window,
            },
        );
    }

    /// Classify a batch of bytes a surface just wrote to a PTY and stamp the
    /// window it belongs to, if any. The single entry point both surfaces use,
    /// so the TUI and the web can never disagree about what a wheel report is.
    /// Returns the classification for callers that want to log or assert on it.
    pub fn note_pty_write(&mut self, id: &str, bytes: &[u8]) -> crate::pty::PtyWriteKind {
        let kind = crate::pty::classify_pty_write(bytes);
        match kind {
            crate::pty::PtyWriteKind::Typing => self.note_pty_input(id),
            crate::pty::PtyWriteKind::Pointer(report) => self.note_pty_pointer(id, report),
            crate::pty::PtyWriteKind::Ignored => {}
        }
        kind
    }

    /// Whether a pointer report was forwarded to this PTY recently enough that
    /// any output it provoked is a REPAINT the user asked for, not the agent
    /// producing work. While this is true the working decision stops inferring
    /// anything from output text and defers to the agent's own progress report.
    ///
    /// "Recently enough" is per stamp, not one constant: a wheel notch holds
    /// this true for [`POINTER_REPAINT_WINDOW`] and a click for the much
    /// shorter [`POINTER_CLICK_REPAINT_WINDOW`].
    pub fn recent_pointer_input(&self, id: &str) -> bool {
        self.pty_pointer.get(id).is_some_and(PointerStamp::is_fresh)
    }

    /// Returns whether the tab's agent should read as "working". Priority: real
    /// PTY OUTPUT wins over everything, then the agent's own OSC 9;4 progress
    /// report, then idle.
    ///
    /// - Rendered output within [`AGENT_STREAMING_WINDOW`] → working. `pty_activity`
    ///   is stamped only on real content changes in the terminal's ACTIVE AREA
    ///   (see `TerminalState::take_content_change`, which hashes the active area
    ///   rather than the displayed viewport precisely so a scrolled-back operator
    ///   still sees a producing agent as working), so this is genuine agent
    ///   output, not an OSC status
    ///   sequence. It overrides the OSC report everywhere: an agent that misreports
    ///   "idle" (or stopped reporting) while still printing must still read as
    ///   working. There are two exceptions, and both are output the USER caused:
    ///   the terminal echoing keystrokes within [`AGENT_INPUT_SUPPRESSION_WINDOW`],
    ///   and the REPAINT a child answers a forwarded pointer report with. A child
    ///   that owns the mouse redraws its whole grid for every wheel notch, so
    ///   without the second exception the mere act of scrolling an idle agent lit
    ///   the working indicator for as long as the user scrolled. How long that
    ///   second exception lasts depends on the gesture: [`POINTER_REPAINT_WINDOW`]
    ///   for a wheel notch, the much shorter [`POINTER_CLICK_REPAINT_WINDOW`] for a
    ///   click or tap, and nothing at all for pointer motion (see
    ///   [`pointer_suppression_window`]).
    /// - No fresh (non-echo) output → fall back to a fresh OSC 9;4 progress report,
    ///   if any. A stale report (older than [`PROGRESS_AUTHORITY_WINDOW`]) grants no
    ///   authority, so a crashed agent that stopped reporting can't stick it on.
    /// - Otherwise idle.
    ///
    /// Keyed by TAB id (see `poll_pty_activity`), not session id.
    pub fn is_agent_streaming(&self, tab_id: &str) -> bool {
        // TEXT WINS: a visible content change is the ground truth that the agent is
        // producing work, and it overrides the agent's own OSC progress claims.
        // Discount only output that is the terminal echoing recent keystrokes.
        let streaming = self
            .pty_activity
            .get(tab_id)
            .is_some_and(|t| t.elapsed() < AGENT_STREAMING_WINDOW);
        let typing = self
            .pty_input
            .get(tab_id)
            .is_some_and(|t| t.elapsed() < AGENT_INPUT_SUPPRESSION_WINDOW);
        // A forwarded pointer report makes the child repaint on demand, so while
        // one is fresh the output text says nothing about whether the agent is
        // working and the decision defers to the agent's own report below.
        let pointer = self.recent_pointer_input(tab_id);
        if streaming && !typing && !pointer {
            return true;
        }
        // No fresh (non-echo, non-repaint) output: consult the agent's own
        // OSC 9;4 report.
        if let Some(report) = self.pty_progress.get(TabIdRef::new(tab_id))
            && report.at.elapsed() < PROGRESS_AUTHORITY_WINDOW
        {
            return report.working;
        }
        // No output and no fresh report: assume idle.
        false
    }

    /// Whether the user has forwarded keystrokes to this PTY within
    /// [`AGENT_INPUT_SUPPRESSION_WINDOW`], i.e. it is currently "typing". This is
    /// the sole source of the Typing state and works for both tab ids and
    /// terminal ids (both stamp `pty_input` via [`Engine::note_pty_input`]).
    /// A forwarded pointer report is NOT typing and never reaches this map; it
    /// stamps `pty_pointer` instead (see [`Engine::note_pty_write`]).
    /// It is exactly the suppression predicate `is_agent_streaming` uses to void
    /// streaming, surfaced so the ViewModel can render a distinct Typing cue.
    pub fn is_typing(&self, id: &str) -> bool {
        self.pty_input
            .get(id)
            .is_some_and(|t| t.elapsed() < AGENT_INPUT_SUPPRESSION_WINDOW)
    }

    /// Whether a companion terminal is "busy" (its Working cue). Unlike an agent,
    /// a terminal is busy in two cases: it is streaming output right now
    /// (`is_agent_streaming`), OR a foreground app is running in it even while
    /// quiet. `PtyClient::foreground_process_name` returns `None` when the shell
    /// itself owns the terminal foreground (an idle prompt) and `Some(app)` once a
    /// real command runs, so a set `foreground_cmd` means an app is running. Typing
    /// takes precedence: while the user is typing into the terminal it reads as
    /// Typing, not Working, matching how `is_agent_streaming` voids streaming
    /// during input. Returns false for an unknown id.
    ///
    /// Scrolling deliberately suppresses only the FIRST of the two cases. The
    /// output half is an INFERENCE from repaint text, and a repaint the user's
    /// own wheel provoked is no evidence at all, so `is_agent_streaming`
    /// discounts it (see [`POINTER_REPAINT_WINDOW`]). The foreground-app half is
    /// a FACT read off the kernel: a `vim` that repaints because somebody
    /// scrolled it is still `vim` running, so scrolling must not hide it.
    /// Suppressing that half too would make every terminal with a real process
    /// in it flicker to Idle the moment the user scrolled to read its output.
    pub fn terminal_is_working(&self, terminal_id: &str) -> bool {
        if self.is_typing(terminal_id) {
            return false;
        }
        if self.is_agent_streaming(terminal_id) {
            return true;
        }
        self.companion_terminals
            .get(terminal_id)
            .and_then(|t| t.foreground_cmd.as_deref())
            .is_some_and(|cmd| !cmd.is_empty())
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
        let mut progress_updates: Vec<(TabId, ProgressReport)> = Vec::new();
        let mut fired: Vec<TabId> = Vec::new();
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
        let engaged = |tab: &TabIdRef| attention_engaged(viewed, typed, tab, now);
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
    ///
    /// Takes a plain `&str`: this is a transport-facing entry point and the id
    /// arrives unclassified from a client. It is named as a tab id here, at the
    /// door, which is where an unknown-kind id is supposed to be classified.
    pub fn note_agent_viewed(&mut self, tab_id: &str) {
        self.agent_viewed.insert(TabId::new(tab_id), Instant::now());
        self.needs_attention.remove(TabIdRef::new(tab_id));
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
        self.needs_attention.contains(TabIdRef::new(tab_id))
    }

    /// Whether this tab's LAST run ended badly (a failed launch, or a non-zero
    /// exit). See [`Engine::failed_tab_runs`] for what the answer is for.
    pub fn tab_last_run_failed(&self, tab_id: &str) -> bool {
        self.failed_tab_runs.contains(TabIdRef::new(tab_id))
    }

    /// Record that this tab's last run ended badly. Called from the two places
    /// that observe a bad ending: the launch-failed event and the non-zero-exit
    /// prune.
    pub fn mark_tab_run_failed(&mut self, tab_id: &TabIdRef) {
        self.failed_tab_runs.insert(tab_id.to_owned());
    }

    /// Forget a tab's recorded failure: a launch has been dispatched for it, so
    /// whatever happens next is the verdict that counts.
    pub fn clear_tab_run_failure(&mut self, tab_id: &TabIdRef) {
        self.failed_tab_runs.remove(tab_id);
    }

    /// Whether ANY tab of the session (session-slot or extra) currently needs
    /// attention. The any-tab rollup the sidebar row uses, mirroring how
    /// `working` rolls up. Cheap: `needs_attention` is usually empty, so this
    /// short-circuits without scanning tabs.
    pub fn session_needs_attention(&self, session_id: &str) -> bool {
        if self.needs_attention.is_empty() {
            return false;
        }
        if self
            .needs_attention
            .contains(self.slot_tab_id_of(SessionIdRef::new(session_id)))
        {
            return true;
        }
        self.agent_tabs
            .values()
            .any(|t| t.session_id == session_id && self.tab_needs_attention(&t.id))
    }

    /// Whether ANY tab of the session (session-slot or extra) is currently
    /// streaming output. The any-tab rollup the sidebar row's "working" spinner
    /// uses, mirroring `session_needs_attention` and the viewmodel's `working`
    /// field — so an agent whose non-slot tab is streaming still reads as working.
    pub fn session_is_streaming(&self, session_id: &str) -> bool {
        if self.is_agent_streaming(self.slot_tab_id_of(SessionIdRef::new(session_id)).as_str()) {
            return true;
        }
        self.agent_tabs
            .values()
            .any(|t| t.session_id == session_id && self.is_agent_streaming(&t.id))
    }

    /// Whether ANY tab of the session (session-slot or extra) is currently being
    /// typed into. The any-tab rollup mirroring `session_is_streaming`, so the
    /// sidebar row can show a Typing cue whenever the user is typing into any of
    /// the agent's tabs.
    pub fn session_is_typing(&self, session_id: &str) -> bool {
        if self.is_typing(self.slot_tab_id_of(SessionIdRef::new(session_id)).as_str()) {
            return true;
        }
        self.agent_tabs
            .values()
            .any(|t| t.session_id == session_id && self.is_typing(&t.id))
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
    /// that the path exists, is a git repository root (not a folder inside
    /// one, and not git's internal directory), and is not already registered.
    /// Returns the canonicalized path on success or a user-facing error
    /// string on failure.
    ///
    /// This gate stops new interactive adds only. Already-registered projects
    /// and config/SQLite-sourced entries load through `load_projects`, whose
    /// looser `is_git_repo` probe is deliberately untouched so existing
    /// subfolder projects keep working.
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
        // Fail-open on Indeterminate (a gate, per the CommitState doctrine);
        // WorkTreeRoot and BareRoot pass (bare adds are shipped behavior).
        // NotARepo is unreachable after the is_git_repo check above.
        match crate::git::repo_path_kind(&path) {
            crate::git::RepoPathKind::InsideWorkTree { root } => {
                return Err(format!(
                    "\"{}\" is inside the git repository at \"{}\". Add \"{}\" instead.",
                    path.display(),
                    root.display(),
                    root.display()
                ));
            }
            crate::git::RepoPathKind::InsideGitDir { .. } => {
                return Err(format!(
                    "\"{}\" is inside git's internal directory. Add the repository itself instead.",
                    path.display()
                ));
            }
            _ => {}
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

    /// Validate a raw path string before initializing it as a brand-new git
    /// repository and registering it as a project. The folder must exist, must
    /// not already be (or sit inside) a git repository, and must not already
    /// be registered. Unlike the add gate, `Indeterminate` fails closed here:
    /// this validation front-runs a mutation (`git init`).
    pub fn validate_project_init_path(
        &self,
        raw_path: &str,
    ) -> std::result::Result<PathBuf, String> {
        let trimmed = raw_path.trim();
        let path = match PathBuf::from(trimmed).canonicalize() {
            Ok(path) if path.is_dir() => path,
            _ => {
                return Err(format!("\"{trimmed}\" is not an existing folder."));
            }
        };
        match crate::git::repo_path_kind(&path) {
            crate::git::RepoPathKind::NotARepo => {}
            crate::git::RepoPathKind::WorkTreeRoot | crate::git::RepoPathKind::BareRoot => {
                return Err(format!(
                    "\"{}\" is already a git repository. Use Add project instead.",
                    path.display()
                ));
            }
            crate::git::RepoPathKind::InsideWorkTree { root } => {
                return Err(format!(
                    "\"{}\" is inside the git repository at \"{}\". Add \"{}\" instead.",
                    path.display(),
                    root.display(),
                    root.display()
                ));
            }
            crate::git::RepoPathKind::InsideGitDir { .. } => {
                return Err(format!(
                    "\"{}\" is inside git's internal directory. Add the repository itself instead.",
                    path.display()
                ));
            }
            crate::git::RepoPathKind::Indeterminate => {
                return Err(format!(
                    "couldn't determine whether \"{}\" is already a git repository, so refusing to initialize one there.",
                    path.display()
                ));
            }
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
        let github_was_enabled = self.github_integration_enabled;
        self.github_integration_enabled = config.ui.github_integration;
        if self.github_integration_enabled
            && (!github_was_enabled || !matches!(self.gh_status, crate::model::GhStatus::Available))
        {
            // Off-to-on through a config reload is the same transition as the
            // toggle, and needs the same fresh answer from `gh`.
            //
            // A reload while the integration was already on re-probes too, but
            // only when `gh` is not currently usable: reloading the config is a
            // deliberate act and one of the reasons to perform it is having just
            // fixed `gh`, so waiting out the timer would be needless. A reload
            // with everything working stays a no-op, because there is nothing to
            // recover and a probe costs a process.
            self.spawn_gh_status_check();
        }
        self.projects = crate::project_browser::load_projects(
            &self.session_store.load_projects()?,
            &self.session_store.load_project_created_ats()?,
            &config,
        );
        self.config = config;
        self.retune_after_config_swap();
        self.refresh_project_defaults();
        self.update_branch_sync_sessions();
        Ok(())
    }

    /// Re-point the live machinery at `self.config` after a reload swapped it
    /// in: the log level, and both poll loops. The loops re-read their shared
    /// interval on every wait and the branch-sync spawn is idempotent, so this
    /// also covers a reload that turns branch sync on from `0`. Each surface
    /// applies a reload its own way, so both call this rather than relying on
    /// the other's path.
    pub fn retune_after_config_swap(&mut self) {
        crate::logger::set_level(&self.config.logging.level);
        self.pr_poll_interval_secs.store(
            u64::from(crate::config::normalized_pr_poll_interval(
                self.config.ui.pr_poll_interval_seconds,
            )),
            Ordering::Relaxed,
        );
        self.spawn_branch_sync_worker();
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
        let configured = self.config.ui.branch_sync_interval;
        // Seeded even on the disabled path so a running loop from an earlier,
        // enabled config sees the `0` and naps instead of sweeping.
        self.branch_sync_interval_secs
            .store(u64::from(configured), Ordering::Relaxed);
        if configured == 0 {
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
        let interval_secs = Arc::clone(&self.branch_sync_interval_secs);
        let wait = Arc::clone(&self.branch_sync_wait);
        let sessions = Arc::clone(&self.branch_sync_sessions);
        self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "branch-sync".into(),
            },
            move |tx| {
                let secs = interval_secs.load(Ordering::Relaxed);
                let nap_ms = branch_sync_nap_secs(secs).saturating_mul(1_000);
                let mut slept_ms = 0u64;
                wait.waits_started.fetch_add(1, Ordering::Relaxed);
                while slept_ms < nap_ms {
                    let slice = wait
                        .slice_ms
                        .load(Ordering::Relaxed)
                        .max(1)
                        .min(nap_ms - slept_ms);
                    thread::sleep(Duration::from_millis(slice));
                    slept_ms += slice;
                    if interval_secs.load(Ordering::Relaxed) != secs {
                        // Retuned (including 0<->N): restart the wait on the new
                        // value rather than finishing one measured for the old.
                        return LoopControl::Continue;
                    }
                }
                if !branch_sync_should_poll(secs) {
                    return LoopControl::Continue;
                }
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
                    // The watch exists to notice the AGENT's branch moving, and
                    // a standalone agent has no agent branch. Skipped even when
                    // its folder happens to be a repository: watching it would
                    // fire pull-request checks for a branch that is not dux's.
                    let Some(managed) = session.workspace.as_managed() else {
                        continue;
                    };
                    let refs_dir = PathBuf::from(&managed.worktree_path)
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

    /// The configured `gh` re-check interval, clamped.
    ///
    /// The tick calls this, so the clamp is deliberately silent: an out-of-range
    /// value is warned about and corrected once, where the config was taken in
    /// (see [`crate::config::correct_github_probe_interval`]), which leaves
    /// nothing for this to say however often it is asked.
    pub fn github_probe_interval(&self) -> Duration {
        Duration::from_secs(
            crate::config::normalized_github_probe_interval(
                self.config.ui.github_probe_interval_secs,
            )
            .into(),
        )
    }

    /// Re-ask `gh` when the periodic re-check is due. Called once per tick by
    /// whichever surface is driving the engine (the terminal UI's run loop, or
    /// the web's maintenance sweep), the same way the PTY activity poll is.
    ///
    /// The decision is [`crate::gh::gh_reprobe_is_due`]; this only supplies the
    /// clock and spawns the worker, so nothing about `gh` is ever run on a UI
    /// thread and a wedged `gh` is bounded by the probe's own timeout.
    pub fn poll_gh_probe_schedule(&mut self) {
        if !crate::gh::gh_reprobe_is_due(
            self.gh_status,
            self.github_integration_enabled,
            self.gh_probe.last_probe_at.map(|at| at.elapsed()),
            self.github_probe_interval(),
        ) {
            return;
        }
        crate::logger::info(&format!(
            "[gh-integration] re-checking gh (status is {:?}); dux retries every {}s while \
             GitHub features are unavailable",
            self.gh_status,
            self.github_probe_interval().as_secs(),
        ));
        self.spawn_gh_status_check();
    }

    /// Re-ask `gh` right now because the user said so, from the TUI palette or
    /// the web app menu.
    ///
    /// Returns the sentence to show. Restarting dux is often not an option (it
    /// would take every running agent with it), so this is the way out of a
    /// paused integration that does not involve waiting for the timer. It is
    /// gated on the integration being enabled and NOT on the current status: a
    /// gesture that vanishes exactly when it would help is not a way back.
    pub fn request_gh_recheck(&mut self) -> StatusUpdate {
        if !self.github_integration_enabled {
            return StatusUpdate::warning(
                "The GitHub integration is turned off, so there is nothing to re-check. \
                 Turn it on first (ui.github_integration), and dux asks gh straight away.",
            )
            .with_key(crate::engine::events::GH_AVAILABILITY_STATUS_KEY);
        }
        self.gh_probe.announce_outcome = true;
        self.spawn_gh_status_check();
        StatusUpdate::info(
            "Re-checking the GitHub CLI: running gh auth status now, and the result \
             replaces this message when it lands.",
        )
        .with_key(crate::engine::events::GH_AVAILABILITY_STATUS_KEY)
    }

    /// Ask `gh` which hosts it can serve, on a worker.
    ///
    /// This is the same call the surfaces already make at startup to decide
    /// whether the GitHub features are available at all, so the startup case
    /// costs no extra process; it now keeps the parsed hosts instead of reducing
    /// the answer to a yes or a no. It is RE-RUN on every off-to-on transition of
    /// the GitHub integration (both toggles, both config reload paths), because
    /// an enterprise user who enables the integration after starting dux would
    /// otherwise be stuck with whatever the value was at boot.
    pub fn spawn_gh_status_check(&mut self) {
        // Not guarded against re-spawn: this is a one-shot job, and a fresh check
        // picks up any `gh login` the user did in the meantime. Overlapping runs
        // are made safe by the generation stamp rather than by a guard.
        if !self.github_integration_enabled {
            return;
        }
        // Stamped BEFORE the spawn, and carried on every way this probe can
        // finish, including the synthesised result of a panicking worker.
        self.gh_probe.generation = self.gh_probe.generation.wrapping_add(1);
        self.gh_probe.last_probe_at = Some(Instant::now());
        let generation = self.gh_probe.generation;
        let program = self.gh_probe.program.clone();
        let spawn = self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "gh-status-check".into(),
                in_flight_key: None,
                // The primitive logs a panic at error level before this event is
                // built, so an OBSOLETE probe panicking still writes an error
                // line even though the generation guard then discards its
                // result. That is deliberate. Moving the logging into the
                // generation-aware handler would make the one worker whose
                // result may be discarded also the one whose crashes are
                // invisible, and it would mean adding a per-caller knob to a
                // primitive eleven sites share. A panic is a defect in dux's own
                // code and it really happened; the generation stamp governs
                // which ANSWER wins, not which events were true. The handler
                // logs the discard at debug level so the pair reads correctly in
                // `dux.log`.
                panic_event: Some(Box::new(move |reason| WorkerEvent::GhStatusChecked {
                    generation,
                    // A panic decided nothing, so it is reported as transient:
                    // the last known good host set stands, and a first probe
                    // that panics still moves the status off Unknown.
                    outcome: crate::gh::GhProbe::Transient(format!(
                        "gh host probe panicked: {reason}"
                    )),
                })),
            },
            move |tx| {
                let _ = tx.send(WorkerEvent::GhStatusChecked {
                    generation,
                    outcome: crate::gh::probe_github_hosts(&program),
                });
            },
        );
        if spawn != BackgroundSpawn::Spawned {
            // The primitive documents that a spawn it did not make produces NO
            // completion event and needs the caller to recover. Without this the
            // generation above is burned (so any older queued answer is
            // discarded) with nothing arriving to replace it, and a FIRST probe
            // would leave the status stuck on Unknown, which the interface
            // renders as neither available nor unavailable. Reported as
            // transient, through the normal channel, so it passes the same
            // generation guard as any other way this probe can finish.
            let _ = self.worker_tx.send(WorkerEvent::GhStatusChecked {
                generation,
                outcome: crate::gh::GhProbe::Transient(
                    "could not start the gh host probe worker".to_string(),
                ),
            });
        }
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
    /// (off-thread), and so does the TUI's `refresh-changes` command, which a
    /// locked repository would otherwise freeze; the TUI's selection-driven
    /// `reload_changed_files` still computes inline on its own App thread.
    ///
    /// - `None` (or an UNKNOWN id) → clear the watch and the lists, return `None`.
    /// - `Some(id)` for a known session → watch its worktree, record the id, empty
    ///   the lists, and return `Some(worktree)` to compute changed files for.
    ///
    /// For a STANDALONE agent the watch is folder-driven: it is enrolled only
    /// while the folder is itself a repository, so nothing ever polls a plain
    /// folder (which would answer with an error every cycle and surface as
    /// "the repository is busy"). Opening the panel is also the moment the
    /// folder verdict is refreshed, so a folder that became a repository since
    /// the last look starts working here.
    #[must_use]
    pub fn set_watched_session(&mut self, session_id: Option<&str>) -> Option<PathBuf> {
        // Refresh first: the probe is off-thread, so this call still uses the
        // previous verdict and `FolderRepoStatusReady` re-enrols the watch when
        // the answer changes. That is the whole "noticed when the panel opens"
        // behavior, without a git subprocess on the engine thread.
        if let Some(id) = session_id {
            self.spawn_folder_repo_probe(id);
        }
        let resolved = session_id.and_then(|id| {
            let session = self.sessions.iter().find(|s| s.id == id)?;
            match &session.workspace {
                crate::model::AgentWorkspace::Managed(managed) => {
                    Some((id.to_string(), PathBuf::from(&managed.worktree_path)))
                }
                crate::model::AgentWorkspace::Folder(folder) => self
                    .folder_repo_status(id)
                    .changes_panel_works()
                    .then(|| (id.to_string(), PathBuf::from(&folder.folder_path))),
            }
        });
        let worktree = resolved.as_ref().map(|(_, path)| path.clone());
        // Keep the background poller in sync with the watched worktree.
        if let Ok(mut guard) = self.watched_worktree.lock() {
            *guard = worktree.clone();
        }
        // The WATCHED SESSION is whichever session the panel is showing, even
        // when there is no repository to poll in it. Dropping the id here
        // instead would leave the previous session's id in place and let its
        // files render under a standalone agent's panel, which is the exact
        // cross-tab confusion this field exists to prevent.
        self.watched_session_id = session_id
            .filter(|id| self.sessions.iter().any(|s| &s.id == id))
            .map(str::to_string);
        // Always clear so the pane shows "no changes yet" (never the previous
        // watch's stale files) until the off-thread/inline compute lands.
        self.staged_files = Vec::new();
        self.unstaged_files = Vec::new();
        worktree
    }

    /// The live repository verdict for a standalone agent's folder.
    ///
    /// A MANAGED agent answers [`crate::git::FolderRepoStatus::WorkingRepo`]:
    /// its worktree is a repository by construction, so callers that only want
    /// "may the changes panel work here" can ask this for any session without
    /// first sorting out which kind it is.
    ///
    /// An unprobed standalone folder answers
    /// [`crate::git::FolderRepoStatus::Unprobed`], not "no repository": dux has
    /// not looked yet, and saying anything more definite would let a mutation
    /// through on a guess. It gates exactly as `Indeterminate` does and reads
    /// as a wait rather than as a fault, which matters because a freshly
    /// created agent in a healthy repository spends a moment in this state.
    ///
    /// An unknown id keeps `Indeterminate`: there is no folder to still be
    /// looking at.
    pub fn folder_repo_status(&self, session_id: &str) -> crate::git::FolderRepoStatus {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return crate::git::FolderRepoStatus::Indeterminate;
        };
        match &session.workspace {
            crate::model::AgentWorkspace::Managed(_) => crate::git::FolderRepoStatus::WorkingRepo,
            crate::model::AgentWorkspace::Folder(_) => self
                .folder_repo_statuses
                .get(session_id)
                .copied()
                .unwrap_or(crate::git::FolderRepoStatus::Unprobed),
        }
    }

    /// Ask git, OFF the engine thread, what a standalone agent's folder is, and
    /// post the answer back as [`WorkerEvent::FolderRepoStatusReady`].
    ///
    /// A no-op for a managed agent and for an unknown id: neither has a folder
    /// whose repository-ness can change under dux. `repo_path_kind` runs up to
    /// four git subprocesses, so this must never run inline; a folder on a
    /// stalled network mount would otherwise freeze every web client.
    ///
    /// Also a no-op while a probe for the same agent is already running. Every
    /// question about the folder asks for a refresh, and the web's
    /// changed-files poller asks every two seconds, so without the guard this
    /// was an unbounded loop of threads and git subprocesses for as long as a
    /// standalone agent's changes panel stayed open. One probe in flight is
    /// enough: its answer is what the next question reads.
    pub fn spawn_folder_repo_probe(&mut self, session_id: &str) {
        let Some(folder) = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.folder_path())
            .map(PathBuf::from)
        else {
            return;
        };
        let session_id = session_id.to_string();
        let key = InFlightKey::FolderRepoProbe(session_id.clone());
        if self.is_in_flight(&key) {
            return;
        }
        self.mark_in_flight(key);
        let label = format!("folder-repo-probe:{session_id}");
        let probed_session = session_id.clone();
        let started = self.spawn_loop_worker(LoopWorkerSpec { label }, move |tx| {
            let status = crate::git::folder_repo_status(&folder);
            let _ = tx.send(WorkerEvent::FolderRepoStatusReady {
                session_id: session_id.clone(),
                status,
            });
            // One-shot: the next question about the folder asks again, and the
            // in-flight key above is what stops that becoming a loop.
            LoopControl::Break
        });
        if !started {
            self.release_folder_repo_probe(&probed_session);
        }
    }

    /// Give the probe's single-instance slot back after a spawn that never
    /// started.
    ///
    /// [`Self::spawn_loop_worker`] deliberately does not clear in-flight keys
    /// itself (its own doc says so), and this key is otherwise cleared only by
    /// the `FolderRepoStatusReady` handler, which a thread that never ran can
    /// never post. One failed spawn would therefore pin the key for the rest of
    /// the run: the folder's git surface keeps answering `Unprobed`, the changes
    /// panel stays quiet, every mutation is refused, the upload seed is
    /// withheld, and only a restart heals it. Releasing the key re-arms the
    /// probe instead, so the next ask (the changed-files poll, a couple of
    /// seconds away while the panel is open) tries again.
    fn release_folder_repo_probe(&mut self, session_id: &str) {
        self.clear_in_flight(&InFlightKey::FolderRepoProbe(session_id.to_string()));
        crate::logger::warn(&format!(
            "could not start the folder classification for agent {session_id}, so dux does not \
             know yet whether its folder is a git repository; the next look will try again"
        ));
    }

    /// How many runtime PTYs are alive: one per launched provider tab plus one
    /// per companion terminal. This is the definition of "something is running"
    /// for the whole app, and it lives here so both surfaces answer it the same
    /// way instead of each re-deriving the expression.
    pub fn running_process_count(&self) -> usize {
        self.providers.len() + self.companion_terminals.len()
    }

    /// Keep [`Self::has_active_processes`] in step with the live PTY count.
    ///
    /// That flag is what [`Self::spawn_changed_files_poller`] reads to pick its
    /// cadence (2s while something runs, 10s while nothing does), so whichever
    /// surface owns the loop has to call this once per iteration, AFTER the work
    /// that starts and ends processes. Only the TUI used to do it; `dux server`
    /// never stored to the flag at all and so polled on the idle cadence with
    /// every agent in the workspace running.
    pub fn sync_has_active_processes(&self) {
        self.has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);
    }

    /// Compute the changed files for `worktree` OFF the engine actor thread and
    /// post them back as a `ChangedFilesReady` event. The one-shot worker mirrors
    /// `spawn_pr_check_for_session`'s spawn shape and the changed-files poller's
    /// git call. The event carries the `worktree` it was computed for, so the
    /// `ChangedFilesReady` drain in `process_worker_event` automatically drops a
    /// result whose watch has since moved (the 4faf872 stale-poll guard).
    ///
    /// A `git::changed_files` error rides along as `Err`: the drain leaves the
    /// lists untouched rather than emptying them, so a locked or unreadable
    /// repository never renders as a clean worktree, and a surface waiting on
    /// this refresh (the TUI's `refresh-changes` command) can report the failure
    /// as a failure. Both surfaces call this right after `set_watched_session`.
    pub fn spawn_changed_files_refresh(&self, worktree: PathBuf) {
        let label = format!("changed-files-refresh:{}", worktree.display());
        self.spawn_loop_worker(LoopWorkerSpec { label }, move |tx| {
            let outcome = crate::git::changed_files(&worktree).map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::ChangedFilesReady {
                outcome,
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
                            // The poller only sends what it managed to read: a
                            // transient failure is skipped entirely (the `let
                            // Ok` above), so it never reports one.
                            outcome: Ok((staged, unstaged)),
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

    /// List the worktrees the MANAGER may act on for a project (managed, not
    /// the project checkout, dirtiness included).
    ///
    /// Separate from [`Self::spawn_project_worktrees_worker`], which feeds the
    /// adopt picker: that one wants the whole classification (external
    /// worktrees and the project checkout included) and does not pay for a
    /// `git status` per worktree.
    pub fn spawn_manageable_worktrees_worker(
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
                label: format!("manageable-worktrees:{}", project.id),
                in_flight_key: None,
                panic_event: Some(Box::new(move |reason| {
                    WorkerEvent::ManageableWorktreesReady {
                        project_id: project_id_for_panic,
                        result: Err(format!("Worktree-manager worker panicked: {reason}")),
                        status_op_id: status_op_id_for_panic,
                    }
                })),
            },
            move |tx| {
                let result =
                    crate::worktree_manager::list_manageable_worktrees(&project, &paths, &sessions);
                let _ = tx.send(WorkerEvent::ManageableWorktreesReady {
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

    /// Arm pull-request work and start its poller, at most once.
    ///
    /// The poller is EXPLICITLY single-instance: [`PrSyncControl::arm`] claims a
    /// slot and this returns without spawning when one is already live. Without
    /// that, every off-to-on transition created another permanent poller, because
    /// the old one reads its kill switch only once per [`PR_SYNC_SLICE_SECS`] and
    /// a fast off-then-on lands inside that window.
    pub fn spawn_pr_sync_worker(&self) {
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let control = Arc::clone(&self.pr_sync);
        let interval_secs = Arc::clone(&self.pr_poll_interval_secs);
        // Seed the shared interval from config so the first iteration honors it,
        // and arm BEFORE spawning so the kill switch observes the live state on
        // the first iteration.
        interval_secs.store(
            u64::from(crate::config::normalized_pr_poll_interval(
                self.config.ui.pr_poll_interval_seconds,
            )),
            Ordering::Relaxed,
        );
        if !control.arm() {
            // A poller is already live and has just been told to keep going.
            return;
        }
        let control_for_loop = Arc::clone(&control);
        let backoff = Arc::clone(&self.pr_backoff);
        // The SHARED policy, not a snapshot: this loop outlives any number of
        // probes, so it must see a re-probe's answer rather than the one dux
        // held when it started.
        let policy = Arc::clone(&self.gh_probe.policy);
        let spawned = self.spawn_loop_worker(
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
                    if !control_for_loop.poller_should_continue() {
                        // Releases the slot in the same critical section, so a
                        // later enable gets a replacement and a simultaneous
                        // one keeps this loop alive instead.
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
                let policy = policy.lock().unwrap_or_else(|e| e.into_inner()).clone();
                // The blind poll: nobody asked, so a dormant agent whose pull
                // request is already closed is left alone until a one-shot
                // trigger looks at it.
                let (results, signals) = crate::gh::run_pr_sync(
                    &sessions,
                    &snapshot,
                    &policy,
                    crate::gh::SyncTrigger::BlindPoll,
                );
                Self::apply_pr_backoff(&backoff, &signals, tx);
                if !results.is_empty() && tx.send(WorkerEvent::PrStatusReady(results)).is_err() {
                    // Receiver dropped (shutdown). Release the slot anyway, so a
                    // surface flip that reuses the engine can start a fresh one.
                    control_for_loop.poller_stopped();
                    return LoopControl::Break;
                }
                LoopControl::Continue
            },
        );
        if !spawned {
            // The thread never started, so nothing will ever release the slot
            // and pull-request polling would be dead for the process lifetime.
            control.poller_stopped();
        }
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

    /// A snapshot of which hosts dux may name when it calls `gh`.
    ///
    /// Handed EXPLICITLY to each of the three places a host can enter (a
    /// project's configured address, a pull-request reference the user types,
    /// and a host remembered from a previous pull request) rather than reached
    /// for as a process-global, which is how the name-based heuristic it
    /// replaces ended up duplicated in the first place.
    pub fn github_host_policy(&self) -> crate::gh::GithubHostPolicy {
        self.gh_probe
            .policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the host policy. Only the probe's completion handler does this.
    pub fn set_github_host_policy(&self, policy: crate::gh::GithubHostPolicy) {
        *self
            .gh_probe
            .policy
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Stop pull-request background work: clear the arm flag so the live poller
    /// ends its loop on its next slice. Called on an explicit disable and on a
    /// DECISIVE `gh` answer that no host works, so work armed from an older,
    /// better answer cannot keep running while the interface says GitHub is
    /// unavailable.
    pub fn disarm_pr_sync(&self) {
        self.pr_sync.disarm();
    }

    pub fn spawn_initial_pr_refresh(&mut self) {
        self.pr_sync.note_refresh();
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let backoff = Arc::clone(&self.pr_backoff);
        let policy = self.github_host_policy();
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
                // Boot is a one-shot: a pull request closed before the last
                // shutdown may have been reopened since, and this is the pass
                // that notices.
                let (results, signals) = crate::gh::run_pr_sync(
                    &sessions,
                    &snapshot,
                    &policy,
                    crate::gh::SyncTrigger::OneShot,
                );
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
        // `providers` is keyed by tab id. Iterate it so every live tab's process
        // is a target, not just one per session.
        for (tab_id, pty) in &self.providers {
            let Some(pid) = pty.child_process_id() else {
                continue;
            };
            if let Some(session) = self.session_for_slot_tab(tab_id) {
                // session-slot tab.
                let title = session.display_label();
                let provider = self.running_provider_for(session);
                targets.push(ResourceTarget {
                    id: tab_id.as_str().to_string(),
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
                    .map(|s| s.display_label())
                    .unwrap_or_else(|| tab.session_id.clone());
                let provider = self
                    .running_provider_pins
                    .get(tab_id)
                    .cloned()
                    .unwrap_or_else(|| tab.provider.clone());
                targets.push(ResourceTarget {
                    id: tab_id.as_str().to_string(),
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
    /// single-session check as [`Self::spawn_pr_check_for_session`] but with the
    /// tighter [`PR_FOREGROUND_DEBOUNCE`] window, so focusing an agent shows
    /// fresh data.
    ///
    /// It carries the FOCUS trigger, which is what keeps tabbing down a sidebar
    /// of finished agents from spawning a `gh` process per agent passed: a
    /// terminal pull request on an exited agent is answered from SQLite here and
    /// re-queried on the deliberate triggers instead.
    pub fn spawn_foreground_pr_check(&mut self, session_id: &str) {
        self.spawn_pr_check_for_session_with(
            session_id,
            PR_FOREGROUND_DEBOUNCE,
            crate::gh::SyncTrigger::Focus,
        );
    }

    /// Trigger a single-session PR check for a deliberate event (a refs change,
    /// an agent exit, the user asking), unless it was checked more recently than
    /// `min_interval` ago. Those pass [`PR_CHECK_MIN_INTERVAL`]; foreground focus
    /// goes through [`Self::spawn_foreground_pr_check`] instead, which carries
    /// both the tighter [`PR_FOREGROUND_DEBOUNCE`] and its own sync trigger.
    ///
    /// The timestamp is recorded BEFORE the worker thread is spawned so a burst
    /// of triggers within a single event-loop tick — e.g. several callers each
    /// invoking this for the same session before the first worker's
    /// `PrStatusReady` event has been processed — does not bypass the
    /// rate-limit and spawn N concurrent `gh` subprocesses.
    /// Seed `pr_statuses` from the persisted `latest_prs` rows so both startups
    /// (the TUI and `dux serve`) show PR badges immediately, before the first
    /// network poll and even when `gh` is unavailable. A no-op when GitHub
    /// integration is off. The "OPEN"/"MERGED"/"CLOSED" decode is the shared
    /// `gh::reconstruct_pr_from_stored`, so the mapping lives in one place.
    pub fn seed_pr_statuses_from_store(&mut self) {
        if !self.github_integration_enabled {
            return;
        }
        // Load the durable detaches FIRST: a suppressed session's stored
        // `session_prs` row is history, not a badge, and re-badging it here
        // would make a restart quietly undo the user's detach.
        self.pr_suppressions = self
            .session_store
            .load_pr_suppressions()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let stored = self.session_store.load_all_latest_prs().unwrap_or_default();
        for pr in stored {
            if self.pr_suppressions.contains(&pr.session_id) {
                continue;
            }
            if let Some(info) = crate::gh::reconstruct_pr_from_stored(&pr) {
                self.pr_statuses.insert(pr.session_id, info);
            }
        }
        // A manually attached PR wins over the `session_prs` latest: the latest
        // row may be a different (even higher-numbered) autodetected PR, and
        // the badge must show the pin. Loading the map here is also what arms
        // the `PrStatusReady` identity guard and the pinned sync planning from
        // boot (and again on the integration re-arm paths, which re-call this).
        // No suppression check here: a pin and a suppression cannot coexist
        // (attaching lifts the suppression, detaching removes the pin), and a
        // pin is a manual association the user asked for, not autodetection.
        for pinned in self.session_store.load_pr_overrides().unwrap_or_default() {
            if let Some(info) = crate::gh::reconstruct_pr_from_stored(&pinned) {
                self.pr_statuses.insert(pinned.session_id.clone(), info);
            }
            self.pr_overrides.insert(pinned.session_id.clone(), pinned);
        }
        if !self.pr_statuses.is_empty() {
            crate::logger::info(&format!(
                "[gh-integration] seeded {} PR statuses from database",
                self.pr_statuses.len(),
            ));
        }
    }

    pub fn spawn_pr_check_for_session(&mut self, session_id: &str, min_interval: Duration) {
        self.spawn_pr_check_for_session_with(
            session_id,
            min_interval,
            crate::gh::SyncTrigger::OneShot,
        );
    }

    /// [`Self::spawn_pr_check_for_session`] with the trigger named. Only the
    /// foreground check passes anything but `OneShot`; see [`crate::gh::SyncTrigger`].
    fn spawn_pr_check_for_session_with(
        &mut self,
        session_id: &str,
        min_interval: Duration,
        trigger: crate::gh::SyncTrigger,
    ) {
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
        // The user detached this agent's pull request, so there is nothing to
        // detect for it. Checked before the debounce stamp so a resume gets a
        // genuinely immediate check rather than one the skipped calls pushed
        // out. `update_pr_sync_sessions` drops the session from the batched
        // loop for the same reason; this is the one-shot half.
        if self.pr_suppressions.contains(session_id) {
            return;
        }
        // A standalone agent has no branch, so there is no pull request to
        // check for. Refused HERE rather than only in the batched enumerator
        // because the refs-watcher event routes straight into this one-shot,
        // and refused BEFORE the debounce stamp below so a skipped agent never
        // records a check that did not happen.
        if !self
            .sessions
            .iter()
            .any(|s| s.id == session_id && s.supports_branch_git())
        {
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
        // A pinned session checks against its PIN, exactly like the batched
        // loop: the override row is the known PR and the pin identity rides
        // along so the one-shot check queries the pinned repo only. Without
        // this, a one-shot racing an attach could answer for the remote-derived
        // repo (the identity guard would drop it, but there is no reason to
        // spend the call).
        let pinned_row = self.pr_overrides.get(session_id).cloned();
        let known_pr = pinned_row.clone().or_else(|| {
            self.session_store
                .load_prs(session_id)
                .ok()
                .and_then(|prs| prs.into_iter().next())
        });
        // The workspace gate above already refused a standalone id, so a
        // managed workspace is guaranteed here; reading it is what keeps the
        // entry from being built with an empty branch name.
        let Some(managed) = session.workspace.as_managed() else {
            return;
        };
        let entry = PrSyncEntry {
            session_id: session.id.clone(),
            branch_name: managed.branch_name.clone(),
            worktree_path: managed.worktree_path.clone(),
            known_pr,
            agent_exited: !self.providers.contains_key(session.slot_tab_id()),
            pinned: pinned_row.as_ref().map(pinned_pr_from_stored),
        };
        let label = format!("pr-check:{}", entry.session_id);
        let backoff = Arc::clone(&self.pr_backoff);
        let policy = self.github_host_policy();
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
                let (result, signals) =
                    crate::gh::check_pr_for_entry(&entry, &snapshot, &policy, trigger);
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
            // A standalone agent has no branch to keep in step with, and
            // its folder may not be a repository at all, so it is never
            // enrolled: `filter_map` over the branch identity is the gate and
            // the projection in one, so there is no arm that could enrol one
            // with an empty branch name.
            *guard = self
                .sessions
                .iter()
                .filter_map(|s| {
                    let managed = s.workspace.as_managed()?;
                    Some(BranchSyncEntry {
                        session_id: s.id.clone(),
                        worktree_path: managed.worktree_path.clone(),
                        branch_name: managed.branch_name.clone(),
                    })
                })
                .collect();
        }
    }

    /// Core-owned first half of a branch rename: validate the requested name,
    /// enforce the single-rename overlap guard, write the display title
    /// optimistically (and persist it), then decide whether a git branch
    /// rename is actually needed. When it is, stash the expected branches so
    /// the branch-sync poller can tell the user's own in-progress rename from
    /// unrelated external drift, and return the parameters the surface hands to
    /// `git::rename_branch` in its own background worker.
    ///
    /// This is the single decision both the TUI and a future web rename
    /// consume, so validation, no-op detection, the optimistic write, and the
    /// expectation stash cannot drift between surfaces. The engine deliberately
    /// does NOT mark the rename in-flight here — that marker belongs to the
    /// worker spawn (`BackgroundWorkerSpec::in_flight_key`), so a surface that
    /// never dispatches (title-only or no-op) leaves no dangling marker.
    ///
    /// The surface still owns everything presentation-shaped: the keyed status
    /// wording, the worker dispatch and its completion event, the list rebuild,
    /// and, on a synchronous spawn failure, the unwind via
    /// `revert_optimistic_rename`.
    pub fn prepare_branch_rename(
        &mut self,
        session_id: &str,
        new_name: &str,
        rename_branch: bool,
    ) -> BranchRenamePlan {
        let name = new_name.trim().to_string();
        if name.is_empty() {
            return BranchRenamePlan::Rejected(BranchRenameRejection::EmptyName);
        }
        // The refname rules apply only when the name really does become a git
        // branch. A STANDALONE agent's name is a label: creation takes it
        // verbatim precisely because folder names legally contain spaces, dots
        // and punctuation a ref cannot, so applying the ref validator here made
        // a title dux itself had minted impossible to type back, and clearing it
        // a one-way door (the fallback label is that same folder name).
        //
        // Non-empty, checked above, is the whole rule for a folder: every row
        // label falls back through a branch name it does not have, so a
        // nameless agent is what must not be allowed.
        if self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.supports_branch_git())
            .unwrap_or(true)
            && !crate::git::is_valid_agent_name(&name)
        {
            return BranchRenamePlan::Rejected(BranchRenameRejection::MalformedName);
        }
        // Block overlapping renames: a second concurrent `git branch -m` on the
        // same worktree would race the first (and could corrupt the in-flight
        // drift-suppression bookkeeping). Mirror the CreateAgent busy-guard.
        if self.is_in_flight(&InFlightKey::BranchRename(session_id.to_string())) {
            return BranchRenamePlan::Rejected(BranchRenameRejection::AlreadyInFlight);
        }

        // Capture the previous title before mutating, in case a failed branch
        // rename has to revert it.
        let previous_title = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.title.clone());

        // Always update the display title immediately (optimistic write).
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.title = Some(name.clone());
            session.updated_at = Utc::now();
        }
        if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
            let _ = self.session_store.upsert_session(session);
        }

        if !rename_branch {
            // Title-only change: the branch stays, but branch-sync display
            // should refresh (matches the pre-extraction `else` arm).
            return BranchRenamePlan::TitleWritten {
                name,
                sync_branches: true,
            };
        }

        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            // The session vanished before we could resolve its branch (the
            // optimistic write above also found nothing). Nothing to dispatch.
            return BranchRenamePlan::Noop;
        };
        // A standalone agent has no branch, so renaming it is a title
        // change and nothing more. The caller already wrote the title
        // optimistically above, so there is simply nothing to dispatch.
        let Some(managed) = session.workspace.as_managed() else {
            return BranchRenamePlan::TitleWritten {
                name,
                sync_branches: false,
            };
        };
        let old_branch = managed.branch_name.clone();
        if name == old_branch {
            // The branch already carries this name: only the title changed, and
            // there is nothing to sync.
            return BranchRenamePlan::TitleWritten {
                name,
                sync_branches: false,
            };
        }
        let worktree_path = managed.worktree_path.clone();

        // Stash the expected branches so `BranchSyncReady` can distinguish our
        // own in-progress rename (silently skip) from an unrelated external
        // change landing mid-rename (log it). Cleared alongside the in-flight
        // marker in `BranchRenameCompleted`, or by `revert_optimistic_rename`
        // on a spawn failure.
        self.rename_expected.insert(
            session_id.to_string(),
            RenameExpectation {
                old_branch: old_branch.clone(),
                new_branch: name.clone(),
            },
        );

        BranchRenamePlan::RenameBranch(BranchRenameDispatch {
            session_id: session_id.to_string(),
            worktree_path,
            old_branch,
            new_branch: name,
            previous_title,
        })
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
                // A detached session is left out of the plan entirely: dux was
                // told there is no pull request here, so it neither asks
                // GitHub nor has anything to answer with.
                .filter(|s| !self.pr_suppressions.contains(&s.id))
                // A standalone agent has no branch to open a pull request
                // from, so it is never enrolled. Reading the branch identity
                // out of the workspace is the gate and the projection at once.
                .filter_map(|s| {
                    let managed = s.workspace.as_managed()?;
                    // A pinned session syncs against its PIN: the override row
                    // is the known PR (the `session_prs` latest can be a
                    // different, autodetected PR) and the pin identity rides
                    // along so the planner queries the pinned repo only.
                    let pinned_row = self.pr_overrides.get(&s.id);
                    Some(PrSyncEntry {
                        session_id: s.id.clone(),
                        branch_name: managed.branch_name.clone(),
                        worktree_path: managed.worktree_path.clone(),
                        known_pr: pinned_row
                            .cloned()
                            .or_else(|| known_map.get(&s.id).cloned()),
                        agent_exited: !self.providers.contains_key(s.slot_tab_id()),
                        pinned: pinned_row.map(pinned_pr_from_stored),
                    })
                })
                .collect();
        }
    }

    /// Apply a manual pull-request attachment: persist the override row, mirror
    /// it in memory, show the badge immediately, and re-derive the sync snapshot
    /// so the next cycle queries the pin. Shared by the `AttachPullRequest` wire
    /// command and the `PullRequestResolved` attach handler, so both surfaces
    /// apply the exact same mutation. Returns the user-facing confirmation.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_pr_attach(
        &mut self,
        session_id: &str,
        host: &str,
        owner_repo: &str,
        number: u64,
        title: &str,
        state: &str,
        url: &str,
    ) -> anyhow::Result<String> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        // The stored state column is constrained to the three values
        // `reconstruct_pr_from_stored` understands; anything else would seed a
        // row the zero-network reconstruction silently drops.
        if !matches!(state, "OPEN" | "MERGED" | "CLOSED") {
            anyhow::bail!(
                "cannot attach PR #{number}: \"{state}\" is not a state dux can track \
                 (expected OPEN, MERGED or CLOSED)."
            );
        }
        let agent_name = session.display_label();
        // Store the host exactly as the sync planner would derive it (empty
        // means github.com, lowercased): a raw wire command can carry any
        // spelling, and an unnormalized stored pin would never match the
        // planner's target, so the pin would never refresh.
        let host = crate::gh::normalized_github_host(host);
        let url = if url.trim().is_empty() {
            crate::gh::pull_request_url(&host, owner_repo, number)
        } else {
            url.to_string()
        };
        let stored = crate::storage::StoredPr {
            session_id: session_id.to_string(),
            pr_number: number,
            host,
            owner_repo: owner_repo.to_string(),
            state: state.to_string(),
            title: title.to_string(),
            url,
        };
        self.session_store.upsert_pr_override(&stored)?;
        // Plugging a pull request back in by hand lifts an earlier detach: the
        // user has said what this agent's PR is, so the session is tracked
        // again (against the pin, and against autodetection once unpinned).
        self.pr_suppressions.remove(session_id);
        self.session_store.delete_pr_suppression(session_id)?;
        if let Some(info) = crate::gh::reconstruct_pr_from_stored(&stored) {
            self.pr_statuses.insert(session_id.to_string(), info);
        }
        self.pr_overrides.insert(session_id.to_string(), stored);
        self.update_pr_sync_sessions();
        Ok(format!(
            "Attached PR #{number} ({owner_repo}) to agent \"{agent_name}\". dux will track \
             this pull request until you detach it; autodetection is paused for this agent."
        ))
    }

    /// Detach a session's pull request: this agent has no PR, as of now. The
    /// pin goes if there was one, the badge is cleared immediately (rather
    /// than surviving until some later sync cycle re-evaluates), and the
    /// session is recorded as suppressed so autodetection cannot put the badge
    /// straight back. Applies to an AUTODETECTED association too, which is the
    /// case the old pin-only detach could not answer at all.
    ///
    /// The suppression is durable: a restart is not the user changing their
    /// mind. It is lifted by a manual attach or by
    /// [`Self::resume_pr_autodetection`].
    pub fn clear_pull_request_override(&mut self, session_id: &str) -> anyhow::Result<String> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        let agent_name = session.display_label();
        // A standalone agent has no branch, so it has no pull request and
        // never can. Refused HERE, immediately after the existence check and
        // BEFORE the one-attach-at-a-time mutual block below, so the answer is
        // "this agent has no pull requests" rather than "wait for the attach
        // to finish" about a feature it does not have.
        let _ = self.branch_git_workspace(
            session_id,
            "attach, detach or track a pull request for",
            STANDALONE_ADD_AS_PROJECT_REMEDY,
        )?;
        // A manual attach for this agent is mid-flight, so its outcome is
        // still to come: detaching now would be undone (or half-undone) by the
        // attach landing a moment later. Refuse instead of racing it. The
        // guard sits AFTER the existence check so an unknown session still
        // gets the unknown-session error (the surfaces' 404).
        //
        // Deliberately `PrAttach` only. `InFlightKey::PrCheck` (the resume
        // one-shot and the background checks) must NEVER block these
        // operations: a background poll is not the user's own half-finished
        // act, and blocking on it would make detach fail at random moments.
        if self.is_in_flight(&InFlightKey::PrAttach(session_id.to_string())) {
            anyhow::bail!(pr_attach_in_flight_message(&agent_name));
        }
        self.pr_overrides.remove(session_id);
        self.session_store.delete_pr_override(session_id)?;
        // Write the suppression BEFORE re-deriving the sync snapshot, so the
        // snapshot this detach produces already excludes the session.
        self.pr_suppressions.insert(session_id.to_string());
        self.session_store.set_pr_suppressed(session_id)?;
        // The badge goes now. Callers reach this through a wire command, and
        // every wire command is a spine mutation, so the cleared badge is
        // published to the web with this change rather than a cycle later; the
        // TUI rebuilds its rows off the same call.
        self.pr_statuses.remove(session_id);
        self.update_pr_sync_sessions();
        Ok(format!(
            "Detached the pull request from agent \"{agent_name}\". dux will stop looking for \
             one on this agent until you attach a pull request by hand or resume autodetection \
             for it."
        ))
    }

    /// Undo a detach: autodetection is switched back on for the session and one
    /// immediate check runs so the badge comes back now rather than at the next
    /// poll. Deliberately not gated on `gh` (like the detach itself): the
    /// suppression is dux's own state, and clearing it must never depend on a
    /// CLI that could have been uninstalled since. Without a usable `gh` the
    /// check is a no-op and the next cycle after the integration re-arms picks
    /// the session up.
    pub fn resume_pr_autodetection(&mut self, session_id: &str) -> anyhow::Result<String> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        let agent_name = session.display_label();
        // A standalone agent has no branch, so it has no pull request and
        // never can. Refused HERE, immediately after the existence check and
        // BEFORE the one-attach-at-a-time mutual block below, so the answer is
        // "this agent has no pull requests" rather than "wait for the attach
        // to finish" about a feature it does not have.
        let _ = self.branch_git_workspace(
            session_id,
            "attach, detach or track a pull request for",
            STANDALONE_ADD_AS_PROJECT_REMEDY,
        )?;
        // Same mutual block as the detach beside it: an attach that is still
        // resolving owns this agent's pull-request state until it lands or
        // fails. `PrCheck` is deliberately not consulted here either.
        if self.is_in_flight(&InFlightKey::PrAttach(session_id.to_string())) {
            anyhow::bail!(pr_attach_in_flight_message(&agent_name));
        }
        let was_suppressed = self.pr_suppressions.remove(session_id);
        self.session_store.delete_pr_suppression(session_id)?;
        self.update_pr_sync_sessions();
        // A zero interval on purpose: the user just asked for this, so the
        // debounce a background trigger would respect has nothing to protect.
        self.spawn_pr_check_for_session(session_id, Duration::from_secs(0));
        // The one-shot above is a no-op while gh is unusable, so the message
        // must not claim a check that never started.
        let tail = if self.pr_agent_command_available() {
            "dux is checking GitHub for a pull request on its branch now."
        } else {
            "dux will check GitHub once the GitHub integration is enabled and gh is signed in."
        };
        if was_suppressed {
            Ok(format!(
                "Resumed pull-request autodetection for agent \"{agent_name}\". {tail}"
            ))
        } else {
            Ok(format!(
                "Pull-request autodetection was already running for agent \"{agent_name}\"; {tail}"
            ))
        }
    }
}

impl Engine {
    /// Dispatch the shared resolve→attach flow: validate synchronously, mint
    /// the ONE keyed op that spans resolve→attach, and spawn the lookup worker.
    /// Returns the op id and its pending (busy) status; the caller surfaces the
    /// busy (the TUI applies it as a reaction, the web returns it plus the op
    /// id in the `202` body). The final is resolved engine-side in
    /// `process_worker_event`'s `PullRequestResolved` attach arm, so neither
    /// surface can drift on the outcome handling.
    pub fn dispatch_attach_pull_request(
        &mut self,
        session_id: &str,
        raw_input: &str,
    ) -> anyhow::Result<(String, StatusUpdate)> {
        // Same gate as the new-agent-from-PR flow: GitHub integration on AND an
        // authenticated `gh`.
        if !self.pr_agent_command_available() {
            anyhow::bail!(
                "Attaching a pull request requires GitHub integration and an authenticated \
                 gh CLI."
            );
        }
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        let agent_name = session.display_label();
        // A standalone agent has no branch, so it has no pull request and
        // never can. Refused HERE, immediately after the existence check and
        // BEFORE the one-attach-at-a-time mutual block below, so the answer is
        // "this agent has no pull requests" rather than "wait for the attach
        // to finish" about a feature it does not have.
        let _ = self.branch_git_workspace(
            session_id,
            "attach, detach or track a pull request for",
            STANDALONE_ADD_AS_PROJECT_REMEDY,
        )?;
        // One attach at a time per agent: a second one would resolve against
        // the same session and the arrival order would decide which pin wins.
        // After the existence check, so an unknown session still 404s. Only
        // `PrAttach` blocks here; a running `PrCheck` is background work and
        // must never stop the user attaching a pull request by hand.
        if self.is_in_flight(&InFlightKey::PrAttach(session_id.to_string())) {
            anyhow::bail!(pr_attach_in_flight_message(&agent_name));
        }
        let Some(project) = session
            .project_id()
            .and_then(|project_id| self.projects.iter().find(|p| p.id == project_id))
            .cloned()
        else {
            anyhow::bail!(
                "Cannot attach a pull request: agent \"{agent_name}\" belongs to a project \
                 dux no longer knows."
            );
        };
        if raw_input.trim().is_empty() {
            anyhow::bail!("Enter a GitHub PR URL, owner/repo#123, or a PR number.");
        }

        // Validation is done and the worker is about to be spawned, so the
        // agent's other pull-request operations are blocked from here until
        // the resolution arrives (the `BranchRename` ordering: mark once the
        // dispatch is certain, never before a refusal path).
        self.mark_in_flight(InFlightKey::PrAttach(session_id.to_string()));

        let op = status_op(format!(
            "Resolving PR to attach to agent \"{agent_name}\"..."
        ))
        .resolve_in_handler(|o: &PrAttachOutcome| match o {
            PrAttachOutcome::Attached { message } => Final::info(message.clone()),
            PrAttachOutcome::Failed { message } => Final::error(message.clone()),
        })
        .with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = op.pending_status();
        self.pending_pr_attach_ops.insert(op_id.clone(), op);

        let worker_tx = self.worker_tx.clone();
        let policy = self.github_host_policy();
        let raw = raw_input.to_string();
        let sid = session_id.to_string();
        let op_id_for_worker = op_id.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Keep a sender outside `catch_unwind` so a panicking job still
            // resolves the keyed op instead of stranding its busy.
            let tx_panic = worker_tx.clone();
            let sid_panic = sid.clone();
            let op_id_panic = op_id_for_worker.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::gh::run_attach_pull_request_lookup_job(
                    project,
                    sid,
                    raw,
                    worker_tx,
                    Some(op_id_for_worker),
                    policy,
                );
            })) {
                let reason = crate::engine::format_panic_payload(payload);
                crate::logger::error(&format!("pr-attach lookup worker panicked: {reason}"));
                let _ = tx_panic.send(crate::worker::WorkerEvent::PullRequestResolved {
                    result: Err(format!("Worker panicked: {reason}")),
                    purpose: crate::worker::PrLookupPurpose::Attach {
                        session_id: sid_panic,
                    },
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok((op_id, pending))
    }
}

/// Project a stored override row into the pin identity the sync planner reads.
pub(crate) fn pinned_pr_from_stored(row: &crate::storage::StoredPr) -> crate::worker::PinnedPr {
    crate::worker::PinnedPr {
        host: row.host.clone(),
        owner_repo: row.owner_repo.clone(),
        number: row.pr_number,
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
        session
            .project_id()
            .and_then(|project_id| self.projects.iter().find(|p| p.id == project_id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Plan a standalone-agent create: validate the folder, resolve the title
    /// and the provider, and hand back the request to dispatch.
    ///
    /// THE ONE PLACE the standalone create's refusals live, so the web endpoint
    /// and the TUI's folder browser cannot answer the same question
    /// differently. Also returns the Busy message, because a keyed busy and its
    /// final have to name the same thing.
    pub fn plan_standalone_agent(
        &self,
        folder: &str,
        name: &str,
        provider: Option<&str>,
    ) -> anyhow::Result<(crate::worker::CreateAgentRequest, String)> {
        let folder = PathBuf::from(folder.trim());
        if !folder.is_absolute() {
            anyhow::bail!(
                "A standalone agent needs an absolute path to the folder it should run in; \
                 \"{}\" is relative, and dux has no working directory to resolve it against \
                 on your behalf.",
                folder.display()
            );
        }
        // THE OCCUPIED-DIRECTORY REFUSAL. Coding CLIs resume their conversation
        // history PER DIRECTORY, so a second agent in one directory would
        // silently pick up the first one's conversation, which is a
        // data-loss-shaped surprise rather than a mere duplicate.
        //
        // It compares against EVERY agent, not only the standalone ones. A
        // managed agent's worktree is a directory just the same, and aiming a
        // standalone agent at one is worse than a shared conversation: the
        // launch detaches the agent already there
        // (`detach_conflicting_worktree_session`), and the survivor then blocks
        // that worktree's deletion forever with a "still used by other agents"
        // message about an agent the user never associated with it.
        //
        // Canonical paths, so a symlink is not a way around it.
        //
        // This canonicalizes ON THE ENGINE THREAD, which the folder probe's own
        // doc forbids for itself, and the difference is deliberate: the probe
        // runs on a poll and would repeat that cost forever, while this runs
        // once per create.
        //
        // BE HONEST ABOUT THE BLAST RADIUS, though, because it is wider than the
        // one folder the user just picked: the loop canonicalizes EVERY existing
        // agent's directory as well, so one hung mount anywhere in the workspace
        // stalls the engine actor for this whole call, and the actor is what
        // serves keystrokes to every other agent. The accepted cost is that
        // window, on an action the user just took; the alternative is a
        // two-phase create, where the occupancy refusal would have to be
        // re-checked after the worker returned anyway. The candidate is
        // canonicalized ONCE up front rather than once per session, which is the
        // cheap half of the cost and all of it when there are no other agents.
        //
        // The refusal is a SIGNPOST, not a wall: adding the folder as a project
        // is the multi-agent shape dux is built for, and it brings tabs along.
        let wanted = crate::project_browser::canonical_or_original(&folder);
        if let Some(existing) = self.sessions.iter().find(|session| {
            crate::project_browser::canonical_or_original(std::path::Path::new(session.directory()))
                == wanted
        }) {
            anyhow::bail!(
                "Agent \"{}\" is already working in \"{}\". Coding CLIs resume their \
                 conversation history per directory, so a second agent there would silently \
                 pick up the first one's conversation. Add that folder as a project instead if \
                 you want several agents working on it: agents in a project each get their own \
                 worktree, so their conversations stay separate.",
                existing.display_label(),
                crate::home_path::shorten_home(&folder)
            );
        }
        // The typed name is used verbatim: no branch is created here, so
        // `is_valid_agent_name` deliberately does not apply. Folder names
        // legally contain characters a ref name cannot.
        let title = crate::git::standalone_agent_title(name, &folder);
        // The GLOBAL default provider, the same source the config's own default
        // uses, unless the caller overrode it.
        //
        // An override is validated against the configured provider list, the
        // same rule `change_agent_provider_wire` applies, because this plan is
        // reachable from an HTTP body: an unconfigured name falls back to being
        // used AS THE COMMAND (`Config::provider_command`), so accepting one
        // spawns whatever the caller named.
        let provider = match provider.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                if !self.config.providers.commands.contains_key(value) {
                    anyhow::bail!(
                        "Provider \"{value}\" is not configured. Pick one of the configured \
                         providers, or add it to the [providers] section of your config."
                    );
                }
                crate::model::ProviderKind::new(value)
            }
            None => self.config.default_provider(),
        };
        let busy_message = format!(
            "Creating a standalone agent in \"{}\"\u{2026}",
            crate::home_path::shorten_home(&folder)
        );
        Ok((
            crate::worker::CreateAgentRequest::Standalone {
                folder,
                title,
                provider,
            },
            busy_message,
        ))
    }

    /// Resolve what git dux may do for one agent. `None` for an unknown id, so
    /// a route can still answer 404 for one.
    ///
    /// The companion to [`Self::branch_git_workspace`]: that one answers the
    /// branch-identity question with a refusal sentence, this one answers all
    /// three parts at once for a caller that has to choose between them (the
    /// changes routes, which work in a standalone agent's folder when it is a
    /// repository, and refuse when it is not).
    pub fn session_git_access(&self, session_id: &str) -> Option<SessionGitAccess> {
        let session = self.sessions.iter().find(|s| s.id == session_id)?;
        Some(match &session.workspace {
            crate::model::AgentWorkspace::Managed(managed) => SessionGitAccess::Full {
                worktree: PathBuf::from(&managed.worktree_path),
            },
            crate::model::AgentWorkspace::Folder(folder) => {
                let directory = PathBuf::from(&folder.folder_path);
                let status = self.folder_repo_status(session_id);
                if status.changes_panel_works() {
                    SessionGitAccess::ChangesOnly { directory }
                } else {
                    SessionGitAccess::NoRepository {
                        directory,
                        quiet_reason: status.quiet_reason(),
                    }
                }
            }
        })
    }

    /// THE CHOKEPOINT. Resolve a session id to the managed working copy a
    /// branch-identity git feature may run in, or an error saying why it may
    /// not.
    ///
    /// Every git action that is about the AGENT's branch goes through here:
    /// push, pull, fork, the pull-request routes, branch rename, provenance,
    /// the worktree manager. Hiding the buttons is not an answer, because each
    /// of those is also an HTTP route and a palette command, so the id of a
    /// standalone agent could otherwise reach a real push in the user's folder
    /// from a command line.
    ///
    /// This is deliberately NOT the question the changes panel asks. That one
    /// is folder-driven and answered live by repository detection, because a
    /// standalone agent pointed at a repository gets a real changes panel; see
    /// [`Self::folder_repo_status`].
    ///
    /// `feature` and `remedy` are the two halves of the refusal sentence; see
    /// [`standalone_agent_refusal`].
    pub fn branch_git_workspace(
        &self,
        session_id: &str,
        feature: &str,
        remedy: &str,
    ) -> anyhow::Result<&crate::model::ManagedWorkspace> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        match session.workspace.as_managed() {
            Some(managed) => Ok(managed),
            None => anyhow::bail!(standalone_agent_refusal(
                &session.display_label(),
                feature,
                remedy
            )),
        }
    }

    /// The resolved environment an agent's processes run with.
    ///
    /// A MANAGED agent gets the global environment merged with its project's,
    /// as always. A STANDALONE agent gets the global environment with NO
    /// project overlay, exactly like a standalone terminal
    /// (`create_standalone_terminal`), because there is no project to overlay
    /// it with.
    ///
    /// This is a named, tested answer rather than an inherited code path on
    /// purpose: every site that looked a project up and fell through
    /// `unwrap_or_default` would silently hand a project-less agent an EMPTY
    /// environment, which is a very different thing from the global one.
    pub fn session_env(&self, session: &AgentSession) -> Vec<(String, String)> {
        let project_env = match &session.workspace {
            crate::model::AgentWorkspace::Managed(managed) => self
                .projects
                .iter()
                .find(|project| project.id == managed.project_id)
                .map(|project| project.env.clone())
                .unwrap_or_default(),
            crate::model::AgentWorkspace::Folder(_) => std::collections::BTreeMap::new(),
        };
        crate::config::resolve_agent_env(&self.config.env, &project_env).unwrap_or_default()
    }

    /// Where an agent lives, as a phrase a status line can drop into a
    /// sentence: `project "web"` for a managed agent, `folder "~/notes"` for a
    /// standalone one. Folder paths are shortened against the server's home.
    pub fn session_location_phrase(&self, session: &AgentSession) -> String {
        match &session.workspace {
            crate::model::AgentWorkspace::Managed(_) => {
                format!("project \"{}\"", self.project_name_for_session(session))
            }
            crate::model::AgentWorkspace::Folder(folder) => format!(
                "folder \"{}\"",
                crate::home_path::shorten_home(Path::new(&folder.folder_path))
            ),
        }
    }

    /// The status message shown when an existing agent's provider becomes ready
    /// after a reconnect. Shared by the TUI and the web so both report the SAME
    /// completion message (1:1) instead of a frontend echoing its own
    /// "attaching…" placeholder back to the user. `resume` is the result of
    /// [`should_resume_session`]. Callers may append extra context (e.g. a
    /// detached-worktree note) to the returned string.
    pub fn agent_reconnect_status_message(&self, session: &AgentSession, resume: bool) -> String {
        let location = self.session_location_phrase(session);
        if resume {
            format!(
                "Resumed {} agent \"{}\" in {}.",
                session.provider.as_str(),
                session.display_label(),
                location
            )
        } else {
            format!(
                "Started fresh {} session for agent \"{}\" in {}. Use /sessions inside the agent to restore a prior conversation.",
                session.provider.as_str(),
                session.display_label(),
                location
            )
        }
    }

    /// Build the plan for reconnecting (relaunching) an agent session, the
    /// single source both surfaces call so the guards, the resume decision, and
    /// the status message are computed once. `force` is the force-reconnect
    /// (always-fresh) path. `pty_size` is the surface's launch size (the TUI's
    /// last known size; `(24, 80)` for the web).
    ///
    /// A returned `Launch` has ALREADY applied the pre-dispatch mutations (the
    /// force teardown via `clear_tab_runtime`, and detaching any agent holding
    /// the same worktree's live PTY); the caller only dispatches the request.
    /// The `AlreadyConnected`/`WorktreeMissing` variants apply no mutations.
    ///
    /// The resume decision uses `tab_resume_decision` (collision-aware) for both
    /// the request and the announced `resume`, so a surface never announces a
    /// resume that the dispatch downgrades to fresh.
    pub fn reconnect_plan(
        &mut self,
        session_id: &str,
        force: bool,
        pty_size: (u16, u16),
    ) -> anyhow::Result<ReconnectPlan> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;

        // Check order mirrors the surfaces: normal reconnect refuses while a
        // provider is live; force skips that (it kills the provider). Both then
        // guard the worktree.
        if !force && self.providers.contains_key(session.slot_tab_id()) {
            return Ok(ReconnectPlan::AlreadyConnected {
                message: format!(
                    "Agent \"{}\" is already connected.",
                    session.display_label()
                ),
            });
        }
        // Both kinds have a directory to reconnect into, and both must
        // exist; only the sentence differs, because a standalone agent's
        // directory is the user's folder and dux cannot re-create it.
        if !std::path::Path::new(session.directory()).exists() {
            let message = match &session.workspace {
                crate::model::AgentWorkspace::Managed(_) => format!(
                    "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                    session.display_label()
                ),
                crate::model::AgentWorkspace::Folder(folder) => format!(
                    "The folder agent \"{}\" runs in ({}) no longer exists. Restore the folder, or delete this agent and create a new one pointing at the folder you want.",
                    session.display_label(),
                    crate::home_path::shorten_home(Path::new(&folder.folder_path))
                ),
            };
            return Ok(ReconnectPlan::WorktreeMissing { message });
        }

        if force {
            // Kill the existing provider and clear ALL resume state (routed
            // through the single-source `clear_tab_runtime`, so the in-flight
            // `AgentLaunch` key goes too) so the relaunch is genuinely fresh.
            self.clear_tab_runtime(session.slot_tab_id());
        }
        // Detach any other session holding the same worktree's live PTY.
        let detached_label = self
            .detach_conflicting_worktree_session(session.directory(), &session.id)
            .map(|detached| detached.label);

        // The one resume decision: collision-aware, used for BOTH the request and
        // the announced message. Force never resumes.
        let resume = if force {
            false
        } else {
            self.tab_resume_decision(&session, session.slot_tab_id(), &session.provider, true)
        };
        let mut msg = self.agent_reconnect_status_message(&session, resume);
        if let Some(detached) = &detached_label {
            msg.push_str(&format!(
                " Agent \"{}\" was detached to avoid worktree conflicts.",
                detached,
            ));
        }
        // A standalone agent has no project whose default provider it could
        // be diverging from, so the note is simply not written for one.
        if let Some(project) = session
            .project_id()
            .and_then(|project_id| self.projects.iter().find(|p| p.id == project_id))
            && project.default_provider != session.provider
        {
            let provider_label = if self.project_uses_explicit_default_provider(&project.id) {
                "current project provider"
            } else {
                "current global default provider"
            };
            msg.push_str(&format!(
                " Note: this agent uses {}. Your {provider_label} is {}.",
                session.provider.as_str(),
                project.default_provider.as_str(),
            ));
        }

        let branch_name = session.display_label();
        let kind = if force {
            crate::worker::AgentLaunchKind::ForceReconnect {
                status_message: msg,
            }
        } else {
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: msg,
            }
        };
        let request = self.build_agent_launch_request(session, resume, pty_size, kind);
        let busy_message = if force {
            format!("Starting fresh agent \"{branch_name}\"...")
        } else {
            format!("Launching agent \"{branch_name}\"...")
        };
        Ok(ReconnectPlan::Launch {
            request: Box::new(request),
            busy_message,
            resume,
            detached_label,
        })
    }

    /// Provider currently driving the session's live PTY, if any. After an
    /// in-place provider swap while the agent is still running, this returns
    /// the *original* provider until the user exits and relaunches — so the
    /// pane title doesn't lie about what's actually on screen.
    pub fn running_provider_for(&self, session: &AgentSession) -> ProviderKind {
        // The pin map is keyed by TAB id, so the agent's own pane reads the pin
        // under its session-slot tab, which the session's pointer names.
        self.running_provider_pins
            .get(session.slot_tab_id())
            .cloned()
            .unwrap_or_else(|| session.provider.clone())
    }

    pub fn should_resume_session(&self, session: &AgentSession) -> bool {
        self.should_resume_provider(session, &session.provider)
    }

    /// The provider a new tab (added with no explicit provider arg) defaults to:
    /// the owning project's `default_provider`, falling back to the global config
    /// default only when the project is missing. The single-source rule both
    /// Rust surfaces call (the web `create_agent_tab`, the TUI new-tab flow) so
    /// the "+" quick-add and its picker's "default" marker cannot disagree with
    /// what launches. (The web TS mirror `defaultProviderForSession` reads the
    /// already-resolved `default_provider` off the spine's project.)
    /// `project_id` is `None` for a STANDALONE agent, which has no project to
    /// take a default from and so takes the global one, the same source the
    /// config's own default uses.
    pub fn default_provider_for_new_tab(&self, project_id: Option<&str>) -> ProviderKind {
        project_id
            .and_then(|project_id| self.projects.iter().find(|p| p.id == project_id))
            .map(|p| p.default_provider.clone())
            .unwrap_or_else(|| self.config.default_provider())
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
    pub fn tab_running_provider(&self, session: &AgentSession, tab_id: &TabIdRef) -> ProviderKind {
        if let Some(pinned) = self.running_provider_pins.get(tab_id) {
            return pinned.clone();
        }
        if self.is_slot_tab(session, tab_id) {
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
        tab_id: &TabIdRef,
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

    /// The agent record for `session_id`, or `None`. A plain identity lookup by
    /// primary key, not a slot-ness question.
    pub fn session_by_id(&self, session_id: &str) -> Option<&AgentSession> {
        self.sessions.iter().find(|s| s.id == session_id)
    }

    /// The id of `session`'s session-slot tab. Thin wrapper over
    /// [`AgentSession::slot_tab_id`] so engine-side callers with an `Engine` in
    /// hand ask the same question in the same words. See that method for why the
    /// answer is not simply the session id forever.
    pub fn slot_tab_id_for<'a>(&self, session: &'a AgentSession) -> &'a TabIdRef {
        session.slot_tab_id()
    }

    /// Whether `tab_id` names `session`'s session-slot tab. The engine-side
    /// spelling of [`AgentSession::is_slot_tab`]; no call site compares a tab id
    /// against a session id inline.
    pub fn is_slot_tab(&self, session: &AgentSession, tab_id: &TabIdRef) -> bool {
        session.is_slot_tab(tab_id)
    }

    /// Slot-ness when only ids are in hand (the wire, REST and socket layers,
    /// which are handed two path segments and no records). An unknown
    /// `session_id` answers `false`: it names no agent, so it has no slot tab.
    ///
    /// That is deliberately NOT the fallback [`Self::slot_tab_id_of`] takes for
    /// the same unknown id. These two are asked for opposite purposes: this one
    /// guards refusal paths, where the safe answer to "is this the slot tab of an
    /// agent nobody has heard of" is no, while the other seeds enumerations,
    /// where dropping the key entirely would lose a runtime entry.
    pub fn is_slot_tab_of(&self, session_id: &SessionIdRef, tab_id: &TabIdRef) -> bool {
        self.session_by_id(session_id.as_str())
            .is_some_and(|session| session.is_slot_tab(tab_id))
    }

    /// The slot tab id for a session id, falling back to `session_id` itself when
    /// no such agent exists. The fallback keeps every enumeration seeded with a
    /// key rather than silently dropping one for a session that has already been
    /// removed from the map (the historical behavior of these paths).
    ///
    /// The asymmetry with [`Self::is_slot_tab_of`], which answers `false` for the
    /// same unknown id, is deliberate: a fallback preserves those enumeration
    /// seeds, while a `false` preserves the refusals that ask the question.
    pub fn slot_tab_id_of<'a>(&'a self, session_id: &'a SessionIdRef) -> &'a TabIdRef {
        self.session_by_id(session_id.as_str())
            // The fallback REINTERPRETS a session id as a tab id. Nothing
            // makes that a real tab any more, and it is not meant to be one: it
            // is a stable placeholder key for a session the map has already
            // forgotten, so an enumeration keeps its seed instead of silently
            // dropping an entry. Every live session answers from its pointer
            // above and never reaches here.
            .map_or(TabIdRef::new(session_id.as_str()), |session| {
                session.slot_tab_id()
            })
    }

    /// The agent whose session-slot tab is `tab_id`, or `None` when `tab_id` is
    /// an extra tab or names nothing. The inverse of
    /// [`AgentSession::slot_tab_id`], and the one place that resolves a bare tab
    /// id back to the agent it is the first tab of.
    pub fn session_for_slot_tab(&self, tab_id: &TabIdRef) -> Option<&AgentSession> {
        self.sessions.iter().find(|s| s.is_slot_tab(tab_id))
    }

    /// Every runtime-map key owned by a session: its session-slot tab plus every
    /// extra tab id. The single source of truth for teardown fan-out — a
    /// full-session teardown must clear all of these, not just the slot tab.
    ///
    /// The slot id comes from the session's stored pointer, and the extras from
    /// the in-memory map, which holds exactly the tabs that are not in the slot
    /// (see `SessionStore::load_extra_agent_tabs`).
    pub fn tab_ids_for_session(&self, session_id: &str) -> Vec<TabId> {
        let mut ids = vec![
            self.slot_tab_id_of(SessionIdRef::new(session_id))
                .to_owned(),
        ];
        ids.extend(
            self.agent_tabs
                .values()
                .filter(|t| t.session_id == session_id)
                .map(|t| TabId::new(t.id.clone())),
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
    /// first, then extra tabs ordered by `(sort_order, created_at, id)` (the
    /// same order the TUI's tab strip renders). A tab counts as live when it has a
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
        // The id is the final tiebreak, matching `successor_slot_tab` and the two
        // render orderings. Ties are unreachable today (`sort_order` is a
        // per-agent append-only stamp), so this is parity across the four
        // orderings rather than a fix: they must not be able to disagree about
        // which pill comes first.
        extras.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        std::iter::once(
            self.slot_tab_id_of(SessionIdRef::new(session_id))
                .to_owned(),
        )
        .chain(extras.into_iter().map(|t| TabId::new(t.id.clone())))
        .find(|id| {
            self.providers.contains_key(id.as_ref_id())
                || self.is_in_flight(&InFlightKey::AgentLaunch(id.clone()))
        })
        .map(|id| id.as_str().to_string())
    }

    /// Resolve a tab id back to the session that owns it. An extra tab resolves
    /// via its `agent_tabs` row; otherwise the id is looked up as some agent's
    /// session-slot tab. Returns `None` for an unknown id.
    pub fn owning_session_for_tab(&self, tab_id: &str) -> Option<String> {
        // Transport-facing: the id arrives unclassified (a URL segment, a socket
        // frame). Named as a tab id here, at the door.
        let tab_id = TabIdRef::new(tab_id);
        if let Some(tab) = self.agent_tabs.get(tab_id) {
            return Some(tab.session_id.clone());
        }
        self.session_for_slot_tab(tab_id)
            .map(|session| session.id.clone())
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

        let running = self
            .providers
            .contains_key(self.sessions[index].slot_tab_id());
        let previous = self.sessions[index].provider.clone();

        // The slot tab's row and the session's mirrored `provider` move
        // together, through the one write site that owns both.
        let updated_at = Utc::now();
        self.session_store
            .set_slot_provider(session_id, provider.as_str(), updated_at)?;
        let session = &mut self.sessions[index];
        session.provider = provider.clone();
        session.updated_at = updated_at;
        let updated = session.clone();

        // Pin the still-running provider so UI labels stay truthful until the
        // user exits and relaunches the agent. Only set on the first
        // swap-while-running — later swaps don't change what's spawned.
        if running {
            self.running_provider_pins
                .entry(updated.slot_tab_id().to_owned())
                .or_insert_with(|| previous.clone());
        }

        // The resume decision is asked of a TAB, compared against this session's
        // sibling tab ids: the tab being retargeted is the session-slot one.
        let resume_available =
            self.tab_resume_decision(&updated, updated.slot_tab_id(), &provider, true);

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

        // Per-agent cap. Every tab is a row, the slot tab included, so the
        // stored count is the whole count.
        let max_per_agent = i64::from(self.agent_tabs_max());
        if self.session_store.count_agent_tabs(session_id)? >= max_per_agent {
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
        self.agent_tabs.insert(TabId::new(tab_id.clone()), tab);

        let status_message = format!("Started a fresh {} tab.", provider.as_str());
        let request = self.build_tab_launch_request(
            TabId::new(tab_id.clone()),
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
                    self.agent_tabs.remove(TabIdRef::new(&tab_id));
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

    /// Remove an extra tab's row (memory + session store) for a tab whose PTY
    /// is already gone — the clean-exit auto-close used by both surfaces' exit
    /// paths (see `clean_exit_closes_tab_row`). Returns `true` when a row was
    /// actually removed. A store failure is logged, not fatal: the in-memory
    /// removal already happened and the stale row is re-reconciled on the next
    /// restart at worst.
    pub fn remove_agent_tab_row(&mut self, tab_id: &str) -> bool {
        if self.agent_tabs.remove(TabIdRef::new(tab_id)).is_none() {
            return false;
        }
        // The row is gone, so nothing will ever read this tab's recorded verdict
        // again; keeping it would be one leaked entry per closed tab on a
        // long-running server.
        self.clear_tab_run_failure(TabIdRef::new(tab_id));
        if let Err(err) = self.session_store.delete_agent_tab(tab_id) {
            crate::logger::warn(&format!(
                "failed to delete cleanly-exited tab {tab_id} from the session store: {err}"
            ));
        }
        true
    }

    /// Every tab of a session in strip order, paired with the label the strip
    /// shows for it: the tab's EFFECTIVE provider (a retarget-while-running pin
    /// wins over the row, exactly as the pill reads it) run through
    /// [`crate::agent_tabs::tab_labels`], so a repeated provider carries the
    /// same disambiguating suffix the pills carry.
    ///
    /// Every sentence that names a tab reads it here. A confirmation, a status
    /// line and a pill that derive the name three different ways can disagree
    /// about which tab they mean, and the user has only the name to go on.
    pub fn tab_strip_labels(&self, session_id: &SessionIdRef) -> Vec<(TabId, String)> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id.as_str()) else {
            return Vec::new();
        };
        let mut extras: Vec<&AgentTab> = self
            .agent_tabs
            .values()
            .filter(|t| t.session_id == session_id.as_str())
            .collect();
        // The same `(sort_order, created_at, id)` order `successor_slot_tab`
        // and `first_live_tab` use: the four orderings must not be able to
        // disagree about which pill comes first.
        extras.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        let ids: Vec<TabId> = std::iter::once(self.slot_tab_id_of(session_id).to_owned())
            .chain(extras.into_iter().map(|t| TabId::new(t.id.clone())))
            .collect();
        let providers: Vec<String> = ids
            .iter()
            .map(|id| {
                self.tab_running_provider(session, id.as_ref_id())
                    .as_str()
                    .to_string()
            })
            .collect();
        let labels = crate::agent_tabs::tab_labels(
            &providers.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );
        ids.into_iter().zip(labels).collect()
    }

    /// One tab's name as prose names it: its strip label, upper-cased on the
    /// first character (see [`crate::agent_tabs::prose_tab_label`]). `None` for
    /// a tab this session does not have.
    ///
    /// Read it BEFORE a close when the sentence is about a close: the strip the
    /// user is looking at is the pre-close one, and that is the strip the
    /// confirmation and the status line must both be talking about.
    pub fn tab_prose_label(&self, session_id: &SessionIdRef, tab_id: &TabIdRef) -> Option<String> {
        self.tab_strip_labels(session_id)
            .into_iter()
            .find(|(id, _)| id.as_str() == tab_id.as_str())
            .map(|(_, label)| crate::agent_tabs::prose_tab_label(&label))
    }

    /// The tab that takes the slot when the tab currently in it is closed: the
    /// FIRST extra tab in strip order — `sort_order`, then `created_at`, then
    /// `id`, the same ordering the strip and [`Self::first_live_tab`] render —
    /// so the successor is the pill next to the one going away. Live or dormant
    /// makes no difference; a tab is a tab.
    ///
    /// `None` when the agent has no other tab, which is the case that stays
    /// "closing the last tab detaches the agent".
    ///
    /// The id is the final tiebreak so the answer is deterministic even for two
    /// tabs written in the same instant: `agent_tabs` is a `HashMap`, whose
    /// iteration order is not.
    pub fn successor_slot_tab(&self, session_id: &SessionIdRef) -> Option<&AgentTab> {
        self.agent_tabs
            .values()
            .filter(|t| t.session_id == session_id.as_str())
            .min_by(|a, b| {
                a.sort_order
                    .cmp(&b.sort_order)
                    .then_with(|| a.created_at.cmp(&b.created_at))
                    .then_with(|| a.id.cmp(&b.id))
            })
    }

    /// Hand the session slot to `session_id`'s next tab in strip order and
    /// return which tab took it, so the caller can tear the outgoing one down
    /// like any other tab.
    ///
    /// Nothing is re-keyed: the promoted tab keeps its id, its row, its PTY, its
    /// sockets, its attention flag and its resume state. Only its ROLE changes,
    /// which is why a promotion cannot strand a browser connection or a
    /// runtime-map entry.
    ///
    /// Persist-first, like every other tab mutation: the one transaction lands
    /// before memory moves, so a storage failure leaves the agent exactly as it
    /// was rather than pointing at a tab SQLite still calls an extra.
    fn promote_next_tab_into_slot(
        &mut self,
        session_id: &SessionIdRef,
        outgoing_tab_id: &TabIdRef,
    ) -> anyhow::Result<TabId> {
        // Mid-deletion the agent's worktree is about to go and every one of its
        // tabs is already being torn down; moving the slot around inside it
        // would be re-pointing at a tab that is itself about to vanish.
        if self.closing_sessions.contains(session_id.as_str()) {
            anyhow::bail!("this agent is being deleted; its tabs cannot hand the slot around");
        }
        let (new_slot, provider) = self
            .successor_slot_tab(session_id)
            .map(|t| (TabId::new(t.id.clone()), t.provider.clone()))
            .ok_or_else(|| anyhow::anyhow!(crate::agent_tabs::ONLY_TAB_CLOSE_REFUSAL))?;
        let updated_at = Utc::now();
        self.session_store.promote_tab_to_slot(
            session_id.as_str(),
            new_slot.as_str(),
            outgoing_tab_id.as_str(),
            provider.as_str(),
            updated_at,
        )?;
        // The in-memory `agent_tabs` map holds exactly the tabs that are NOT in
        // the slot, so the promoted one leaves it: listed in both places it
        // would be enumerated twice and counted twice.
        self.agent_tabs.remove(&new_slot);
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.id == session_id.as_str())
        {
            session.slot_tab_id = new_slot.as_str().to_string();
            // The session's provider is a mirror of whichever tab holds the
            // slot, so it moves with the pointer (the storage half wrote the
            // same value in the same transaction).
            session.provider = provider;
            session.updated_at = updated_at;
            // The focus memory represents the slot tab as absence, so a memory
            // naming either end of this promotion is retired. Persisted in the
            // transaction above; this is the in-memory half of the same rule.
            let stale_memory = session
                .last_focused_tab
                .as_deref()
                .is_some_and(|id| id == new_slot.as_str() || id == outgoing_tab_id.as_str());
            if stale_memory {
                session.last_focused_tab = None;
            }
        }
        Ok(new_slot)
    }

    /// Close one of an agent's tabs.
    ///
    /// Closing the tab in the session slot PROMOTES the next tab in strip order
    /// into it and then tears the outgoing tab down exactly like any other: the
    /// slot is a pointer, not an identity, so there is nothing special left to
    /// protect. Closing the LAST remaining tab is still refused, because an
    /// agent always has a slot; that close is the agent's detach, a different
    /// action.
    ///
    /// Either way the row goes first (a persistence failure leaves in-memory
    /// state untouched), then the PTY is torn down gracefully and every runtime
    /// map the tab keyed is cleared.
    pub fn close_tab(&mut self, session_id: &str, tab_id: &str) -> anyhow::Result<CloseTabOutcome> {
        // Transport-facing: two path segments arrive as bare strings, which is
        // exactly the pair that used to be swappable. Named here, at the door.
        let promoted = if self.is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(tab_id))
        {
            Some(
                self.promote_next_tab_into_slot(
                    SessionIdRef::new(session_id),
                    TabIdRef::new(tab_id),
                )?,
            )
        } else {
            match self.agent_tabs.get(TabIdRef::new(tab_id)) {
                Some(tab) if tab.session_id == session_id => {}
                Some(_) => anyhow::bail!("tab {tab_id} does not belong to session {session_id}"),
                None => anyhow::bail!("unknown tab: {tab_id}"),
            }

            // Persist first.
            self.session_store.delete_agent_tab(tab_id)?;
            None
        };

        // Graceful PTY teardown (SIGTERM into the terminating set), then clear
        // every runtime map this tab keyed via the shared `clear_tab_runtime`
        // (begin_close_provider only drops `providers`, which clear_tab_runtime
        // then finds already gone — a harmless no-op).
        let label = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.display_label())
            .unwrap_or_else(|| tab_id.to_string());
        // No worktree removal is deferred on a tab close, so the return is None.
        let _ = self.begin_close_provider(TabIdRef::new(tab_id), label, None);
        self.clear_tab_runtime(TabIdRef::new(tab_id));
        self.agent_tabs.remove(TabIdRef::new(tab_id));
        // Closing this tab may have removed the agent's last live process (its
        // session-slot tab already dormant). No tab is privileged, so recompute:
        // the agent detaches once nothing of it is live/launching. `any_tab_active`
        // is IN-FLIGHT-AWARE, so a sibling mid-launch keeps the agent attached
        // (the outcome both surfaces consume, instead of re-deriving it from
        // `has_live_process` (a `providers` lookup that misses in-flight launches).
        let detached = !self.any_tab_active(session_id);
        if detached {
            // Closing the tab in the SLOT is the deliberate "I am done with this
            // agent" gesture (the same one a clean exit of the slot tab is, which
            // clears the intent in `prune_exited_ptys`), so when it takes the
            // agent's last live process the auto-reopen intent goes with it
            // rather than resurrecting the agent at the next startup sweep. An
            // extra tab's close is not that gesture and leaves the intent alone.
            // This is a deliberate departure from the plan's "the promotion
            // transfers nothing": the intent belongs to the gesture, not to the
            // tab, and leaving it set would reopen an agent the user just closed.
            if promoted.is_some() {
                self.mark_session_desired_running(session_id, false);
            }
            self.mark_session_status(session_id, crate::model::SessionStatus::Detached);
        }
        Ok(CloseTabOutcome { detached, promoted })
    }

    /// Retarget a tab's provider (effective on its next launch). For the session-slot tab
    /// this delegates to the untouched
    /// [`Engine::change_agent_provider`]; for an extra tab it updates only that
    /// tab's row, pinning the previously-running provider if it is live.
    pub fn change_tab_provider(
        &mut self,
        session_id: &str,
        tab_id: &str,
        provider: ProviderKind,
    ) -> anyhow::Result<ChangeAgentProviderOutcome> {
        // Transport-facing, exactly as in `close_tab`: name the two segments.
        if self.is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(tab_id)) {
            return self.change_agent_provider(session_id, provider);
        }
        let tab_id_ref = TabIdRef::new(tab_id);

        if !self
            .config
            .providers
            .commands
            .contains_key(provider.as_str())
        {
            anyhow::bail!("provider \"{}\" is not configured", provider.as_str());
        }

        let running = self.providers.contains_key(tab_id_ref);
        // Persist first: read the previous value read-only and verify ownership,
        // write the DB, and only mutate the in-memory row after the write
        // succeeds — so a persistence failure leaves memory and SQLite in sync
        // (mirroring `create_tab`/`close_tab`).
        let previous = self
            .agent_tabs
            .get(tab_id_ref)
            .filter(|t| t.session_id == session_id)
            .map(|t| t.provider.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown tab: {tab_id}"))?;
        self.session_store
            .update_agent_tab_provider(tab_id, provider.as_str())?;
        if let Some(tab) = self.agent_tabs.get_mut(tab_id_ref) {
            tab.provider = provider.clone();
        }

        if running {
            self.running_provider_pins
                .entry(tab_id_ref.to_owned())
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
            .map(|session| self.tab_resume_decision(session, tab_id_ref, &provider, true))
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
        // The slot tab is represented as absence. That question goes to the
        // resolver: it used to be an inline `id != session_id`, which stopped
        // being right the moment the slot became a stored pointer.
        let normalized = match tab_id {
            Some(id)
                if !self.is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(id))
                    && self
                        .agent_tabs
                        .get(TabIdRef::new(id))
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

/// The outcome of [`Engine::reconnect_plan`]: everything a surface needs to
/// relaunch (or refuse to relaunch) an agent. Core computes the resume decision
/// and status once for both the TUI and web surfaces.
///
/// A `Launch` plan has already applied the pre-dispatch mutations (force teardown
/// and any conflicting-worktree detach); the caller only dispatches `request`.
pub enum ReconnectPlan {
    /// Normal reconnect refused: a provider is already live. The caller shows
    /// `message` and does nothing else.
    AlreadyConnected { message: String },
    /// The session's worktree is gone. The caller surfaces `message` as an error
    /// (the TUI as a status error, the web as a 400).
    WorktreeMissing { message: String },
    /// Relaunch: dispatch `request` and surface `busy_message` as the pending
    /// status. `resume` is the collision-aware decision the request carries, so a
    /// surface can announce it truthfully. `detached_label` names any conflicting
    /// same-worktree agent that was detached to make room (already applied).
    Launch {
        request: Box<crate::worker::AgentLaunchRequest>,
        busy_message: String,
        resume: bool,
        detached_label: Option<String>,
    },
}

impl std::fmt::Debug for ReconnectPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReconnectPlan::AlreadyConnected { message } => f
                .debug_struct("AlreadyConnected")
                .field("message", message)
                .finish(),
            ReconnectPlan::WorktreeMissing { message } => f
                .debug_struct("WorktreeMissing")
                .field("message", message)
                .finish(),
            ReconnectPlan::Launch {
                busy_message,
                resume,
                detached_label,
                ..
            } => f
                .debug_struct("Launch")
                .field("busy_message", busy_message)
                .field("resume", resume)
                .field("detached_label", detached_label)
                .finish_non_exhaustive(),
        }
    }
}

/// Result of [`Engine::close_tab`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseTabOutcome {
    /// Whether closing the tab DETACHED the agent (it was the agent's last live
    /// tab). Computed with the in-flight-aware `any_tab_active`, so both
    /// surfaces consume this authoritative value instead of re-deriving
    /// detachment from a `providers`-only liveness check that misses a
    /// sibling's in-flight launch.
    pub detached: bool,
    /// The tab that took the session slot, when the closed tab was the one in
    /// it. `None` for an ordinary extra tab's close. Surfaces use it to say
    /// which tab is the agent's first pill now, and to land the user on it.
    pub promoted: Option<TabId>,
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
    use crate::engine::test_support::{
        sample_project, sample_session, sample_standalone_session, test_engine,
    };

    /// An engine with one managed agent that has one extra tab, for the
    /// slot-tab resolvers.
    fn engine_with_an_extra_tab() -> (Engine, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        engine
            .projects
            .push(crate::engine::test_support::sample_project("p1", "/tmp/p1"));
        engine.sessions.push(sample_session("s1", "p1", "b1"));
        engine.agent_tabs.insert(
            TabId::new("tab-b"),
            crate::model::AgentTab {
                id: "tab-b".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: Utc::now(),
            },
        );
        (engine, tmp)
    }

    #[test]
    fn slot_tab_resolvers_agree_on_which_tab_is_the_slot() {
        let (engine, _tmp) = engine_with_an_extra_tab();
        let session = engine.session_by_id("s1").expect("session").clone();

        let slot = engine.slot_tab_id_for(&session).to_owned();
        assert_eq!(
            engine.slot_tab_id_of(SessionIdRef::new("s1")),
            slot.as_ref_id()
        );
        assert!(engine.is_slot_tab(&session, &slot));
        assert!(engine.is_slot_tab_of(SessionIdRef::new("s1"), &slot));

        // The extra tab is not the slot tab, whichever way it is asked.
        assert!(!engine.is_slot_tab(&session, TabIdRef::new("tab-b")));
        assert!(!engine.is_slot_tab_of(SessionIdRef::new("s1"), TabIdRef::new("tab-b")));
    }

    #[test]
    fn session_for_slot_tab_is_the_inverse_and_ignores_extra_tabs() {
        let (engine, _tmp) = engine_with_an_extra_tab();
        let slot = engine.slot_tab_id_of(SessionIdRef::new("s1")).to_owned();

        assert_eq!(
            engine.session_for_slot_tab(&slot).map(|s| s.id.as_str()),
            Some("s1")
        );
        // An extra tab is somebody's tab but nobody's SLOT tab.
        assert!(
            engine
                .session_for_slot_tab(TabIdRef::new("tab-b"))
                .is_none()
        );
        assert!(engine.session_for_slot_tab(TabIdRef::new("nope")).is_none());
    }

    #[test]
    fn slot_ness_of_an_unknown_session_is_false_and_its_slot_id_falls_back() {
        let (engine, _tmp) = engine_with_an_extra_tab();

        // No agent, so nothing can be its slot tab.
        assert!(!engine.is_slot_tab_of(SessionIdRef::new("ghost"), TabIdRef::new("ghost")));
        assert!(!engine.is_slot_tab_of(SessionIdRef::new("ghost"), TabIdRef::new("tab-b")));
        // But the enumeration seeds still get a key rather than dropping one,
        // which is what the pre-resolver code did for a removed session.
        assert_eq!(
            engine.slot_tab_id_of(SessionIdRef::new("ghost")).as_str(),
            "ghost"
        );
        assert_eq!(
            engine.tab_ids_for_session("ghost"),
            vec![TabId::new("ghost")]
        );
    }

    #[test]
    fn tab_ids_for_session_leads_with_the_slot_tab() {
        let (engine, _tmp) = engine_with_an_extra_tab();
        assert_eq!(
            engine.tab_ids_for_session("s1"),
            vec![
                engine.slot_tab_id_of(SessionIdRef::new("s1")).to_owned(),
                TabId::new("tab-b")
            ]
        );
    }

    /// An engine with one ordinary managed agent and one standalone agent, for
    /// the background-enumerator gates. Every enumerator must enrol the first
    /// and skip the second: a plain folder has no branch to watch and no
    /// repository to ask GitHub about, and enrolling it would burn a git or
    /// `gh` call per cycle to produce an error nobody can act on.
    fn engine_with_a_standalone_agent() -> (Engine, tempfile::TempDir, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        let folder = tempfile::tempdir().expect("folder");
        engine
            .projects
            .push(crate::engine::test_support::sample_project("p1", "/tmp/p1"));
        engine.sessions.push(sample_session("s1", "p1", "b1"));
        engine.sessions.push(sample_standalone_session(
            "sa1",
            folder.path().to_string_lossy().as_ref(),
        ));
        (engine, tmp, folder)
    }

    /// The three-way capability, resolved in one engine round trip so a route
    /// cannot ask half the question. A managed agent gets everything; a
    /// standalone agent in a repository gets the changes panel and nothing
    /// branch-shaped; a standalone agent in a plain folder gets neither, with
    /// a sentence saying why.
    #[test]
    fn session_git_access_answers_the_three_way_capability() {
        let (mut engine, _tmp, folder) = engine_with_a_standalone_agent();

        match engine.session_git_access("s1") {
            Some(SessionGitAccess::Full { worktree }) => {
                assert_eq!(worktree, std::path::Path::new("/tmp/s1-worktree"));
            }
            other => panic!("a managed agent has the full git surface, got {other:?}"),
        }

        // Not probed yet: honest-quiet, and no mutations on a guess. The
        // sentence is a wait rather than an accusation, because this is the
        // state a healthy repository passes through on the way to its verdict.
        match engine.session_git_access("sa1") {
            Some(SessionGitAccess::NoRepository { quiet_reason, .. }) => {
                assert!(quiet_reason.contains("still looking"), "{quiet_reason}");
            }
            other => panic!("an unprobed folder must be honest-quiet, got {other:?}"),
        }

        // The folder IS a repository: the changes panel works, and only that.
        engine
            .folder_repo_statuses
            .insert("sa1".to_string(), crate::git::FolderRepoStatus::WorkingRepo);
        match engine.session_git_access("sa1") {
            Some(SessionGitAccess::ChangesOnly { directory }) => {
                assert_eq!(directory, folder.path());
            }
            other => panic!("a repository folder gets the changes panel, got {other:?}"),
        }

        // A folder inside somebody else's repository stays quiet, and says so
        // in its own words rather than reporting a busy repository.
        engine.folder_repo_statuses.insert(
            "sa1".to_string(),
            crate::git::FolderRepoStatus::InsideRepoRootedElsewhere,
        );
        match engine.session_git_access("sa1") {
            Some(SessionGitAccess::NoRepository { quiet_reason, .. }) => {
                assert!(quiet_reason.contains("rooted elsewhere"), "{quiet_reason}");
                assert!(!quiet_reason.to_lowercase().contains("busy"));
            }
            other => panic!("a nested folder must stay quiet, got {other:?}"),
        }

        assert!(engine.session_git_access("nope").is_none());
    }

    /// A provider the client made up is refused, rather than being spawned as a
    /// command.
    ///
    /// This is the only create path that takes a provider from the request, and
    /// an unconfigured name falls back to being used AS THE COMMAND, so without
    /// the check an HTTP body could name any executable on the server's PATH.
    #[test]
    fn a_standalone_create_refuses_a_provider_that_is_not_configured() {
        let (engine, tmp) = test_engine();
        let folder = tmp.path().join("plain");
        std::fs::create_dir_all(&folder).expect("folder");
        let folder = folder.to_string_lossy().to_string();

        let err = engine
            .plan_standalone_agent(&folder, "", Some("not-a-provider"))
            .expect_err("an unconfigured provider is refused");
        assert!(
            err.to_string().contains("not configured"),
            "and says so in the user's terms, got: {err}"
        );

        // A configured one still plans, so the guard is about the value and not
        // about overrides in general.
        let configured = engine
            .config
            .providers
            .commands
            .keys()
            .next()
            .expect("the default config configures providers")
            .clone();
        engine
            .plan_standalone_agent(&folder, "", Some(&configured))
            .expect("a configured provider is accepted");
    }

    /// "Nobody has looked yet" reads as a wait, not as a fault on the user's
    /// machine. A freshly created agent in a healthy repository spends a moment
    /// in this state, and it used to be told to check that git was installed.
    #[test]
    fn an_unprobed_folder_is_not_reported_as_a_broken_git() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_standalone_session("sa1", "/home/someone/notes"));

        let status = engine.folder_repo_status("sa1");
        assert_eq!(status, crate::git::FolderRepoStatus::Unprobed);
        // Gates exactly as Indeterminate does: fail closed on both.
        assert!(!status.mutations_allowed());
        assert!(!status.changes_panel_works());
        assert!(!status.git_can_see_path());
        let reason = status.quiet_reason();
        assert!(
            reason.contains("still looking"),
            "it reads as a wait, got: {reason}"
        );
        assert!(
            !reason.contains("git is installed"),
            "and never accuses the user's machine, got: {reason}"
        );
    }

    /// One probe per standalone agent at a time.
    ///
    /// Every question about the folder asks for a refresh, and the web's
    /// changed-files poller asks every two seconds for as long as the panel is
    /// open, so without the in-flight key this was an unbounded loop of OS
    /// threads each running up to four git subprocesses, in exactly the case
    /// where the feature works (a folder that IS a repository).
    #[test]
    fn a_second_folder_repo_probe_is_a_no_op_while_the_first_is_in_flight() {
        let (mut engine, tmp) = test_engine();
        let folder = tmp.path().join("plain-folder");
        std::fs::create_dir_all(&folder).expect("folder");
        let folder = folder.to_string_lossy().to_string();
        engine
            .sessions
            .push(sample_standalone_session("sa1", &folder));

        // Three asks in a row, as the poller would make them.
        engine.spawn_folder_repo_probe("sa1");
        engine.spawn_folder_repo_probe("sa1");
        engine.spawn_folder_repo_probe("sa1");

        let first = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the first ask really does probe");
        assert!(matches!(
            &first,
            WorkerEvent::FolderRepoStatusReady { session_id, .. } if session_id == "sa1"
        ));
        assert_eq!(
            engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .err()
                .map(|_| ()),
            Some(()),
            "the asks that arrived while a probe was in flight spawned nothing"
        );

        // The answer being applied is what re-arms the next probe: the key is
        // cleared by the handler, not by a timer.
        engine.process_worker_event(first);
        engine.spawn_folder_repo_probe("sa1");
        assert!(
            engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_ok(),
            "a later ask probes again once the previous answer landed"
        );
    }

    /// A probe whose THREAD never started must give the in-flight key back.
    ///
    /// `spawn_loop_worker` does not clear keys on a spawn failure, and only the
    /// `FolderRepoStatusReady` handler clears this one, so a single failed spawn
    /// used to pin the key for the rest of the run: the folder answered
    /// `Unprobed` forever, its changes panel stayed quiet, its mutations were
    /// refused, and nothing short of a restart healed it.
    #[test]
    fn a_folder_repo_probe_that_cannot_spawn_re_arms_the_next_ask() {
        let (mut engine, tmp) = test_engine();
        let folder = tmp.path().join("plain-folder");
        std::fs::create_dir_all(&folder).expect("folder");
        let folder = folder.to_string_lossy().to_string();
        engine
            .sessions
            .push(sample_standalone_session("sa1", &folder));

        engine
            .force_loop_worker_spawn_failure
            .store(true, std::sync::atomic::Ordering::SeqCst);
        engine.spawn_folder_repo_probe("sa1");
        assert!(
            !engine.is_in_flight(&InFlightKey::FolderRepoProbe("sa1".to_string())),
            "a probe that never started must not keep holding its slot"
        );

        // And the next ask really does probe, without a restart or an event
        // that can no longer arrive.
        engine.spawn_folder_repo_probe("sa1");
        assert!(
            engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_ok(),
            "the ask after a failed spawn classifies the folder"
        );
    }

    /// The stored verdict goes with the session, so a deleted agent leaves no
    /// residue in the map.
    #[test]
    fn deleting_an_agent_drops_its_folder_repo_verdict() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_standalone_session("sa1", "/home/someone/notes");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine
            .folder_repo_statuses
            .insert("sa1".to_string(), crate::git::FolderRepoStatus::NoRepo);

        engine
            .finish_delete_session("sa1")
            .expect("delete the session");
        assert!(
            !engine.folder_repo_statuses.contains_key("sa1"),
            "the verdict is runtime state keyed by session id and goes with it"
        );
    }

    /// A standalone agent's name is not a refname, so renaming it is not held
    /// to the refname rules.
    ///
    /// The asymmetry this fixes was a one-way door: creation deliberately takes
    /// a standalone title verbatim (folder names legally contain spaces, dots
    /// and punctuation a ref cannot), while rename ran the ref validator, so a
    /// title dux itself had minted could not be typed back. Clearing it made it
    /// worse, because the fallback label is the folder's name, which the
    /// validator also refuses.
    #[test]
    fn renaming_a_standalone_agent_is_not_held_to_the_refname_rules() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_standalone_session("sa1", "/home/someone/My Notes"));

        // A name dux itself can mint from a folder called "My Notes".
        let plan = engine.prepare_branch_rename("sa1", "My Notes (2026)", true);
        assert!(
            matches!(&plan, BranchRenamePlan::TitleWritten { name, sync_branches: false } if name == "My Notes (2026)"),
            "a standalone rename writes the title and syncs no branch, got {plan:?}"
        );
        assert_eq!(
            engine.sessions[0].title.as_deref(),
            Some("My Notes (2026)"),
            "and the title actually changed"
        );

        // Empty is still refused: every row label falls back through a branch
        // name this agent does not have, so a nameless agent is not allowed.
        assert!(matches!(
            engine.prepare_branch_rename("sa1", "   ", true),
            BranchRenamePlan::Rejected(BranchRenameRejection::EmptyName)
        ));
    }

    /// A MANAGED agent keeps the refname rules, because its name really does
    /// become a git branch.
    #[test]
    fn renaming_a_managed_agent_still_enforces_the_refname_rules() {
        let (mut engine, _tmp) = test_engine();
        engine
            .projects
            .push(crate::engine::test_support::sample_project("p1", "/tmp/p1"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        assert!(matches!(
            engine.prepare_branch_rename("s1", "My Notes", true),
            BranchRenamePlan::Rejected(BranchRenameRejection::MalformedName)
        ));
    }

    /// Every pull-request route refuses a standalone id with a purposeful
    /// message rather than an accidental error, and refuses it BEFORE the
    /// one-attach-at-a-time mutual block: the ordering is existence, then
    /// workspace, then in-flight, so a standalone agent is never told to "wait
    /// for the attach to finish" about a feature it does not have.
    #[test]
    fn every_pull_request_route_refuses_a_standalone_agent_before_the_in_flight_check() {
        let (mut engine, _tmp, _folder) = engine_with_a_standalone_agent();
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;
        // Arm the mutual block that the workspace gate must beat.
        engine.mark_in_flight(InFlightKey::PrAttach("sa1".to_string()));

        let detach = engine.clear_pull_request_override("sa1").unwrap_err();
        let resume = engine.resume_pr_autodetection("sa1").unwrap_err();
        let attach = engine
            .dispatch_attach_pull_request("sa1", "https://github.com/o/r/pull/1")
            .unwrap_err();

        for (what, err) in [("detach", detach), ("resume", resume), ("attach", attach)] {
            let message = err.to_string();
            assert!(
                message.contains("standalone agent"),
                "{what} must say what this agent is, got {message:?}"
            );
            assert!(
                message.contains("no branch"),
                "{what} must say why there is no pull request, got {message:?}"
            );
            assert!(
                !message.contains("still resolving"),
                "{what} must refuse on the workspace before the in-flight block, got {message:?}"
            );
        }
    }

    /// The same gate for an id that names no agent at all: existence still
    /// wins, so an unknown id keeps its unknown-session error (the surfaces'
    /// 404) rather than being told it is standalone.
    #[test]
    fn an_unknown_id_still_gets_the_unknown_session_error() {
        let (mut engine, _tmp, _folder) = engine_with_a_standalone_agent();
        let err = engine
            .clear_pull_request_override("nope")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown session"), "got {err:?}");
    }

    #[test]
    fn the_branch_watcher_never_enrols_a_standalone_agent() {
        let (engine, _tmp, _folder) = engine_with_a_standalone_agent();
        engine.update_branch_sync_sessions();
        let enrolled: Vec<String> = engine
            .branch_sync_sessions
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect();
        assert_eq!(
            enrolled,
            vec!["s1".to_string()],
            "a folder has no branch to watch"
        );
    }

    #[test]
    fn the_pull_request_watcher_never_enrols_a_standalone_agent() {
        let (engine, _tmp, _folder) = engine_with_a_standalone_agent();
        engine.update_pr_sync_sessions();
        let enrolled: Vec<String> = engine
            .pr_sync_sessions
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect();
        assert_eq!(
            enrolled,
            vec!["s1".to_string()],
            "a folder has no branch to open a pull request from"
        );
    }

    /// The refs watcher puts an inotify watch on `.git/refs/heads` per session.
    /// A standalone agent must not get one even when its folder IS a repository
    /// (the watch exists to notice the AGENT's branch moving, and there is no
    /// agent branch here).
    #[test]
    fn the_refs_watcher_never_watches_a_standalone_agents_folder() {
        let (mut engine, _tmp, folder) = engine_with_a_standalone_agent();
        init_plain_repo(folder.path());
        std::fs::create_dir_all(folder.path().join(".git").join("refs").join("heads")).unwrap();
        engine.spawn_refs_watcher();
        let watched: Vec<String> = engine.refs_watch_paths.values().cloned().collect();
        assert!(
            !watched.contains(&"sa1".to_string()),
            "a standalone agent has no agent branch for a refs watch to be about, got {watched:?}"
        );
    }

    /// The one-shot pull-request check is reachable directly (the refs-watcher
    /// event routes into it), so it refuses a standalone id itself rather than
    /// relying on the batched enumerator having skipped it.
    #[test]
    fn a_one_shot_pull_request_check_refuses_a_standalone_agent() {
        let (mut engine, _tmp, _folder) = engine_with_a_standalone_agent();
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;
        engine.spawn_pr_check_for_session("sa1", Duration::from_secs(0));
        assert!(
            !engine.is_in_flight(&InFlightKey::PrCheck("sa1".to_string())),
            "no gh call may be dispatched for an agent with no branch"
        );
        assert!(
            !engine.pr_last_checked.contains_key("sa1"),
            "and it must not even stamp a debounce, which would imply a check happened"
        );
    }

    fn init_plain_repo(path: &std::path::Path) {
        let out = crate::git::test_support::git_command()
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn prepare_branch_rename_rejects_empty_and_malformed_names() {
        // Validation is core-owned: an empty (or whitespace-only) name and a
        // malformed one are both refused before any state change, so the
        // optimistic title is never written for an invalid request.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session.title = Some("keep-me".into());
        engine.sessions.push(session);

        for empty in ["", "   "] {
            let plan = engine.prepare_branch_rename("s1", empty, true);
            assert_eq!(
                plan,
                BranchRenamePlan::Rejected(BranchRenameRejection::EmptyName)
            );
        }
        let plan = engine.prepare_branch_rename("s1", "-nope", true);
        assert_eq!(
            plan,
            BranchRenamePlan::Rejected(BranchRenameRejection::MalformedName)
        );

        // Nothing was mutated by a refused request.
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s.title.as_deref(), Some("keep-me"));
        assert_eq!(s.branch_name().expect("managed test session"), "old-branch");
        assert!(engine.rename_expected.is_empty());
    }

    #[test]
    fn prepare_branch_rename_rejects_when_rename_in_flight() {
        // The overlap guard mirrors `apply_rename_session`: a second concurrent
        // rename for the same session is refused so two `git branch -m` runs
        // can't race on one worktree.
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "p1", "old-branch"));
        engine.mark_in_flight(InFlightKey::BranchRename("s1".into()));

        let plan = engine.prepare_branch_rename("s1", "new-name", true);
        assert_eq!(
            plan,
            BranchRenamePlan::Rejected(BranchRenameRejection::AlreadyInFlight)
        );
    }

    #[test]
    fn prepare_branch_rename_title_only_writes_title_and_requests_sync() {
        // `rename_branch == false`: the display title is written and persisted,
        // the branch is left alone, and the surface is asked to refresh
        // branch-sync (matching the pre-extraction `else` arm).
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session.title = Some("before".into());
        engine.sessions.push(session);

        let plan = engine.prepare_branch_rename("s1", "  after  ", false);
        assert_eq!(
            plan,
            BranchRenamePlan::TitleWritten {
                name: "after".into(),
                sync_branches: true,
            }
        );

        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s.title.as_deref(), Some("after"), "title written");
        assert_eq!(
            s.branch_name().expect("managed test session"),
            "old-branch",
            "branch untouched"
        );
        assert!(
            engine.rename_expected.is_empty(),
            "no expectation for a title-only change"
        );
        assert!(
            !engine.is_in_flight(&InFlightKey::BranchRename("s1".into())),
            "prepare must not mark in-flight; the worker spawn does"
        );
        // The title write was persisted (reload sees it).
        let loaded = engine.session_store.load_sessions().expect("load");
        let stored = loaded.iter().find(|s| s.id == "s1").expect("stored s1");
        assert_eq!(stored.title.as_deref(), Some("after"));
    }

    #[test]
    fn prepare_branch_rename_noop_when_name_equals_branch() {
        // A branch rename whose new name already equals the current branch is a
        // no-op on git: the title is written but no expectation is stashed and
        // the surface is NOT asked to sync (matching the name-equals-branch
        // early return).
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "p1", "same-branch"));

        let plan = engine.prepare_branch_rename("s1", "same-branch", true);
        assert_eq!(
            plan,
            BranchRenamePlan::TitleWritten {
                name: "same-branch".into(),
                sync_branches: false,
            }
        );
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s.title.as_deref(), Some("same-branch"));
        assert!(engine.rename_expected.is_empty());
    }

    #[test]
    fn prepare_branch_rename_dispatches_and_stashes_expectation() {
        // The real-rename path: the optimistic title is written, the expectation
        // is stashed (so branch-sync skips our own in-progress rename), and the
        // dispatch carries exactly the parameters the surface hands to
        // `git::rename_branch`, plus the pre-write title for the unwind path.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "old-branch");
        session.title = Some("previous-title".into());
        engine.sessions.push(session);

        let plan = engine.prepare_branch_rename("s1", "new-name", true);
        assert_eq!(
            plan,
            BranchRenamePlan::RenameBranch(BranchRenameDispatch {
                session_id: "s1".into(),
                worktree_path: "/tmp/s1-worktree".into(),
                old_branch: "old-branch".into(),
                new_branch: "new-name".into(),
                previous_title: Some("previous-title".into()),
            })
        );

        // Optimistic title written; expectation stashed; in-flight NOT yet set.
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s.title.as_deref(), Some("new-name"));
        let exp = engine
            .rename_expected
            .get("s1")
            .expect("expectation stashed");
        assert_eq!(exp.old_branch, "old-branch");
        assert_eq!(exp.new_branch, "new-name");
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));

        // The unwind primitive restores the pre-write title and drops the stash.
        engine.revert_optimistic_rename("s1", Some("previous-title".into()));
        assert!(engine.rename_expected.is_empty());
        let s = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s.title.as_deref(), Some("previous-title"));
    }

    #[test]
    fn prepare_branch_rename_noop_when_session_missing() {
        // If the session vanished, prepare mutates nothing and returns Noop so
        // the surface stays silent (the pre-extraction early return).
        let (mut engine, _tmp) = test_engine();
        let plan = engine.prepare_branch_rename("ghost", "new-name", true);
        assert_eq!(plan, BranchRenamePlan::Noop);
        assert!(engine.rename_expected.is_empty());
    }

    #[test]
    fn validate_project_add_path_rejects_repo_subdirectories_and_git_dirs() {
        // Catches the goal-5 gate missing (a subfolder registered as a
        // project) plus the panel's hole (<repo>/.git accepted because
        // --show-toplevel fails there and the gate fails open).
        let (engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().unwrap();
        init_plain_repo(repo.path());
        let sub = repo.path().join("src");
        std::fs::create_dir(&sub).unwrap();

        let err = engine
            .validate_project_add_path(sub.to_string_lossy().as_ref())
            .expect_err("a repo subdirectory must be rejected");
        assert!(
            err.contains("is inside the git repository at"),
            "error must name the root, got: {err}"
        );
        assert!(
            err.contains(
                repo.path()
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            ),
            "error must contain the repo root path, got: {err}"
        );

        let git_dir = repo.path().join(".git");
        let err = engine
            .validate_project_add_path(git_dir.to_string_lossy().as_ref())
            .expect_err("<repo>/.git must be rejected");
        assert!(
            err.contains("git's internal directory"),
            "error must explain the git-dir rejection, got: {err}"
        );
    }

    #[test]
    fn validate_project_add_path_still_accepts_a_bare_repo_root() {
        // Regression guard for shipped bare-repo support.
        let (engine, _tmp) = test_engine();
        let bare = tempfile::tempdir().unwrap();
        let out = crate::git::test_support::git_command()
            .args(["init", "--bare"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        engine
            .validate_project_add_path(bare.path().to_string_lossy().as_ref())
            .expect("a bare repository root must remain addable");
    }

    #[test]
    fn validate_project_init_path_rejects_everything_but_a_plain_folder() {
        // Catches `git init` atop or inside existing repositories.
        let (mut engine, _tmp) = test_engine();

        let repo = tempfile::tempdir().unwrap();
        init_plain_repo(repo.path());
        let err = engine
            .validate_project_init_path(repo.path().to_string_lossy().as_ref())
            .expect_err("a work-tree root must not be re-initialized");
        assert!(err.contains("already a git repository"), "got: {err}");

        let bare = tempfile::tempdir().unwrap();
        let out = crate::git::test_support::git_command()
            .args(["init", "--bare"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let err = engine
            .validate_project_init_path(bare.path().to_string_lossy().as_ref())
            .expect_err("a bare root must not be re-initialized");
        assert!(err.contains("already a git repository"), "got: {err}");

        let sub = repo.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        let err = engine
            .validate_project_init_path(sub.to_string_lossy().as_ref())
            .expect_err("a repo subdirectory must not be initialized");
        assert!(
            err.contains("is inside the git repository at"),
            "got: {err}"
        );

        let err = engine
            .validate_project_init_path(repo.path().join(".git").to_string_lossy().as_ref())
            .expect_err("a .git directory must not be initialized");
        assert!(err.contains("git's internal directory"), "got: {err}");

        let err = engine
            .validate_project_init_path("/definitely/not/an/existing/folder")
            .expect_err("a nonexistent path must be rejected");
        assert!(err.contains("not an existing folder"), "got: {err}");

        // An already-registered plain folder must be rejected too.
        let plain = tempfile::tempdir().unwrap();
        let project = sample_project("p1", plain.path().to_string_lossy().as_ref());
        engine.projects.push(project);
        let err = engine
            .validate_project_init_path(plain.path().to_string_lossy().as_ref())
            .expect_err("an already-registered path must be rejected");
        assert!(err.contains("already registered"), "got: {err}");

        // And a fresh plain folder passes, returning the canonical path.
        let fresh = tempfile::tempdir().unwrap();
        let ok = engine
            .validate_project_init_path(fresh.path().to_string_lossy().as_ref())
            .expect("a plain folder must validate for init");
        assert_eq!(ok, fresh.path().canonicalize().unwrap());
    }

    #[test]
    fn session_is_streaming_rolls_up_any_tab() {
        let (mut engine, _tmp) = test_engine();
        // An extra tab of session s1 (its own tab id, session_id = s1) is streaming,
        // while the session-slot tab (s1) is idle.
        engine.agent_tabs.insert(
            TabId::new("s1-x"),
            AgentTab {
                id: "s1-x".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        engine
            .pty_activity
            .insert("s1-x".to_string(), Instant::now());

        assert!(
            !engine.is_agent_streaming("s1"),
            "the session-slot tab itself is idle"
        );
        assert!(
            engine.session_is_streaming("s1"),
            "the any-tab rollup must see the streaming extra tab"
        );
    }

    #[test]
    fn is_typing_honors_the_input_window_and_is_per_id() {
        let (mut engine, _tmp) = test_engine();

        // Fresh keystroke → typing.
        engine.pty_input.insert("a".to_string(), Instant::now());
        assert!(engine.is_typing("a"));

        // Older than the suppression window → not typing.
        engine.pty_input.insert(
            "b".to_string(),
            Instant::now() - (AGENT_INPUT_SUPPRESSION_WINDOW + Duration::from_millis(50)),
        );
        assert!(!engine.is_typing("b"));

        // No entry → not typing, and each id is independent of the others.
        assert!(!engine.is_typing("c"));
        assert!(
            engine.is_typing("a"),
            "one id aging out must not affect another id's typing state"
        );
    }

    #[test]
    fn session_is_typing_rolls_up_any_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.agent_tabs.insert(
            TabId::new("s1-x"),
            AgentTab {
                id: "s1-x".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );

        // Only the extra tab is being typed into; the session-slot tab is idle.
        engine.pty_input.insert("s1-x".to_string(), Instant::now());
        assert!(
            !engine.is_typing("s1"),
            "the session-slot tab itself is not being typed into"
        );
        assert!(
            engine.session_is_typing("s1"),
            "the any-tab rollup must see the typed extra tab"
        );

        // Typing into the session-slot tab alone also rolls up.
        engine.pty_input.remove("s1-x");
        engine.pty_input.insert("s1".to_string(), Instant::now());
        assert!(engine.session_is_typing("s1"));
    }

    #[test]
    fn a_terminal_id_reads_as_working_or_typing_via_the_shared_predicates() {
        let (mut engine, _tmp) = test_engine();
        let tid = "term-1";

        // Fresh output and no input: the terminal is working. Terminals never
        // emit OSC progress, so is_agent_streaming reduces to fresh, non-echo
        // output — no terminal special-casing is needed.
        engine.pty_activity.insert(tid.to_string(), Instant::now());
        assert!(
            engine.is_agent_streaming(tid),
            "fresh terminal output reads as working"
        );
        assert!(!engine.is_typing(tid));

        // A keystroke within the window flips it to typing and voids working: the
        // echo of the user's own typing must not read as the terminal working.
        engine.note_pty_input(tid);
        assert!(engine.is_typing(tid), "a fresh keystroke reads as typing");
        assert!(
            !engine.is_agent_streaming(tid),
            "the typing echo must not read as the terminal working"
        );
    }

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
            TabId::new("s1"),
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
    fn visible_output_overrides_a_false_progress_report() {
        let (mut engine, _tmp) = test_engine();
        // TEXT WINS: real visible output (pty_activity, stamped only on visible grid
        // changes) means the agent is producing work, even if its own OSC 9;4 report
        // claims "idle". An agent that misreports or stops reporting while still
        // printing must still light the indicator.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_progress.insert(
            TabId::new("s1"),
            ProgressReport {
                working: false,
                at: Instant::now(),
            },
        );
        assert!(
            engine.is_agent_streaming("s1"),
            "live visible output must override a stale-claiming idle progress report"
        );
    }

    #[test]
    fn typing_echo_defers_to_the_progress_report() {
        let (mut engine, _tmp) = test_engine();
        // Recent output that is merely the echo of the user's keystrokes is voided,
        // so the OSC report is consulted as the fallback. A fresh "working" report
        // then lights it (the agent is thinking about what the user just typed).
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_input.insert("s1".to_string(), Instant::now());
        engine.pty_progress.insert(
            TabId::new("s1"),
            ProgressReport {
                working: true,
                at: Instant::now(),
            },
        );
        assert!(
            engine.is_agent_streaming("s1"),
            "with output suppressed as typing echo, a fresh working report wins"
        );
    }

    #[test]
    fn stale_progress_report_falls_back_to_heuristic() {
        let (mut engine, _tmp) = test_engine();
        // The last progress report is older than the authority window, so the
        // output-activity heuristic takes over again.
        engine.pty_progress.insert(
            TabId::new("s1"),
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

    /// The SGR wheel report a viewer forwards to a child that has mouse
    /// reporting on. One notch of scroll, at an arbitrary cell.
    const WHEEL_REPORT: &[u8] = b"\x1b[<64;10;5M";

    /// An SGR MOTION report: the pointer moved with no button held (bit 32 set,
    /// button bits 3 = "no button"). An app using any-event tracking gets one of
    /// these for every cell the pointer crosses.
    const MOTION_REPORT: &[u8] = b"\x1b[<35;10;5M";

    #[test]
    fn moving_the_pointer_arms_no_suppression_at_all() {
        let (mut engine, _tmp) = test_engine();

        engine.note_pty_write("s1", MOTION_REPORT);

        assert!(!engine.is_typing("s1"), "moving the pointer is not typing");
        assert!(
            !engine.recent_pointer_input("s1"),
            "motion is not a discrete user action and fires continuously, so it \
             must not suppress the working inference at all"
        );
    }

    /// An SGR BUTTON report: the left button pressed, the shape of a click or a
    /// phone tap.
    const CLICK_REPORT: &[u8] = b"\x1b[<0;10;5M";

    #[test]
    fn a_forwarded_pointer_report_is_never_typing() {
        let (mut engine, _tmp) = test_engine();

        assert_eq!(
            engine.note_pty_write("s1", WHEEL_REPORT),
            crate::pty::PtyWriteKind::Pointer(crate::pty::PointerReport::Wheel)
        );
        assert!(
            !engine.is_typing("s1"),
            "scrolling is not typing: a forwarded wheel must never light Typing"
        );
        assert!(
            !engine.pty_input.contains_key("s1"),
            "a pointer report must not stamp the typing window at all"
        );
        assert!(
            engine.recent_pointer_input("s1"),
            "it must stamp the pointer window instead"
        );
    }

    /// A click causes one repaint and is over, so it must hand the heuristic
    /// back promptly: a user who clicks something that STARTS work should see
    /// the Working cue appear within a moment, not after a second and a quarter.
    /// A scroll is continuous and keeps its long window.
    #[test]
    fn a_click_stops_suppressing_long_before_a_scroll_does() {
        assert!(
            POINTER_CLICK_REPAINT_WINDOW < POINTER_REPAINT_WINDOW,
            "a click is one discrete act; a scroll keeps going"
        );

        let (mut engine, _tmp) = test_engine();
        engine.note_pty_write("click", CLICK_REPORT);
        engine.note_pty_write("wheel", WHEEL_REPORT);

        // Both suppress right away.
        assert!(engine.recent_pointer_input("click"));
        assert!(engine.recent_pointer_input("wheel"));

        // Age both by a hair over the click window, without sleeping.
        let aged = POINTER_CLICK_REPAINT_WINDOW + Duration::from_millis(50);
        for id in ["click", "wheel"] {
            let stamp = engine.pty_pointer.get_mut(id).expect("stamp");
            stamp.at -= aged;
        }

        assert!(
            !engine.recent_pointer_input("click"),
            "the click's short window has lapsed"
        );
        assert!(
            engine.recent_pointer_input("wheel"),
            "the wheel's long window has not"
        );
    }

    /// Motion arms nothing, so it can never blank a working agent no matter how
    /// many reports an any-event-tracking app sends.
    #[test]
    fn a_busy_agent_stays_working_while_the_pointer_moves_over_it() {
        let (mut engine, _tmp) = test_engine();

        for _ in 0..20 {
            assert_eq!(
                engine.note_pty_write("s1", MOTION_REPORT),
                crate::pty::PtyWriteKind::Pointer(crate::pty::PointerReport::Motion)
            );
        }
        engine.pty_activity.insert("s1".to_string(), Instant::now());

        assert!(
            !engine.pty_pointer.contains_key("s1"),
            "motion must not even create a pointer entry"
        );
        assert!(
            engine.is_agent_streaming("s1"),
            "moving the mouse across a busy agent's pane must not read as idle"
        );
    }

    #[test]
    fn a_repaint_after_a_forwarded_wheel_is_not_working_without_a_progress_report() {
        let (mut engine, _tmp) = test_engine();

        // The user scrolls a child that owns the mouse. It repaints its whole
        // grid, so `pty_activity` is stamped, but that output is the user's own
        // scroll coming back, not the agent starting work.
        engine.note_pty_write("s1", WHEEL_REPORT);
        // Reuse the pointer stamp's OWN instant for the activity stamp rather
        // than reading the clock a second time. Two adjacent `Instant::now()`
        // calls make the assertion depend on how long the gap between these two
        // lines happened to be: the activity window outlives the pointer one, so
        // a slow enough gap would lapse the suppression and invert the result.
        let scrolled_at = engine.pty_pointer["s1"].at;
        engine.pty_activity.insert("s1".to_string(), scrolled_at);

        assert!(
            !engine.is_agent_streaming("s1"),
            "a repaint provoked by the user's own scroll must not read as working"
        );
    }

    /// While the heuristic is suppressed the agent's own report decides, in BOTH
    /// directions. The `false` half is what gives this test teeth: with fresh
    /// activity and no suppression the streaming branch would return true, so
    /// removing the suppression fails it. (The `true` half alone could not: the
    /// old code returned true through the streaming branch anyway.)
    #[test]
    fn a_scrolled_agent_defers_to_its_own_progress_report_in_both_directions() {
        for (reported, expected) in [(true, true), (false, false)] {
            let (mut engine, _tmp) = test_engine();

            engine.note_pty_write("s1", WHEEL_REPORT);
            let scrolled_at = engine.pty_pointer["s1"].at;
            engine.pty_activity.insert("s1".to_string(), scrolled_at);
            engine.pty_progress.insert(
                TabId::new("s1"),
                ProgressReport {
                    working: reported,
                    at: scrolled_at,
                },
            );

            assert_eq!(
                engine.is_agent_streaming("s1"),
                expected,
                "a scrolled agent reporting working={reported} must read as {expected}, \
                 whatever the repaint text says"
            );
        }
    }

    #[test]
    fn the_heuristic_returns_once_the_pointer_window_lapses() {
        let (mut engine, _tmp) = test_engine();

        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_pointer.insert(
            "s1".to_string(),
            PointerStamp {
                at: Instant::now() - (POINTER_REPAINT_WINDOW + Duration::from_millis(50)),
                window: POINTER_REPAINT_WINDOW,
            },
        );

        assert!(
            engine.is_agent_streaming("s1"),
            "once scrolling stops, ongoing output reads as working again"
        );
    }

    #[test]
    fn note_pty_write_routes_each_kind_to_its_own_window() {
        let (mut engine, _tmp) = test_engine();

        // A real keystroke keeps its existing meaning exactly: it lights Typing
        // and suppresses the echo, and it touches the pointer window not at all.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        assert_eq!(
            engine.note_pty_write("s1", b"x"),
            crate::pty::PtyWriteKind::Typing
        );
        assert!(engine.is_typing("s1"));
        assert!(!engine.recent_pointer_input("s1"));
        assert!(
            !engine.is_agent_streaming("s1"),
            "typing still suppresses its own echo, unchanged"
        );

        // A focus report is neither, and stamps nothing.
        assert_eq!(
            engine.note_pty_write("s2", b"\x1b[I"),
            crate::pty::PtyWriteKind::Ignored
        );
        assert!(!engine.is_typing("s2"));
        assert!(!engine.recent_pointer_input("s2"));
    }

    #[test]
    fn note_agent_viewed_clears_attention_immediately() {
        let (mut engine, _tmp) = test_engine();
        engine.needs_attention.insert(TabId::new("s1"));
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
        assert!(!attention_engaged(
            &viewed,
            &typed,
            TabIdRef::new("s1"),
            now
        ));

        // Fresh view → engaged.
        viewed.insert(TabId::new("s1"), now);
        assert!(attention_engaged(&viewed, &typed, TabIdRef::new("s1"), now));

        // Stale view → not engaged.
        viewed.insert(
            TabId::new("s1"),
            now - (ATTENTION_ENGAGED_WINDOW + Duration::from_millis(50)),
        );
        assert!(!attention_engaged(
            &viewed,
            &typed,
            TabIdRef::new("s1"),
            now
        ));

        // Fresh typing alone → engaged.
        typed.insert("s1".to_string(), now);
        assert!(attention_engaged(&viewed, &typed, TabIdRef::new("s1"), now));

        // Stale typing → not engaged.
        typed.insert(
            "s1".to_string(),
            now - (AGENT_INPUT_SUPPRESSION_WINDOW + Duration::from_millis(50)),
        );
        assert!(!attention_engaged(
            &viewed,
            &typed,
            TabIdRef::new("s1"),
            now
        ));
    }

    #[test]
    fn clear_tab_runtime_drops_attention_and_progress() {
        let (mut engine, _tmp) = test_engine();
        engine.needs_attention.insert(TabId::new("s1"));
        engine.pty_progress.insert(
            TabId::new("s1"),
            ProgressReport {
                working: true,
                at: Instant::now(),
            },
        );
        engine.agent_viewed.insert(TabId::new("s1"), Instant::now());
        engine.clear_tab_runtime(TabIdRef::new("s1"));
        assert!(!engine.tab_needs_attention("s1"));
        assert!(!engine.pty_progress.contains_key(TabIdRef::new("s1")));
        assert!(!engine.agent_viewed.contains_key(TabIdRef::new("s1")));
    }

    #[test]
    fn note_agent_viewed_if_known_gates_on_a_real_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine.needs_attention.insert(TabId::new("s1-slot"));

        // A bogus id (stale deep link, race, retry) is a no-op: it neither stamps
        // `agent_viewed` nor touches the flag.
        engine.note_agent_viewed_if_known("does-not-exist");
        assert!(
            engine.agent_viewed.is_empty(),
            "a bogus id must not leak an agent_viewed entry"
        );
        assert!(engine.tab_needs_attention("s1-slot"));

        // A real tab id both stamps engagement and clears the flag.
        engine.note_agent_viewed_if_known("s1-slot");
        assert!(engine.agent_viewed.contains_key(TabIdRef::new("s1-slot")));
        assert!(!engine.tab_needs_attention("s1-slot"));
    }

    #[test]
    fn apply_attention_decision_sets_flag_when_not_engaged() {
        let mut set = HashSet::new();
        apply_attention_decision(&mut set, &[TabId::new("s1")], true, |_| false);
        assert!(
            set.contains(TabIdRef::new("s1")),
            "a fired, unengaged tab must be flagged"
        );
    }

    #[test]
    fn apply_attention_decision_suppresses_and_clears_when_engaged() {
        let mut set = HashSet::new();
        set.insert(TabId::new("s1"));
        // Engaged: the fired signal is suppressed AND the existing flag is cleared.
        apply_attention_decision(&mut set, &[TabId::new("s1")], true, |_| true);
        assert!(
            !set.contains(TabIdRef::new("s1")),
            "engagement must both suppress the new signal and clear the old flag"
        );
    }

    #[test]
    fn apply_attention_decision_clears_stale_flag_without_a_new_signal() {
        let mut set = HashSet::new();
        set.insert(TabId::new("s1"));
        // No new signal this tick, but the user is now engaged → clear.
        apply_attention_decision(&mut set, &[], true, |tab| tab.as_str() == "s1");
        assert!(!set.contains(TabIdRef::new("s1")));
    }

    #[test]
    fn apply_attention_decision_retains_flag_while_unengaged() {
        let mut set = HashSet::new();
        set.insert(TabId::new("s1"));
        apply_attention_decision(&mut set, &[], true, |_| false);
        assert!(
            set.contains(TabIdRef::new("s1")),
            "an unviewed flag must persist across ticks"
        );
    }

    #[test]
    fn apply_attention_decision_disabled_clears_everything() {
        let mut set = HashSet::new();
        set.insert(TabId::new("s1"));
        apply_attention_decision(&mut set, &[TabId::new("s2")], false, |_| false);
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
        engine.providers.insert(TabId::new(tab), client);
        for _ in 0..200 {
            if engine
                .providers
                .get(TabIdRef::new(tab))
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
                .get(TabIdRef::new("s1"))
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
        assert!(engine.pty_progress.contains_key(TabIdRef::new("s1")));
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
                .get(TabIdRef::new("s1"))
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
        engine.providers.insert(TabId::new(tab), client);
        // Gate readiness on the passthrough RING being non-empty, not on the
        // progress report. The reader loop sets the progress slot earlier in the
        // same scan pass than it pushes captures into the ring, so a progress-based
        // wait can observe a half-applied pass where the ring is still empty.
        // A non-empty ring proves the capture push completed.
        for _ in 0..200 {
            if engine
                .providers
                .get(TabIdRef::new(tab))
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
        // Prove the pipe is live before claiming the clipboard is not on it: a
        // forward of nothing at all also carries no `]52;`.
        assert!(
            off.windows(4).any(|w| w == b"]9;h"),
            "notify still forwards, so the passthrough pipe is live"
        );
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
        engine
            .projects
            .push(crate::engine::test_support::sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feature");

        // Resume → a completed-action message naming provider, agent, project.
        // The AGENT is named by its display label (its title here), not by the
        // branch: a standalone agent has no branch, so every message that used
        // to reach for one now goes through the shared label rule.
        assert_eq!(
            engine.agent_reconnect_status_message(&session, true),
            "Resumed claude agent \"s1-title\" in project \"p1-name\"."
        );

        // Fresh → the no-resume variant with the /sessions hint.
        assert_eq!(
            engine.agent_reconnect_status_message(&session, false),
            "Started fresh claude session for agent \"s1-title\" in project \"p1-name\". \
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1", 24, 80)
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

    // ── gh host probe ──────────────────────────────────────────────────────

    use super::test_support::settle_gh_probe;
    use crate::gh::probe_test_support::{stand_in_gh, stand_in_gh_serving};
    use crate::gh::{GhProbe, GithubHostPolicy};

    fn eligible(engine: &Engine) -> GithubHostPolicy {
        engine.github_host_policy()
    }

    fn hosts(names: &[&str]) -> GithubHostPolicy {
        GithubHostPolicy::Hosts(names.iter().map(|n| n.to_string()).collect())
    }

    /// The off→on re-arm restores a pinned PR's badge immediately: the probe
    /// completion (the ONE place PR work is armed) re-seeds from the store, so
    /// the pin does not wait for the first sync cycle.
    #[test]
    fn probe_completion_reseeds_pinned_badges_from_the_store() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine
            .session_store
            .upsert_session(&sample_session("s1", "p1", "feat"))
            .expect("seed session");
        engine
            .session_store
            .upsert_pr_override(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 12,
                host: "github.com".to_string(),
                owner_repo: "forker/Hello-World".to_string(),
                state: "OPEN".to_string(),
                title: "Pinned".to_string(),
                url: "https://github.com/forker/Hello-World/pull/12".to_string(),
            })
            .expect("seed the override");
        // The toggle-off path cleared the in-memory badge state.
        assert!(engine.pr_statuses.is_empty());

        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: 0,
            outcome: GhProbe::Decided {
                available: true,
                policy: hosts(&["github.com"]),
            },
        });

        assert_eq!(engine.pr_statuses.get("s1").map(|p| p.number), Some(12));
        assert_eq!(engine.pr_overrides.get("s1").map(|p| p.pr_number), Some(12));
    }

    #[test]
    fn a_stale_probe_result_is_discarded_before_it_changes_anything() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_probe.generation = 2;
        engine.set_github_host_policy(hosts(&["git.company.example"]));
        engine.gh_status = GhStatus::Available;

        // An older probe finishing late, with a decisive but out-of-date answer.
        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: 1,
            outcome: GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::Hosts(Default::default()),
            },
        });

        assert_eq!(
            eligible(&engine),
            hosts(&["git.company.example"]),
            "the newer answer must survive a late older one",
        );
        assert_eq!(engine.gh_status, GhStatus::Available);

        // Including when the older probe ended in a panic rather than a normal
        // result: the synthesised event carries the same stamp and is dropped
        // by the same check.
        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: 1,
            outcome: GhProbe::Transient("gh host probe panicked: boom".to_string()),
        });
        assert_eq!(eligible(&engine), hosts(&["git.company.example"]));
        assert_eq!(engine.gh_status, GhStatus::Available);
    }

    #[test]
    fn the_first_probe_failing_transiently_reports_unavailable_rather_than_unknown() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        assert_eq!(engine.gh_status, GhStatus::Unknown);
        assert_eq!(eligible(&engine), GithubHostPolicy::DenyAll);

        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome: GhProbe::Transient("timed out".to_string()),
        });

        assert_eq!(
            engine.gh_status,
            GhStatus::Unreachable,
            "the status must leave Unknown, and say dux could not ask rather \
             than claiming the user is logged out",
        );
        assert_eq!(
            eligible(&engine),
            GithubHostPolicy::DenyAll,
            "and nothing is eligible",
        );
    }

    #[test]
    fn a_transient_failure_leaves_the_previously_computed_policy_in_place() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.set_github_host_policy(hosts(&["git.company.example"]));

        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome: GhProbe::Transient("failed to launch".to_string()),
        });

        assert_eq!(eligible(&engine), hosts(&["git.company.example"]));
        assert_eq!(engine.gh_status, GhStatus::Available);
    }

    #[test]
    fn gh_disappearing_from_the_path_denies_every_host() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.set_github_host_policy(hosts(&["git.company.example"]));

        engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome: GhProbe::NotInstalled,
        });

        assert_eq!(
            eligible(&engine),
            GithubHostPolicy::DenyAll,
            "a removed gh must not preserve the host set: dux can reach none of them",
        );
        assert_eq!(engine.gh_status, GhStatus::NotInstalled);
    }

    #[test]
    fn a_config_reload_that_enables_the_integration_re_runs_the_probe() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        // Journey: a user logged in to only their company server starts dux with
        // the integration off, then turns it on by editing the config. The
        // enterprise host must become eligible without a restart.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program = stand_in_gh_serving(dir.path(), &["git.company.example"]).into();
        engine.github_integration_enabled = false;
        engine.config.ui.github_integration = false;
        assert_eq!(eligible(&engine), GithubHostPolicy::DenyAll);

        let mut new_config = Config::default();
        new_config.ui.github_integration = true;
        engine
            .apply_reloaded_config(new_config)
            .expect("apply reloaded config");
        settle_gh_probe(&mut engine);

        assert_eq!(eligible(&engine), hosts(&["git.company.example"]));
        assert_eq!(engine.gh_status, GhStatus::Available);
    }

    #[test]
    fn a_config_reload_with_gh_already_working_is_not_a_transition() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        // Starting up is not an off-to-on transition and is already covered by
        // the surface's own boot probe, so a reload that changes nothing while
        // `gh` works must not fire another one.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 0").into();
        engine.github_integration_enabled = true;
        engine.config.ui.github_integration = true;
        engine.gh_status = GhStatus::Available;
        let before = engine.gh_probe.generation;

        let mut new_config = Config::default();
        new_config.ui.github_integration = true;
        engine
            .apply_reloaded_config(new_config)
            .expect("apply reloaded config");

        assert_eq!(engine.gh_probe.generation, before, "no new probe");
    }

    #[test]
    fn a_config_reload_re_asks_gh_while_it_is_unusable() {
        let _guard = crate::logger::level_test_guard();
        // Journey: dux booted while GitHub was rate-limiting, so the status sits
        // at Unreachable. The user fixes things and reloads the config rather
        // than waiting out the retry timer; the reload must ask again.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program = stand_in_gh_serving(dir.path(), &["github.com"]).into();
        engine.github_integration_enabled = true;
        engine.config.ui.github_integration = true;
        engine.gh_status = GhStatus::Unreachable;

        let mut new_config = Config::default();
        new_config.ui.github_integration = true;
        engine
            .apply_reloaded_config(new_config)
            .expect("apply reloaded config");
        settle_gh_probe(&mut engine);

        assert_eq!(engine.gh_status, GhStatus::Available);
    }

    #[test]
    fn the_scheduled_re_check_recovers_from_a_transient_first_probe() {
        // The measured journey: dux boots while GitHub is rate-limiting, so the
        // first probe decides nothing. Once the interval passes, the tick that
        // every surface runs asks again, and GitHub features come back with no
        // restart.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.github_integration_enabled = true;
        engine.config.ui.github_probe_interval_secs = 30;
        engine.gh_status = GhStatus::Unreachable;
        engine.gh_probe.program = stand_in_gh_serving(dir.path(), &["github.com"]).into();

        engine.gh_probe.last_probe_at = Some(Instant::now());
        engine.poll_gh_probe_schedule();
        assert_eq!(
            engine.gh_probe.generation, 0,
            "nothing is due until the interval has passed",
        );

        engine.gh_probe.last_probe_at = Some(Instant::now() - Duration::from_secs(31));
        engine.poll_gh_probe_schedule();
        assert_eq!(engine.gh_probe.generation, 1, "the re-check was spawned");
        settle_gh_probe(&mut engine);
        assert_eq!(engine.gh_status, GhStatus::Available);

        // And it stops asking, because there is nothing left to recover.
        engine.gh_probe.last_probe_at = Some(Instant::now() - Duration::from_secs(31));
        engine.poll_gh_probe_schedule();
        assert_eq!(engine.gh_probe.generation, 1, "no further probes");
    }

    /// Collect the reactions of one settled probe, flattened out of `Multi`.
    fn probe_reactions(engine: &mut Engine, outcome: GhProbe) -> Vec<EventReaction> {
        let reaction = engine.process_worker_event(WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome,
        });
        match reaction {
            EventReaction::Multi(reactions) => reactions,
            EventReaction::Nothing => Vec::new(),
            other => vec![other],
        }
    }

    #[test]
    fn only_a_transition_publishes_and_says_so() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Unreachable;

        let reactions = probe_reactions(
            &mut engine,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
            },
        );
        assert!(
            reactions
                .iter()
                .any(|r| matches!(r, EventReaction::GhAvailabilityChanged { available: true })),
            "the browser is told to refetch the document carrying gh_available",
        );
        let status = reactions
            .iter()
            .find_map(|r| match r {
                EventReaction::Status(status) => Some(status),
                _ => None,
            })
            .expect("the user is told GitHub features came back");
        assert_eq!(
            status.key.as_deref(),
            Some(crate::engine::events::GH_AVAILABILITY_STATUS_KEY),
        );
        assert!(
            status.message.contains("available"),
            "got {}",
            status.message,
        );

        // A second probe that answers the same way is not a transition, so it
        // says nothing: a re-check every few minutes must not be a toast every
        // few minutes.
        let repeat = probe_reactions(
            &mut engine,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
            },
        );
        assert!(repeat.is_empty(), "a repeat answer says nothing");
    }

    #[test]
    fn losing_gh_says_why_and_names_the_retry() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = GhStatus::Available;
        engine.config.ui.github_probe_interval_secs = 300;

        let reactions = probe_reactions(&mut engine, GhProbe::NotInstalled);
        assert!(
            reactions
                .iter()
                .any(|r| matches!(r, EventReaction::GhAvailabilityChanged { available: false }))
        );
        let status = reactions
            .iter()
            .find_map(|r| match r {
                EventReaction::Status(status) => Some(status),
                _ => None,
            })
            .expect("a status");
        assert!(
            status.message.contains("cli.github.com"),
            "the sentence must say what to do: {}",
            status.message,
        );
        assert!(
            status.message.contains("300s"),
            "and that dux keeps trying: {}",
            status.message,
        );
    }

    #[test]
    fn an_on_demand_re_check_answers_even_when_nothing_changed() {
        // Somebody pressed a button. "Still not working, here is why" is an
        // answer; silence is not.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.github_integration_enabled = true;
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 1").into();
        engine.gh_status = GhStatus::NotAuthenticated;

        let asked = engine.request_gh_recheck();
        assert_eq!(
            asked.key.as_deref(),
            Some(crate::engine::events::GH_AVAILABILITY_STATUS_KEY),
            "the request and its outcome share one key, so one replaces the other",
        );
        assert_eq!(engine.gh_probe.generation, 1, "the probe was spawned");

        let reactions = probe_reactions(&mut engine, GhProbe::Transient("HTTP 403".to_string()));
        let status = reactions
            .iter()
            .find_map(|r| match r {
                EventReaction::Status(status) => Some(status),
                _ => None,
            })
            .expect("an on-demand re-check reports its outcome");
        assert!(
            status.message.contains("403"),
            "and names the real reason: {}",
            status.message,
        );
        assert!(
            !reactions
                .iter()
                .any(|r| matches!(r, EventReaction::GhAvailabilityChanged { .. })),
            "nothing changed, so nothing is republished",
        );
    }

    #[test]
    fn an_on_demand_re_check_with_the_integration_off_asks_nothing() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = false;

        let status = engine.request_gh_recheck();
        assert_eq!(engine.gh_probe.generation, 0, "no probe was spawned");
        assert!(
            status.message.contains("turned off"),
            "and it says why: {}",
            status.message,
        );
    }

    /// The integration can be switched off while the re-check it asked for is
    /// still running. The answer that lands then has to name the setting the
    /// user just changed, not accuse them of being logged out.
    #[test]
    fn an_outcome_landing_after_the_integration_was_turned_off_says_so() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.github_integration_enabled = true;
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 1").into();
        engine.gh_status = GhStatus::NotAuthenticated;

        engine.request_gh_recheck();
        engine.github_integration_enabled = false;

        let reactions = probe_reactions(
            &mut engine,
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::DenyAll,
            },
        );
        let status = reactions
            .iter()
            .find_map(|r| match r {
                EventReaction::Status(status) => Some(status),
                _ => None,
            })
            .expect("an on-demand re-check reports its outcome");
        assert!(
            status.message.contains("turned off"),
            "the setting the user just changed is the reason: {}",
            status.message,
        );
        assert!(
            !status.message.contains("nobody is logged in"),
            "and gh's opinion of the login is beside the point: {}",
            status.message,
        );
    }

    #[test]
    fn a_probe_stamps_its_generation_before_it_is_spawned() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 0").into();
        engine.github_integration_enabled = true;

        engine.spawn_gh_status_check();
        assert_eq!(engine.gh_probe.generation, 1);
        engine.spawn_gh_status_check();
        assert_eq!(
            engine.gh_probe.generation, 2,
            "each spawn takes a fresh stamp, so two overlapping probes are ordered",
        );
    }

    /// A worker that never started posts no completion event, so the caller has
    /// to produce one. Without that the very first probe leaves the status stuck
    /// on Unknown, which the interface renders as neither available nor
    /// unavailable, and it has already burned a generation that will discard any
    /// older queued answer with nothing arriving to replace it.
    #[test]
    fn a_probe_whose_worker_cannot_start_still_reports_a_transient_result() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 0").into();
        engine.github_integration_enabled = true;
        engine.force_worker_spawn_failure = true;

        engine.spawn_gh_status_check();
        assert_eq!(engine.gh_probe.generation, 1, "the generation was burned");

        settle_gh_probe(&mut engine);
        assert_eq!(
            engine.gh_status,
            GhStatus::Unreachable,
            "a first probe that fails transiently must still leave Unknown",
        );
        assert_eq!(
            eligible(&engine),
            GithubHostPolicy::DenyAll,
            "and it decided nothing, so nothing became eligible",
        );
    }

    #[test]
    fn apply_reloaded_config_swaps_config_and_refreshes_state() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        // The reload below flips the integration on, which now re-runs the host
        // probe. Point it at a harmless stand-in so the suite never shells out
        // to the real `gh`.
        engine.gh_probe.program = stand_in_gh(dir.path(), "exit 0").into();
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
    /// Drives completion through the real `ReloadCompletionGuard` path.
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
        // wiring regression would then be visible. The surface's reload
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
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        let (mut engine, _tmp) = test_engine();
        // Baseline provider differs from the value the reload will deliver, so we
        // can prove the reloaded config actually landed (the reloaded config
        // must DIFFER from the initial one, not both be `Config::default()`).
        engine.config.defaults.provider = "claude".to_string();

        // Drive a REAL `ReloadConfig`: the engine opens the barrier and the
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

        // The deferred env change survives IN MEMORY after the reload; this is
        // the regression guard. Simulate the surface re-applying the reaction it
        // was handed: under the bug this pins, the reaction carried the BARE
        // reloaded config (no env), so re-applying would wipe the env back out
        // of memory.
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
    /// `Err` completion) through `ReloadCompletionGuard`.
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
        // The failure-with-deferral ordering: when a reload FAILS while
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
        // drop the live guard or spawn a second worker.
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
        // instead, and the reload barrier must stay open.
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
        // The reload completion guard guarantees a `ConfigReloadReady` is
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
    fn a_config_reload_adopts_a_new_logging_level() {
        let _guard = crate::logger::level_test_guard();
        crate::logger::set_level("info");
        let (mut engine, _tmp) = test_engine();

        let mut config = engine.config.clone();
        config.logging.level = "debug".to_string();
        engine
            .apply_reloaded_config(config)
            .expect("reload applies");

        assert_eq!(crate::logger::current_level(), "debug");
        crate::logger::set_level("info");
    }

    #[test]
    fn branch_sync_wait_treats_zero_as_an_idle_nap_rather_than_an_exit() {
        // `0` inside the loop must never end the thread: the guard means "a
        // thread is live", and a later retune to N is picked up by this loop.
        assert_eq!(branch_sync_nap_secs(0), BRANCH_SYNC_IDLE_NAP_SECS);
        assert!(!branch_sync_should_poll(0));
        assert_eq!(branch_sync_nap_secs(7), 7);
        assert!(branch_sync_should_poll(7));
    }

    #[test]
    fn a_config_reload_retunes_the_live_branch_sync_interval() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.branch_sync_interval = 30;
        engine.spawn_branch_sync_worker();
        assert_eq!(engine.branch_sync_interval_secs.load(Ordering::Relaxed), 30);

        let mut config = engine.config.clone();
        config.ui.branch_sync_interval = 45;
        engine
            .apply_reloaded_config(config)
            .expect("reload applies");
        assert_eq!(engine.branch_sync_interval_secs.load(Ordering::Relaxed), 45);
    }

    #[test]
    fn a_config_reload_that_turns_branch_sync_on_from_zero_starts_the_worker() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.branch_sync_interval = 0;
        engine.spawn_branch_sync_worker();
        assert!(!engine.branch_sync_worker_started.load(Ordering::Relaxed));

        let mut config = engine.config.clone();
        config.ui.branch_sync_interval = 5;
        engine
            .apply_reloaded_config(config)
            .expect("reload applies");
        assert!(
            engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "turning the poller on needs no restart",
        );
        assert_eq!(engine.branch_sync_interval_secs.load(Ordering::Relaxed), 5);
    }

    /// The running loop re-reads the shared interval on every wait slice, so a
    /// retune reaches a thread that is already napping on the old value.
    #[test]
    fn the_running_branch_sync_loop_adopts_a_retuned_interval() {
        let (mut engine, tmp) = test_engine();
        let (worker_tx, worker_rx) = std::sync::mpsc::channel();
        engine.worker_tx = worker_tx;
        // A short slice keeps the retune observable without a long wait; the
        // shipped granularity is BRANCH_SYNC_SLICE_MS.
        engine.branch_sync_wait.slice_ms.store(5, Ordering::Relaxed);

        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        crate::git::init_repo(&worktree).expect("init repo");
        let actual = crate::git::current_branch(&worktree).expect("a branch");
        engine
            .branch_sync_sessions
            .lock()
            .expect("branch sync sessions")
            .push(BranchSyncEntry {
                session_id: "session-1".to_string(),
                worktree_path: worktree.to_string_lossy().to_string(),
                branch_name: format!("{actual}-stale"),
            });

        // An hour, so nothing polls unless the loop notices the retune.
        engine.config.ui.branch_sync_interval = 3600;
        engine.spawn_branch_sync_worker();
        // Retune only once the loop is provably waiting on the hour, so a pass
        // cannot come from the thread reading the new value on its way in.
        let deadline = Instant::now() + Duration::from_secs(30);
        while engine
            .branch_sync_wait
            .waits_started
            .load(Ordering::Relaxed)
            == 0
        {
            assert!(Instant::now() < deadline, "the branch-sync loop never woke");
            thread::sleep(Duration::from_millis(1));
        }
        engine.branch_sync_interval_secs.store(1, Ordering::Relaxed);

        let event = worker_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the retuned interval produces a branch sync");
        assert!(matches!(event, WorkerEvent::BranchSyncReady(_)));
    }

    /// A reload to `0` stops the sweeps without stopping the thread: the loop
    /// naps instead, so a later retune back to N is picked up by this same loop
    /// and `branch_sync_worker_started` keeps meaning "a thread is live".
    #[test]
    fn a_reload_to_zero_stops_the_running_branch_sync_loop_from_polling() {
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
        let (mut engine, tmp) = test_engine();
        let (worker_tx, worker_rx) = std::sync::mpsc::channel();
        engine.worker_tx = worker_tx;
        engine.branch_sync_wait.slice_ms.store(5, Ordering::Relaxed);

        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        crate::git::init_repo(&worktree).expect("init repo");
        let actual = crate::git::current_branch(&worktree).expect("a branch");
        // A real session, because the reload rebuilds the enrolment list from
        // `engine.sessions`: an entry pushed by hand would be swept away and the
        // quiet below would prove nothing.
        let mut session = sample_session("session-1", "project-1", &format!("{actual}-stale"));
        if let crate::model::AgentWorkspace::Managed(managed) = &mut session.workspace {
            managed.worktree_path = worktree.to_string_lossy().to_string();
        }
        engine.sessions.push(session);
        engine.update_branch_sync_sessions();

        // One second, so the loop sweeps often enough for the quiet window below
        // to be evidence rather than coincidence.
        engine.config.ui.branch_sync_interval = 1;
        engine.spawn_branch_sync_worker();
        let first = worker_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("an enabled loop sweeps");
        assert!(matches!(first, WorkerEvent::BranchSyncReady(_)));

        let waits_before = engine
            .branch_sync_wait
            .waits_started
            .load(Ordering::Relaxed);
        let mut config = engine.config.clone();
        config.ui.branch_sync_interval = 0;
        engine
            .apply_reloaded_config(config)
            .expect("reload applies");
        assert_eq!(
            engine
                .branch_sync_sessions
                .lock()
                .expect("branch sync sessions")
                .len(),
            1,
            "the agent is still enrolled, so silence is the interval and not an empty list"
        );

        // Wait for the loop to START a new wait, which is where it re-reads the
        // interval: until then the quiet below would only mean the old wait had
        // not elapsed yet.
        let deadline = Instant::now() + Duration::from_secs(30);
        while engine
            .branch_sync_wait
            .waits_started
            .load(Ordering::Relaxed)
            <= waits_before
        {
            assert!(Instant::now() < deadline, "the loop never restarted a wait");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "the thread naps rather than ending, so a later retune can wake it"
        );
        // One sweep may have been queued before the retune landed.
        while worker_rx.try_recv().is_ok() {}

        // Three seconds of a one-second cadence: three sweeps, had it kept going.
        // Only branch syncs are counted; the engine's other pollers share this
        // channel and are not what this test is about.
        let quiet_until = Instant::now() + Duration::from_secs(3);
        while let Some(remaining) = quiet_until.checked_duration_since(Instant::now())
            && let Ok(event) = worker_rx.recv_timeout(remaining)
        {
            assert!(
                !matches!(event, WorkerEvent::BranchSyncReady(_)),
                "a napping loop sweeps nothing"
            );
        }
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

    // -- change_agent_provider ------------------------------------------------

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

    /// The single launch-outcome status mapper (shared by the web op resolver
    /// and the TUI's reconnect ops) maps each variant to its exact final. Before
    /// this, the TUI carried a byte-identical copy (`reconnect_final`); this pins
    /// the one core source so the wording cannot drift.
    #[test]
    fn launch_outcome_final_maps_each_variant_to_its_message() {
        assert_eq!(
            launch_outcome_final(&LaunchOutcome::Ready {
                status_message: "Resumed claude agent \"x\".".to_string(),
            }),
            Final::info("Resumed claude agent \"x\".".to_string()),
        );
        assert_eq!(
            launch_outcome_final(&LaunchOutcome::ReconnectFailed {
                branch_name: "feat".to_string(),
                message: "boom".to_string(),
            }),
            Final::error("Reconnect failed for agent \"feat\": boom".to_string()),
        );
        assert_eq!(
            launch_outcome_final(&LaunchOutcome::ForceReconnectFailed {
                branch_name: "feat".to_string(),
                message: "boom".to_string(),
            }),
            Final::error("Fresh restart failed for agent \"feat\": boom".to_string()),
        );
        assert_eq!(
            launch_outcome_final(&LaunchOutcome::Missing),
            Final::clear()
        );
    }

    /// Seeding populates `pr_statuses` from the SQLite `latest_prs` rows via
    /// the shared `gh::reconstruct_pr_from_stored`, so both startups (the TUI and
    /// `dux serve`) show persisted PR badges immediately, before any network poll.
    #[test]
    fn seed_pr_statuses_from_store_populates_from_stored_rows() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        // The PR row has a FK to a session; seed the session first.
        engine
            .session_store
            .upsert_session(&sample_session("s1", "p1", "feat"))
            .expect("seed session");
        engine
            .session_store
            .upsert_pr(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 42,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "OPEN".to_string(),
                title: "A PR".to_string(),
                url: "https://github.com/o/r/pull/42".to_string(),
            })
            .expect("seed a stored PR");

        engine.seed_pr_statuses_from_store();

        let info = engine.pr_statuses.get("s1").expect("seeded PR status");
        assert_eq!(info.number, 42);
        assert_eq!(info.state, crate::model::PrState::Open);
    }

    /// A pinned PR wins over the `session_prs` latest at seed time: the latest
    /// row can be a HIGHER-numbered autodetected PR, and the badge must show the
    /// pin. The pin also lands in `pr_overrides` so the identity guard and the
    /// sync planner know about it from boot.
    #[test]
    fn seed_pr_statuses_prefers_the_override_over_the_latest_stored_pr() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine
            .session_store
            .upsert_session(&sample_session("s1", "p1", "feat"))
            .expect("seed session");
        engine
            .session_store
            .upsert_pr(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 50,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "OPEN".to_string(),
                title: "Autodetected".to_string(),
                url: "https://github.com/o/r/pull/50".to_string(),
            })
            .expect("seed a stored PR");
        engine
            .session_store
            .upsert_pr_override(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 10,
                host: "github.com".to_string(),
                owner_repo: "fork/r".to_string(),
                state: "OPEN".to_string(),
                title: "Pinned".to_string(),
                url: "https://github.com/fork/r/pull/10".to_string(),
            })
            .expect("seed the override");

        engine.seed_pr_statuses_from_store();

        let info = engine.pr_statuses.get("s1").expect("seeded PR status");
        assert_eq!(info.number, 10, "the pin wins over the latest stored PR");
        assert_eq!(info.owner_repo, "fork/r");
        assert_eq!(
            engine.pr_overrides.get("s1").map(|p| p.pr_number),
            Some(10),
            "the override map is loaded alongside the badge",
        );
    }

    /// A detached session's stored `session_prs` row must not come back as a
    /// badge on the next boot. The suppression row is durable precisely so a
    /// restart is not a way to undo the user's detach by accident.
    #[test]
    fn seed_pr_statuses_skips_a_suppressed_sessions_stored_rows() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine
            .session_store
            .upsert_session(&sample_session("s1", "p1", "feat"))
            .expect("seed session");
        engine
            .session_store
            .upsert_session(&sample_session("s2", "p1", "other"))
            .expect("seed session");
        for (id, number) in [("s1", 42), ("s2", 43)] {
            engine
                .session_store
                .upsert_pr(&crate::storage::StoredPr {
                    session_id: id.to_string(),
                    pr_number: number,
                    host: "github.com".to_string(),
                    owner_repo: "o/r".to_string(),
                    state: "OPEN".to_string(),
                    title: "A PR".to_string(),
                    url: format!("https://github.com/o/r/pull/{number}"),
                })
                .expect("seed a stored PR");
        }
        engine
            .session_store
            .set_pr_suppressed("s1")
            .expect("persist the detach");

        engine.seed_pr_statuses_from_store();

        assert!(
            engine.pr_suppressions.contains("s1"),
            "seeding loads the durable suppression into the in-memory mirror"
        );
        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "a detached agent must come back from a restart with no badge"
        );
        assert_eq!(
            engine.pr_statuses.get("s2").map(|p| p.number),
            Some(43),
            "an untouched agent still seeds its badge"
        );
    }

    /// Detach is the whole feature: the badge goes now, the pin goes, and the
    /// session is recorded as suppressed both in memory and on disk.
    #[test]
    fn detach_clears_the_badge_now_and_suppresses_autodetection() {
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
            .apply_pr_attach("s1", "github.com", "o/r", 12, "Pinned", "OPEN", "")
            .expect("attach");
        assert!(engine.pr_statuses.contains_key("s1"));

        engine.clear_pull_request_override("s1").expect("detach");

        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "the badge must disappear with the detach, not one sync cycle later"
        );
        assert!(engine.pr_overrides.is_empty(), "the pin goes too");
        assert!(engine.pr_suppressions.contains("s1"));
        assert_eq!(
            engine
                .session_store
                .load_pr_suppressions()
                .expect("load suppressions"),
            vec!["s1".to_string()],
            "the detach is durable"
        );
    }

    /// Detach is no longer a no-op without a pin: an AUTODETECTED badge is
    /// exactly the case the user is complaining about, so it must clear too.
    #[test]
    fn detach_without_a_pin_still_clears_the_badge_and_suppresses() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);
        engine.pr_statuses.insert(
            "s1".to_string(),
            crate::model::PrInfo {
                number: 12,
                state: crate::model::PrState::Open,
                title: "Autodetected".to_string(),
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                url: "https://github.com/o/r/pull/12".to_string(),
            },
        );

        let message = engine.clear_pull_request_override("s1").expect("detach");

        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "an autodetected badge is detachable, got {:?}",
            engine.pr_statuses.get("s1"),
        );
        assert!(engine.pr_suppressions.contains("s1"));
        assert!(
            !message.contains("no manually attached pull request"),
            "the old honest-no-op copy is now false, got {message:?}"
        );
    }

    /// A suppressed session is left out of the sync snapshot entirely, so the
    /// poll loop never asks GitHub about it and can never re-badge it.
    #[test]
    fn a_suppressed_session_is_excluded_from_the_sync_entries() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        for id in ["s1", "s2"] {
            let session = sample_session(id, "p1", id);
            engine
                .session_store
                .upsert_session(&session)
                .expect("seed session");
            engine.sessions.push(session);
        }

        engine.clear_pull_request_override("s1").expect("detach");

        let entries = engine.pr_sync_sessions.lock().unwrap().clone();
        let ids: Vec<String> = entries.into_iter().map(|e| e.session_id).collect();
        assert_eq!(
            ids,
            vec!["s2".to_string()],
            "a detached agent must not be planned for at all"
        );
    }

    /// The one-shot paths (focus, refs change, agent exit) must respect the
    /// detach too, or focusing a detached agent would re-detect its PR.
    #[test]
    fn a_suppressed_session_gets_no_one_shot_pr_check() {
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

        engine.spawn_pr_check_for_session("s1", Duration::from_secs(0));

        assert!(
            !engine.pr_last_checked.contains_key("s1"),
            "a suppressed session must be skipped before the check is stamped"
        );
    }

    /// Plugging the PR back in by hand is the documented way out of a detach,
    /// so an attach clears the suppression in memory and on disk.
    #[test]
    fn a_manual_attach_lifts_the_suppression() {
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

        engine
            .apply_pr_attach("s1", "github.com", "o/r", 12, "Pinned", "OPEN", "")
            .expect("attach");

        assert!(engine.pr_suppressions.is_empty());
        assert!(
            engine
                .session_store
                .load_pr_suppressions()
                .expect("load suppressions")
                .is_empty()
        );
        assert_eq!(engine.pr_statuses.get("s1").map(|p| p.number), Some(12));
        let entries = engine.pr_sync_sessions.lock().unwrap().clone();
        assert_eq!(
            entries.len(),
            1,
            "the session is planned for again once it is attached"
        );
    }

    /// The way back from a detach: the suppression is lifted in memory and on
    /// disk, the session rejoins the sync plan, and one immediate check is
    /// spawned so the badge can come back now rather than at the next poll.
    #[test]
    fn resume_lifts_the_suppression_and_checks_once_immediately() {
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
        assert!(engine.pr_sync_sessions.lock().unwrap().is_empty());

        engine.resume_pr_autodetection("s1").expect("resume");

        assert!(engine.pr_suppressions.is_empty());
        assert!(
            engine
                .session_store
                .load_pr_suppressions()
                .expect("load suppressions")
                .is_empty(),
            "the resume is durable too"
        );
        let entries = engine.pr_sync_sessions.lock().unwrap().clone();
        assert_eq!(
            entries.len(),
            1,
            "the session is planned for again from this cycle on"
        );
        assert!(
            engine.pr_last_checked.contains_key("s1"),
            "one immediate check runs, so the badge does not wait for the poll"
        );
    }

    /// A detach followed by a fresh engine over the same database: the badge
    /// stays gone, and the resume brings detection back for good.
    #[test]
    fn a_detach_survives_a_restart_and_a_resume_survives_one_too() {
        // "Restart" here is the seeding layer: the in-memory mirrors are
        // dropped and rebuilt from the database exactly as a fresh boot does.
        // That the rows themselves survive a real file reopen is proven by
        // `storage::tests::pr_suppression_round_trips_and_survives_reopen`.
        fn reboot(engine: &mut Engine) {
            engine.pr_statuses.clear();
            engine.pr_overrides.clear();
            engine.pr_suppressions.clear();
            engine.seed_pr_statuses_from_store();
        }

        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("seed session");
        engine.sessions.push(session);
        engine
            .session_store
            .upsert_pr(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 12,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "OPEN".to_string(),
                title: "Detected".to_string(),
                url: "https://github.com/o/r/pull/12".to_string(),
            })
            .expect("seed a stored PR");
        engine.clear_pull_request_override("s1").expect("detach");

        reboot(&mut engine);
        assert!(engine.pr_suppressions.contains("s1"));
        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "the detach outlives the process"
        );

        engine.resume_pr_autodetection("s1").expect("resume");

        // And so does the resume.
        reboot(&mut engine);
        assert!(engine.pr_suppressions.is_empty());
        assert_eq!(
            engine.pr_statuses.get("s1").map(|p| p.number),
            Some(12),
            "the remembered association badges again once detection is back"
        );
    }

    #[test]
    fn seed_pr_statuses_from_store_is_a_noop_when_github_integration_is_off() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = false;
        engine
            .session_store
            .upsert_session(&sample_session("s1", "p1", "feat"))
            .expect("seed session");
        engine
            .session_store
            .upsert_pr(&crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 42,
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                state: "OPEN".to_string(),
                title: "A PR".to_string(),
                url: "u".to_string(),
            })
            .expect("seed a stored PR");

        engine.seed_pr_statuses_from_store();
        assert!(engine.pr_statuses.is_empty());
    }

    /// The new-tab default provider resolution is core-owned (both Rust
    /// surfaces call it): the session's project's `default_provider`, falling
    /// back to the global config default only when the project is missing.
    #[test]
    fn default_provider_for_new_tab_prefers_the_project_then_the_global_default() {
        let (mut engine, _tmp) = test_engine();
        engine.config.defaults.provider = "claude".to_string();
        let mut project = sample_project("p1", "/tmp/p1");
        project.default_provider = ProviderKind::new("codex");
        engine.projects.push(project);

        // Project present: its default_provider wins.
        assert_eq!(
            engine.default_provider_for_new_tab(Some("p1")).as_str(),
            "codex"
        );
        // Project missing: fall back to the global config default.
        assert_eq!(
            engine.default_provider_for_new_tab(Some("nope")).as_str(),
            "claude"
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
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
        engine.providers.insert(TabId::new("s1-slot"), client);

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("codex"))
            .expect("swap provider while running");

        assert!(outcome.running, "a PTY is live for this session");
        assert_eq!(outcome.previous.as_str(), "claude");
        // The persisted provider is the new one...
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        // ...but the previously-running provider is pinned so labels stay true.
        // The pin map is keyed by TAB id, so the key is the session-slot tab id
        // resolved from the record, and `running_provider_for` reads it back
        // through the same resolver: the write and the read cannot drift apart.
        let slot = engine.sessions[0].slot_tab_id().to_string();
        assert_eq!(
            engine
                .running_provider_pins
                .get(TabIdRef::new(&slot))
                .map(|p| p.as_str()),
            Some("claude")
        );
        assert_eq!(
            engine.running_provider_for(&engine.sessions[0]).as_str(),
            "claude",
            "the pane label reads the pin under the slot tab id"
        );

        // A second swap while still running must NOT overwrite the pin: the PTY
        // is still the original provider until the user relaunches.
        engine
            .change_agent_provider("s1", ProviderKind::new("gemini"))
            .expect("second swap while running");
        assert_eq!(
            engine
                .running_provider_pins
                .get(TabIdRef::new(&slot))
                .map(|p| p.as_str()),
            Some("claude"),
            "the pin records what's actually spawned, not the latest selection"
        );

        // Clean up so the PTY doesn't outlive the test.
        engine.providers.remove(TabIdRef::new("s1-slot"));
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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

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
            .insert(TabId::new(foreign_tab.id.clone()), foreign_tab);

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
    use crate::worker::{AgentLaunchFailedData, AgentLaunchKind, AgentLaunchReadyData};

    fn spawn_cat(cwd: &std::path::Path) -> PtyClient {
        PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
    }

    fn extra_tab(id: &str, session_id: &str, provider: &str) -> AgentTab {
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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

        let mk = |engine: &Engine| {
            engine.build_tab_launch_request(
                TabId::new("tab-1"),
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
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1")));
        assert!(
            mk(&engine).resume,
            "a claude tab in flight does not block a codex tab's resume"
        );

        // A SECOND codex tab already launching makes this one start fresh.
        let other = extra_tab("tab-2", "s1", "codex");
        engine
            .agent_tabs
            .insert(TabId::new(other.id.clone()), other);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-2")));
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
        let oc = extra_tab("tab-oc", "s1", "opencode");
        engine.session_store.insert_agent_tab(&oc).unwrap();
        engine.agent_tabs.insert(TabId::new(oc.id.clone()), oc);

        // The opencode tab is already up when the claude session-slot launches.
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-oc")));
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
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1")));
        let oc_req = engine.build_tab_launch_request(
            TabId::new("tab-oc"),
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
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-x")));
        let tab = extra_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
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
        let tab = extra_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-x")));

        assert!(
            engine.should_resume_session(&session),
            "the session provider is resume-eligible on its own"
        );
        assert!(
            !engine.tab_resume_decision(&session, session.slot_tab_id(), &session.provider, true),
            "a live same-provider extra tab downgrades the session-slot launch to fresh"
        );
    }

    /// The reconnect plan's ANNOUNCED resume decision must equal the resume the
    /// dispatched request actually uses, under a same-provider extra-tab
    /// collision. Before extraction the TUI announced resume via the
    /// collision-blind `should_resume_session` while the dispatch re-gated via
    /// `tab_resume_decision`, so it promised a resume that launched fresh.
    /// `reconnect_plan` derives BOTH from `tab_resume_decision`, killing the
    /// divergence for both surfaces.
    #[test]
    fn reconnect_plan_announced_resume_matches_the_dispatched_request() {
        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        session.started_providers = vec!["claude".into()];
        session.provider = ProviderKind::new("claude");
        engine.sessions.push(session.clone());
        // A live same-provider extra tab: `should_resume_session` still reports
        // eligible, but `tab_resume_decision` downgrades to fresh.
        let tab = extra_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-x")));
        assert!(
            engine.should_resume_session(&session),
            "precondition: the collision-blind check still says resume"
        );

        let plan = engine
            .reconnect_plan("s1", false, (24, 80))
            .expect("plan builds");
        match plan {
            ReconnectPlan::Launch {
                resume, request, ..
            } => {
                assert!(
                    !resume,
                    "the announced resume must respect the same-provider collision (fresh)"
                );
                assert_eq!(
                    request.resume, resume,
                    "the dispatched request's resume must equal the announced one"
                );
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_plan_refuses_an_already_connected_normal_reconnect() {
        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));

        match engine.reconnect_plan("s1", false, (24, 80)).expect("plan") {
            ReconnectPlan::AlreadyConnected { message } => {
                assert!(message.contains("already connected"), "{message}");
            }
            other => panic!("expected AlreadyConnected, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_plan_reports_a_missing_worktree() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = "/nonexistent/worktree/path".to_string();
        engine.sessions.push(session);

        match engine.reconnect_plan("s1", false, (24, 80)).expect("plan") {
            ReconnectPlan::WorktreeMissing { message } => {
                assert!(message.contains("no longer exists"), "{message}");
            }
            other => panic!("expected WorktreeMissing, got {other:?}"),
        }
    }

    #[test]
    fn change_agent_provider_resume_available_matches_tab_resume_decision_under_collision() {
        // `change_agent_provider` must derive resume_available from
        // `tab_resume_decision`, not the collision-blind `should_resume_session`:
        // with a live same-provider extra tab it must report `false`, like its
        // sibling `change_tab_provider`.
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.started_providers = vec!["claude".into()];
        session.provider = ProviderKind::new("claude");
        engine.sessions.push(session.clone());
        let tab = extra_tab("tab-x", "s1", "claude");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-x")));

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
    fn a_slot_tab_launch_is_never_its_own_resume_collision() {
        // Both `reconnect_plan` and `change_agent_provider` ask
        // `tab_resume_decision` about the agent's session-slot TAB, and that
        // decision skips the tab it was asked about when it scans for a rival
        // same-provider tab. So the argument must be the slot tab id resolved
        // from the record: hand it any other id and the slot tab's own in-flight
        // launch reads as a rival and silently downgrades the resume to fresh.
        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        session.started_providers = vec!["claude".into()];
        session.provider = ProviderKind::new("claude");
        engine.sessions.push(session);
        let slot = engine.slot_tab_id_of(SessionIdRef::new("s1")).to_owned();
        engine.mark_in_flight(InFlightKey::AgentLaunch(slot.clone()));

        match engine
            .reconnect_plan("s1", false, (24, 80))
            .expect("plan builds")
        {
            ReconnectPlan::Launch {
                resume, request, ..
            } => {
                assert!(resume, "the slot tab's own launch is not a rival tab");
                assert_eq!(request.resume, resume);
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("claude"))
            .expect("swap to the same provider");
        assert!(
            outcome.resume_available,
            "the slot tab's own launch must not veto its own resume here either"
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
        let session = sample_session("s1", "p1", "feat");
        // The slot tab is a row like any other, so the cap counts rows and the
        // fixture has to write one.
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        // Default cap is 20 tabs → the slot row plus 19 extras fills it.
        for i in 0..19 {
            let tab = extra_tab(&format!("t{i}"), "s1", "codex");
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
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
        let slot = engine.slot_tab_id_of(SessionIdRef::new("s1")).to_string();
        engine
            .change_tab_provider("s1", &slot, ProviderKind::new("codex"))
            .unwrap();
        // Delegation mutates the session's own provider (the session-slot tab).
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
    }

    #[test]
    fn change_tab_provider_support_updates_only_the_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = extra_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

        engine
            .change_tab_provider("s1", "tab-1", ProviderKind::new("codex"))
            .unwrap();
        assert_eq!(
            engine.agent_tabs[TabIdRef::new("tab-1")].provider.as_str(),
            "codex"
        );
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
        let tab = extra_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

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
        let tab = extra_tab("tab-1", "s1", "claude");
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine
            .pty_activity
            .insert("tab-1".to_string(), Instant::now());

        engine.close_tab("s1", "tab-1").unwrap();
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 0);
        assert!(!engine.agent_tabs.contains_key(TabIdRef::new("tab-1")));
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine
            .providers
            .insert(TabId::new("tab-1"), spawn_cat(worktree.path()));

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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine
            .providers
            .insert(TabId::new("tab-1"), spawn_cat(worktree.path()));
        // The session-slot tab (id == "s1") is the live sibling that survives.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));

        engine.close_tab("s1", "tab-1").unwrap();

        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Active,
            "a still-live sibling tab must keep the agent Active"
        );
    }

    /// An agent whose slot tab AND extras are real `agent_tabs` rows, which is
    /// what promotion needs: it deletes the departing row and re-points the
    /// session at a surviving one. `extras` is `(id, provider)` in strip order.
    fn agent_with_tabs(engine: &mut Engine, extras: &[(&str, &str)]) {
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        for (i, (id, provider)) in extras.iter().enumerate() {
            let mut tab = extra_tab(id, "s1", provider);
            tab.sort_order = i as i64 + 1;
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        }
    }

    #[test]
    fn close_tab_promotes_the_next_tab_when_the_slot_tab_closes() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex"), ("t3", "claude")]);

        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");

        assert_eq!(outcome.promoted.as_deref(), Some(TabIdRef::new("t2")));
        let session = &engine.sessions[0];
        assert_eq!(session.slot_tab_id().as_str(), "t2");
        assert_eq!(
            session.provider.as_str(),
            "codex",
            "the session's provider mirrors whichever tab holds the slot"
        );
        assert!(
            !engine.agent_tabs.contains_key(TabIdRef::new("t2")),
            "the promoted tab is the slot now, so it must not ALSO be an extra"
        );
        assert!(engine.agent_tabs.contains_key(TabIdRef::new("t3")));
        assert_eq!(
            engine.tab_ids_for_session("s1"),
            vec![TabId::new("t2"), TabId::new("t3")],
            "enumeration leads with the new slot"
        );
        // The cap counts rows, and one row is gone.
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 2);
        // Persisted, so a restart comes back in exactly this shape.
        let stored = engine.session_store.load_sessions().unwrap();
        assert_eq!(stored[0].slot_tab_id, "t2");
        assert_eq!(stored[0].provider.as_str(), "codex");
        let extras: Vec<String> = engine
            .session_store
            .load_extra_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(extras, vec!["t3".to_string()]);
    }

    #[test]
    fn close_tab_promotes_the_first_tab_in_strip_order_not_the_first_inserted() {
        // The successor is the NEXT PILL, so the order that decides it is the
        // strip's (`sort_order`, then `created_at`), never the map's iteration
        // order or the order the rows happened to be written in.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        for (id, order) in [("late", 9), ("early", 2), ("middle", 5)] {
            let mut tab = extra_tab(id, "s1", "codex");
            tab.sort_order = order;
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        }

        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");

        assert_eq!(outcome.promoted.as_deref(), Some(TabIdRef::new("early")));
    }

    #[test]
    fn promotion_leaves_the_successors_identity_and_runtime_untouched() {
        // Nothing is re-keyed: the promoted tab keeps its id, its PTY, its
        // attention flag and every other runtime entry it had. That is the whole
        // reason the pointer moves instead of the tab being renamed.
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        engine
            .providers
            .insert(TabId::new("t2"), spawn_cat(tmp.path()));
        engine.needs_attention.insert(TabId::new("t2"));
        engine
            .pty_activity
            .insert("t2".to_string(), std::time::Instant::now());
        engine
            .running_provider_pins
            .insert(TabId::new("t2"), ProviderKind::new("opencode"));

        engine.close_tab("s1", "s1-slot").expect("promotion");

        assert!(engine.providers.contains_key(TabIdRef::new("t2")));
        assert!(engine.tab_needs_attention("t2"));
        assert!(engine.pty_activity.contains_key("t2"));
        assert_eq!(
            engine
                .tab_running_provider(&engine.sessions[0], TabIdRef::new("t2"))
                .as_str(),
            "opencode",
            "a pin on the promoted tab still wins, as it did before the promotion"
        );
    }

    #[test]
    fn promoting_a_dormant_successor_detaches_the_agent_and_drops_the_reopen_intent() {
        // Every sibling dormant: the close took the agent's last live process, so
        // it detaches. Closing the SLOT tab is the deliberate "stop this agent"
        // gesture (the same one a clean exit of the slot tab is), so the
        // auto-reopen intent goes with it rather than resurrecting the agent at
        // the next startup sweep.
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        engine.sessions[0].status = SessionStatus::Active;
        engine.sessions[0].desired_running = true;
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));

        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");

        assert_eq!(outcome.promoted.as_deref(), Some(TabIdRef::new("t2")));
        assert!(outcome.detached);
        assert_eq!(engine.sessions[0].status, SessionStatus::Detached);
        assert!(!engine.sessions[0].desired_running);
    }

    #[test]
    fn promoting_a_live_successor_keeps_the_agent_active() {
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        engine.sessions[0].status = SessionStatus::Active;
        engine.sessions[0].desired_running = true;
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));
        engine
            .providers
            .insert(TabId::new("t2"), spawn_cat(tmp.path()));

        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");

        assert!(!outcome.detached);
        assert_eq!(engine.sessions[0].status, SessionStatus::Active);
        assert!(
            engine.sessions[0].desired_running,
            "the agent is still running, so its reopen intent stands"
        );
    }

    #[test]
    fn close_tab_refuses_the_last_remaining_tab() {
        // Unchanged behavior: an agent always has a slot, so the last tab's
        // close is the agent's detach, which is a different action.
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[]);

        let err = engine.close_tab("s1", "s1-slot").unwrap_err();

        assert_eq!(err.to_string(), crate::agent_tabs::ONLY_TAB_CLOSE_REFUSAL);
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "s1-slot");
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 1);
    }

    /// The strip labels a session's tabs in strip order with the disambiguating
    /// suffix, and prose names one of them with its first character upper-cased.
    /// Every sentence about a tab reads these, so a confirmation and the pill it
    /// is about cannot name the tab differently.
    #[test]
    fn tab_strip_labels_lead_with_the_slot_tab_and_disambiguate_repeats() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "claude"), ("t3", "codex")]);

        let labels = engine.tab_strip_labels(SessionIdRef::new("s1"));

        assert_eq!(
            labels
                .iter()
                .map(|(id, label)| (id.as_str(), label.as_str()))
                .collect::<Vec<_>>(),
            vec![("s1-slot", "claude"), ("t2", "claude 2"), ("t3", "codex")]
        );
        assert_eq!(
            engine.tab_prose_label(SessionIdRef::new("s1"), TabIdRef::new("t2")),
            Some("Claude 2".to_string())
        );
        assert_eq!(
            engine.tab_prose_label(SessionIdRef::new("s1"), TabIdRef::new("nope")),
            None
        );
    }

    /// A tab retargeted while running keeps showing the provider it is actually
    /// running, so the label follows the PIN rather than the row: the sentence
    /// names the pill the user can see.
    #[test]
    fn tab_strip_labels_follow_the_running_pin_not_the_row() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "claude")]);
        engine
            .running_provider_pins
            .insert(TabId::new("t2"), ProviderKind::new("codex"));

        assert_eq!(
            engine.tab_prose_label(SessionIdRef::new("s1"), TabIdRef::new("t2")),
            Some("Codex".to_string())
        );
    }

    #[test]
    fn a_refused_promotion_leaves_the_agent_and_the_outgoing_pty_exactly_as_they_were() {
        // Persist-first, proven from the engine side: when the transaction
        // refuses, NOTHING moves. The pointer, the provider mirror and the
        // extras map are untouched, and the outgoing tab's PTY is still running
        // rather than SIGTERMed, because the close never got past the write.
        // The successor is inserted into the in-memory map only, so storage
        // refuses it as a tab it has never heard of.
        let (mut engine, tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = extra_tab("t2", "s1", "codex");
        engine.agent_tabs.insert(TabId::new("t2"), tab);
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));

        let err = engine.close_tab("s1", "s1-slot").unwrap_err();

        assert!(err.to_string().contains("unknown tab"), "err: {err}");
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "s1-slot");
        assert_eq!(
            engine.sessions[0].provider.as_str(),
            "claude",
            "the mirror follows the pointer, and the pointer did not move"
        );
        assert!(
            engine.agent_tabs.contains_key(TabIdRef::new("t2")),
            "the successor is still an extra"
        );
        assert_eq!(
            engine.session_store.load_sessions().unwrap()[0].slot_tab_id,
            "s1-slot"
        );
        assert!(
            engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the outgoing PTY must not have been torn down by a close that failed"
        );
        assert!(
            engine.terminating_ptys.is_empty(),
            "nor SIGTERMed into the terminating set"
        );
    }

    #[test]
    fn close_tab_refuses_to_promote_while_the_agent_is_being_deleted() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        engine.closing_sessions.insert("s1".to_string());

        let err = engine.close_tab("s1", "s1-slot").unwrap_err();

        assert!(err.to_string().contains("being deleted"), "err: {err}");
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "s1-slot");
        assert!(engine.agent_tabs.contains_key(TabIdRef::new("t2")));
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 2);
    }

    #[test]
    fn a_second_promotion_moves_the_slot_again() {
        // The first promotion left a row-backed tab in the slot; closing THAT
        // one must promote again rather than trip over the deleted original.
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex"), ("t3", "opencode")]);

        engine.close_tab("s1", "s1-slot").expect("first promotion");
        let outcome = engine.close_tab("s1", "t2").expect("second promotion");

        assert_eq!(outcome.promoted.as_deref(), Some(TabIdRef::new("t3")));
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "t3");
        assert_eq!(engine.sessions[0].provider.as_str(), "opencode");
        assert!(engine.agent_tabs.is_empty());
        assert_eq!(engine.session_store.count_agent_tabs("s1").unwrap(), 1);
        assert_eq!(engine.tab_ids_for_session("s1"), vec![TabId::new("t3")]);
    }

    #[test]
    fn promotion_forgets_a_focus_memory_that_named_the_promoted_tab() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex"), ("t3", "claude")]);
        engine.set_last_focused_tab("s1", Some("t2")).unwrap();

        engine.close_tab("s1", "s1-slot").expect("promotion");

        assert_eq!(
            engine.sessions[0].last_focused_tab, None,
            "the slot tab is remembered as absence, so the memory is retired"
        );
        assert_eq!(
            engine.session_store.load_sessions().unwrap()[0].last_focused_tab,
            None
        );
    }

    #[test]
    fn a_launch_in_flight_for_the_successor_lands_under_the_slot_arm_after_promotion() {
        // The sharpest race: the successor's launch was dispatched while it was
        // still an extra tab, and the promotion lands before the ready event. The
        // handler must re-ask which tab is the slot AT ARRIVAL: judged by the
        // request's stale session snapshot it is an extra tab with no row (its
        // row is the slot row now), and the ghost-launch guard would kill the
        // freshly spawned process.
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        let stale = engine.sessions[0].clone();
        let request = engine.build_tab_launch_request(
            TabId::new("t2"),
            Some(ProviderKind::new("codex")),
            stale,
            false,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: false,
                status_message: "x".into(),
            },
        );

        engine.close_tab("s1", "s1-slot").expect("promotion");
        engine.process_agent_launch_ready(AgentLaunchReadyData {
            request,
            client: spawn_cat(tmp.path()),
        });

        assert!(
            engine.providers.contains_key(TabIdRef::new("t2")),
            "the promoted tab's PTY must survive its own launch completing"
        );
        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Active,
            "the arrival takes the slot-scoped arm, which flips the agent Active"
        );
        assert!(engine.sessions[0].desired_running);
    }

    #[test]
    fn a_launch_failure_for_the_closed_slot_tab_is_dropped_after_promotion() {
        // The mirror image: the launch that fails belongs to the tab that just
        // LEFT the slot. Re-asked at arrival it is neither the slot nor a row, so
        // it is the ghost case and must be silent.
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        let stale = engine.sessions[0].clone();
        let request = engine.build_tab_launch_request(
            TabId::new("s1-slot"),
            Some(ProviderKind::new("claude")),
            stale,
            false,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: false,
                status_message: "x".into(),
            },
        );

        engine.close_tab("s1", "s1-slot").expect("promotion");
        let (outcome, _) = engine.process_agent_launch_failed(AgentLaunchFailedData {
            request,
            message: "boom".to_string(),
        });

        assert!(matches!(outcome, AgentLaunchFailedOutcome::Silent));
        assert!(
            !engine.failed_tab_runs.contains(TabIdRef::new("s1-slot")),
            "a tab nothing can ask about again must not leave a verdict behind"
        );
    }

    #[test]
    fn a_fresh_launch_failing_for_a_promoted_tab_keeps_the_slot_row() {
        // The third arrival race: `create_tab` dispatched t2's very first launch
        // while t2 was still an extra, the user closed the slot tab before the
        // spawn answered, and t2 is the SLOT by the time the failure lands. The
        // fresh-tab row deletion must not run there: it would delete the row the
        // session's pointer now names and leave `slot_tab_id` dangling on disk.
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        let stale = engine.sessions[0].clone();
        let request = engine.build_tab_launch_request(
            TabId::new("t2"),
            Some(ProviderKind::new("codex")),
            stale,
            false,
            (24, 80),
            AgentLaunchKind::Tab {
                is_fresh: true,
                status_message: "x".into(),
            },
        );

        engine.close_tab("s1", "s1-slot").expect("promotion");
        let (outcome, _) = engine.process_agent_launch_failed(AgentLaunchFailedData {
            request,
            message: "boom".to_string(),
        });

        assert!(
            matches!(outcome, AgentLaunchFailedOutcome::Tab { ref tab_id, .. } if tab_id == "t2"),
            "the failure is real and belongs to the promoted tab"
        );
        let rows: Vec<String> = engine
            .session_store
            .load_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            rows,
            vec!["t2".to_string()],
            "the promoted slot's row must survive its own launch failing"
        );
        assert_eq!(
            engine.session_store.load_sessions().unwrap()[0].slot_tab_id,
            "t2",
            "and the pointer must still name a row that exists"
        );
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "t2");
    }

    #[test]
    fn reaping_the_closed_slot_tab_after_a_promotion_leaves_the_session_alone() {
        // The close SIGTERMs the old slot's PTY into the terminating set; its
        // reap arrives later, by which time the tab is neither the slot nor a
        // row. The reap must not mark the agent anything.
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex")]);
        engine.sessions[0].status = SessionStatus::Active;
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));
        engine
            .providers
            .insert(TabId::new("t2"), spawn_cat(tmp.path()));

        engine.close_tab("s1", "s1-slot").expect("promotion");
        assert_eq!(engine.terminating_ptys.len(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !engine.terminating_ptys.is_empty() && std::time::Instant::now() < deadline {
            engine.reap_terminating_ptys();
        }
        assert!(engine.terminating_ptys.is_empty(), "the old slot reaped");
        engine.prune_exited_ptys();

        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Active,
            "a reap of the departed tab says nothing about the agent"
        );
        assert_eq!(engine.sessions[0].slot_tab_id().as_str(), "t2");
    }

    #[test]
    fn after_promotion_the_slot_resumes_the_promoted_tabs_provider() {
        // The resume rule is untouched and asked of the tab: the slot's next
        // launch resumes codex because codex started in this worktree and no
        // other live tab owns that conversation.
        let (mut engine, tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex"), ("t3", "codex")]);
        engine.mark_session_provider_started("s1", &ProviderKind::new("codex"));

        engine.close_tab("s1", "s1-slot").expect("promotion");

        let session = engine.sessions[0].clone();
        assert!(engine.tab_resume_decision(
            &session,
            session.slot_tab_id(),
            &ProviderKind::new("codex"),
            true
        ));
        // A live sibling running codex owns that conversation, so the slot's
        // launch is fresh instead.
        engine
            .providers
            .insert(TabId::new("t3"), spawn_cat(tmp.path()));
        assert!(!engine.tab_resume_decision(
            &session,
            session.slot_tab_id(),
            &ProviderKind::new("codex"),
            true
        ));
    }

    #[test]
    fn attention_and_streaming_still_roll_up_over_every_tab_after_a_promotion() {
        let (mut engine, _tmp) = test_engine();
        agent_with_tabs(&mut engine, &[("t2", "codex"), ("t3", "claude")]);
        engine.needs_attention.insert(TabId::new("t3"));

        engine.close_tab("s1", "s1-slot").expect("promotion");

        assert!(
            engine.session_needs_attention("s1"),
            "an extra tab's flag still marks the agent"
        );
        engine.needs_attention.remove(TabIdRef::new("t3"));
        engine.needs_attention.insert(TabId::new("t2"));
        assert!(
            engine.session_needs_attention("s1"),
            "and so does the promoted tab's"
        );
    }

    /// An agent whose slot has ALREADY been promoted: a claude tab held the slot
    /// over a codex extra and a claude extra, and closing it moved the slot onto
    /// the codex tab. That leaves the shape every launch path has to get right:
    /// the slot is a ROW-BACKED tab whose id is not the session id, and the
    /// session's provider mirror says codex.
    ///
    /// `worktree` is a directory that exists, because the reconnect and
    /// auto-reopen paths both refuse a vanished one. Both providers have run
    /// here, so a resume decision turns on liveness rather than history.
    fn agent_with_a_promoted_codex_slot(engine: &mut Engine, worktree: &std::path::Path) {
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_string_lossy().to_string();
        session.provider = ProviderKind::new("claude");
        session.started_providers = vec!["claude".into(), "codex".into()];
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        for (i, (id, provider)) in [("t2", "codex"), ("t3", "claude")].iter().enumerate() {
            let mut tab = extra_tab(id, "s1", provider);
            tab.sort_order = i as i64 + 1;
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        }
        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");
        assert_eq!(
            outcome.promoted.as_deref(),
            Some(TabIdRef::new("t2")),
            "fixture precondition: the codex tab is in the slot now"
        );
    }

    #[test]
    fn reconnect_after_a_promotion_launches_the_promoted_slot_tab() {
        // The reconnect path builds its request from the session's POINTER, not
        // from the session id and not from whichever provider used to be in the
        // slot: after a promotion it must launch the promoted tab, running the
        // provider that tab was already configured for.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());

        match engine.reconnect_plan("s1", false, (24, 80)).expect("plan") {
            ReconnectPlan::Launch {
                request, resume, ..
            } => {
                assert_eq!(request.tab_id, TabId::new("t2"));
                assert_eq!(request.provider.as_str(), "codex");
                assert!(
                    request.resume,
                    "codex has run in this worktree and no live sibling owns that conversation"
                );
                assert_eq!(resume, request.resume);
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_after_a_promotion_starts_fresh_when_a_sibling_owns_codex() {
        // The other half of the resume rule, asked of the PROMOTED tab: a live
        // sibling already owns the codex conversation in this worktree, so the
        // slot's relaunch must start fresh rather than fight it for `--continue`.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        let rival = extra_tab("t4", "s1", "codex");
        engine.session_store.insert_agent_tab(&rival).unwrap();
        engine.agent_tabs.insert(TabId::new("t4"), rival);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("t4")));

        match engine.reconnect_plan("s1", false, (24, 80)).expect("plan") {
            ReconnectPlan::Launch {
                request, resume, ..
            } => {
                assert_eq!(request.tab_id, TabId::new("t2"));
                assert_eq!(request.provider.as_str(), "codex");
                assert!(!request.resume, "a live codex sibling downgrades to fresh");
                assert_eq!(resume, request.resume);
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }
    }

    #[test]
    fn a_forced_reconnect_after_a_promotion_targets_the_promoted_slot_tab() {
        // Force is the same path with resume off; it must still tear down and
        // relaunch the tab the pointer names.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        engine
            .providers
            .insert(TabId::new("t2"), spawn_cat(tmp.path()));

        match engine.reconnect_plan("s1", true, (24, 80)).expect("plan") {
            ReconnectPlan::Launch { request, .. } => {
                assert_eq!(request.tab_id, TabId::new("t2"));
                assert_eq!(request.provider.as_str(), "codex");
                assert!(!request.resume, "a forced reconnect never resumes");
                assert!(
                    !engine.providers.contains_key(TabIdRef::new("t2")),
                    "the force teardown cleared the PROMOTED tab's runtime"
                );
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }
    }

    #[test]
    fn auto_reopen_after_a_promotion_launches_the_promoted_slot_tab() {
        // The startup sweep hands each candidate session to the same builder
        // both surfaces call, so the eligibility question and the request must
        // both read the pointer: the provider consulted is the slot's.
        let (mut engine, tmp) = test_engine();
        engine.config.ui.auto_reopen_agents = true;
        engine
            .projects
            .push(crate::engine::test_support::sample_project("p1", "/tmp/p1"));
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        // Closing the slot took the agent's last live process, so the promotion
        // detached it and dropped the reopen intent. This models the next
        // startup, with the agent left running and its intent set again.
        engine.sessions[0].desired_running = true;
        engine.sessions[0].auto_reopen_enabled = true;

        let candidates = engine.auto_reopen_candidates();

        assert_eq!(
            candidates.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["s1"]
        );
        let request = engine.build_agent_launch_request(
            candidates[0].clone(),
            true,
            (24, 80),
            AgentLaunchKind::StartupAutoReopen,
        );
        assert_eq!(request.tab_id, TabId::new("t2"));
        assert_eq!(request.provider.as_str(), "codex");
        assert!(
            request.resume,
            "the reopen resumes the promoted tab's own conversation"
        );
    }

    #[test]
    fn dormant_tab_launch_request_refuses_a_row_backed_slot_tab() {
        // The dormant-tab builder is the EXTRA-tab path: it launches a tab as a
        // `Tab` kind, which never flips the agent's own state. A promoted slot
        // must not come through it, and it cannot: the slot's row is not in the
        // extras map. Both surfaces gate on the slot resolver before calling
        // this (the TUI's activate branch, the web's `launch_agent`), and this is
        // the backstop that makes a missed gate a visible no-op rather than a
        // tab-scoped launch of the agent's own tab.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());

        assert!(
            engine.dormant_tab_launch_request("t2", (24, 80)).is_none(),
            "the promoted slot is not an extra tab"
        );
        let extra = engine
            .dormant_tab_launch_request("t3", (24, 80))
            .expect("the surviving extra still launches through this path");
        assert_eq!(extra.tab_id, TabId::new("t3"));
        assert_eq!(extra.provider.as_str(), "claude");
    }

    #[test]
    fn resume_fallback_retry_after_a_promotion_relaunches_the_slot_as_the_agents_own() {
        // The retry decides slot-ness AT FIRING TIME. The candidate was seeded
        // while t2 was an extra tab; by the time it fires t2 is the slot, so the
        // relaunch must be the agent's own (a session-slot launch, whose failure
        // detaches the agent) rather than a tab-scoped one.
        //
        // The dispatch is refused on purpose (the agent is mid-deletion), because
        // the refusal is what makes the arm observable: only the session-slot arm
        // marks the agent Detached when its relaunch does not get off the ground.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        engine.sessions[0].status = SessionStatus::Active;
        engine
            .resume_fallback_candidates
            .insert(TabId::new("t2"), std::time::Instant::now());
        engine.closing_sessions.insert("s1".to_string());

        let outcome = engine.retry_resume_fallback("t2", (24, 80), "retrying".to_string());

        let ResumeFallbackOutcome::Retried { reaction } = outcome else {
            panic!("expected a Retried outcome");
        };
        match *reaction {
            EventReaction::DispatchAgentLaunchView(view) => {
                assert_eq!(view.tab_id, "t2", "the relaunch targets the promoted tab");
                assert!(!view.launched, "refused: the agent is being deleted");
            }
            _ => panic!("expected a dispatch view"),
        }
        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Detached,
            "a session-slot relaunch that never started detaches the agent"
        );
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("t2")),
            "the stale resume attempt was torn down"
        );
    }

    #[test]
    fn resume_fallback_retry_for_a_surviving_extra_stays_tab_scoped_after_a_promotion() {
        // The mirror of the test above, and the reason the arm has to be
        // re-decided rather than remembered: the surviving extra is still an
        // extra, so its refused relaunch must say nothing about the agent.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        engine.sessions[0].status = SessionStatus::Active;
        engine
            .resume_fallback_candidates
            .insert(TabId::new("t3"), std::time::Instant::now());
        engine.closing_sessions.insert("s1".to_string());

        let outcome = engine.retry_resume_fallback("t3", (24, 80), "retrying".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Active,
            "an extra tab's failed relaunch must not detach the agent"
        );
    }

    #[test]
    fn retargeting_a_row_backed_slot_moves_the_promoted_tabs_row_and_the_next_launch() {
        // Provider retarget on a promoted slot: the one write site moves the
        // PROMOTED tab's row and the session mirror together, so the next launch
        // of that tab runs the new provider.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("opencode"))
            .expect("retarget");

        assert_eq!(outcome.previous.as_str(), "codex");
        assert_eq!(engine.sessions[0].provider.as_str(), "opencode");
        let stored_tab_provider = engine
            .session_store
            .load_agent_tabs()
            .unwrap()
            .into_iter()
            .find(|t| t.id == "t2")
            .expect("the promoted tab's row")
            .provider;
        assert_eq!(
            stored_tab_provider.as_str(),
            "opencode",
            "the retarget wrote the row the pointer names, not a row named after the session"
        );
        match engine.reconnect_plan("s1", false, (24, 80)).expect("plan") {
            ReconnectPlan::Launch { request, .. } => {
                assert_eq!(request.tab_id, TabId::new("t2"));
                assert_eq!(request.provider.as_str(), "opencode");
            }
            other => panic!("expected a Launch plan, got {other:?}"),
        }
    }

    #[test]
    fn retargeting_a_live_row_backed_slot_pins_what_is_actually_running() {
        // The pin rule is unchanged by promotion: while the promoted tab's codex
        // process is alive, every label still says codex, and only the next
        // launch takes the new provider.
        let (mut engine, tmp) = test_engine();
        agent_with_a_promoted_codex_slot(&mut engine, tmp.path());
        engine
            .providers
            .insert(TabId::new("t2"), spawn_cat(tmp.path()));

        let outcome = engine
            .change_agent_provider("s1", ProviderKind::new("opencode"))
            .expect("retarget");

        assert!(outcome.running, "the promoted tab's PTY is the agent's PTY");
        assert_eq!(
            engine.running_provider_for(&engine.sessions[0]).as_str(),
            "codex",
            "the pin is keyed by the promoted tab id, so the label stays truthful"
        );
    }

    #[test]
    fn extra_tab_launch_ready_does_not_flip_session_state() {
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
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        let session = engine.sessions[0].clone();

        let request = engine.build_tab_launch_request(
            TabId::new("tab-1"),
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
        assert!(engine.providers.contains_key(TabIdRef::new("tab-1")));
        // ...but the session's own running state is untouched: not flipped to
        // Active, because only the slot tab moves it.
        assert_eq!(engine.sessions[0].status, SessionStatus::Detached);
    }

    #[test]
    fn ghost_extra_tab_launch_is_dropped_when_the_row_is_gone() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let session = engine.sessions[0].clone();
        // Note: NO agent_tabs row for "tab-1" — simulates a close that raced the
        // in-flight launch.
        let request = engine.build_tab_launch_request(
            TabId::new("tab-1"),
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
        assert!(!engine.providers.contains_key(TabIdRef::new("tab-1")));
    }

    #[test]
    fn first_live_tab_returns_none_when_every_tab_is_dormant() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = extra_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

        assert_eq!(engine.first_live_tab("s1"), None);
    }

    #[test]
    fn first_live_tab_skips_a_dormant_session_slot_for_a_live_extra_tab() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = extra_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        // Session-slot "s1" has no provider; the extra tab does.
        engine
            .providers
            .insert(TabId::new("tab-1"), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("tab-1".to_string()));
    }

    #[test]
    fn first_live_tab_prefers_session_slot_when_it_is_live() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = extra_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(tmp.path()));
        engine
            .providers
            .insert(TabId::new("tab-1"), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("s1-slot".to_string()));
    }

    #[test]
    fn first_live_tab_honors_sort_order_among_live_extras() {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let mut earlier = extra_tab("tab-2", "s1", "codex");
        earlier.sort_order = 2;
        let mut later = extra_tab("tab-1", "s1", "codex");
        later.sort_order = 1;
        engine
            .agent_tabs
            .insert(TabId::new(earlier.id.clone()), earlier);
        engine
            .agent_tabs
            .insert(TabId::new(later.id.clone()), later);
        // Both are live; the lower sort_order ("tab-1") should win, not
        // insertion/HashMap order.
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(tmp.path()));
        engine
            .providers
            .insert(TabId::new("tab-1"), spawn_cat(tmp.path()));

        assert_eq!(engine.first_live_tab("s1"), Some("tab-1".to_string()));
    }

    #[test]
    fn first_live_tab_counts_an_in_flight_launch_as_live() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let tab = extra_tab("tab-1", "s1", "codex");
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("tab-1")));

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
    fn resource_targets_include_project_terminals() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (terminal_id, _label) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("create project terminal");

        let targets = engine.resource_monitor_targets();
        assert_eq!(targets.len(), 1, "the project terminal must be sampled");
        assert_eq!(targets[0].id, terminal_id);
        assert_eq!(targets[0].kind, ResourceKind::Terminal);
    }

    /// `has_active_processes` picks the changed-files poll cadence, and both
    /// surfaces now keep it through this one method. Provider tabs and companion
    /// terminals both count: a workspace with only a terminal open is still busy.
    #[test]
    fn sync_has_active_processes_counts_tabs_and_terminals() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");

        engine.sync_has_active_processes();
        assert_eq!(engine.running_process_count(), 0);
        assert!(!engine.has_active_processes.load(Ordering::Relaxed));

        engine.companion_terminals.insert(
            "term-1".to_string(),
            CompanionTerminal {
                owner: crate::model::TerminalOwner::Standalone,
                label: "shell".to_string(),
                foreground_cmd: None,
                client: spawn_cat(worktree.path()),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        engine.sync_has_active_processes();
        assert_eq!(engine.running_process_count(), 1);
        assert!(engine.has_active_processes.load(Ordering::Relaxed));

        engine
            .providers
            .insert(TabId::new("s1"), spawn_cat(worktree.path()));
        engine.sync_has_active_processes();
        assert_eq!(engine.running_process_count(), 2);

        engine.providers.clear();
        engine.companion_terminals.clear();
        engine.sync_has_active_processes();
        assert!(
            !engine.has_active_processes.load(Ordering::Relaxed),
            "the flag must fall back once the last PTY is gone"
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        session.title = Some("fix-auth".to_string());
        engine.sessions.push(session);

        // The session-slot tab plus one extra tab.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));
        engine.agent_tabs.insert(
            TabId::new("tab-2"),
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
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));
        engine.companion_terminals.insert(
            "term-1".to_string(),
            CompanionTerminal {
                owner: crate::model::TerminalOwner::Session("s1".to_string()),
                label: "dev server".to_string(),
                foreground_cmd: Some("npm".to_string()),
                client: spawn_cat(worktree.path()),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );

        let targets = engine.resource_monitor_targets();
        assert_eq!(targets.len(), 3, "two tabs and one terminal");

        // Every target carries the id the UI joins on, not just a label.
        let slot = targets
            .iter()
            .find(|t| t.id == "s1-slot")
            .expect("the session-slot tab is a target keyed by its own tab id");
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
