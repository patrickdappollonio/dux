//! Background-worker events and their domain payloads. `WorkerEvent` is the
//! channel message a worker sends back to the owner (the TUI today, the Engine
//! in E2+); the payload types are plain data describing worker results.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::{Config, ProviderCommandConfig};
use crate::engine::StatusUpdate;
use crate::model::{AgentSession, ChangedFile, Project, ProjectBranchStatus, ProviderKind};
use crate::pty::PtyClient;
use crate::storage::StoredPr;

#[derive(Clone, Debug)]
pub struct ProjectWorktreeEntry {
    pub path: PathBuf,
    /// The row LABEL: the branch when there is one, else a "detached <sha>"
    /// stand-in. Good for display and useless for deciding anything.
    pub branch_name: String,
    /// The real branch, `None` for a detached worktree. Separate from
    /// `branch_name` because "is there a branch here to delete?" cannot be
    /// answered from a label that invents one.
    pub branch: Option<String>,
    pub is_managed_by_dux: bool,
    pub existing_session_id: Option<String>,
    pub is_external: bool,
    pub is_project_checkout: bool,
    pub is_selectable: bool,
}

impl ProjectWorktreeEntry {
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|part| part.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedPullRequest {
    pub project: Project,
    pub host: String,
    pub owner_repo: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub head_ref_name: String,
    /// Caller-supplied display name to carry through the resolution. The TUI
    /// resolves the PR first and then prompts for a name, so it passes `None`
    /// and the prompt seeds the head branch as the default. The web sends the
    /// name UPFRONT (no post-resolution prompt), so its lookup carries
    /// `Some(name)` here and the web follow-up dispatches the create directly.
    pub custom_name: Option<String>,
}

/// Why a PR lookup ran: to create a new agent from the PR, or to manually
/// attach the PR to an existing session. Carried on
/// [`WorkerEvent::PullRequestResolved`] so the completion handler routes the
/// resolved PR to the right consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrLookupPurpose {
    CreateAgent,
    /// Resolving for a manual attach; carries the target session so the
    /// engine-side handler can re-check it still exists before applying.
    Attach {
        session_id: String,
    },
}

/// A parsed PR-lookup target: the host/owner_repo the PR belongs to and its
/// number. Produced by [`crate::gh::parse_pull_request_lookup`] from a raw URL
/// or `#N`/`N` string and consumed by the `gh pr view` lookup. Shared by the
/// TUI's new-agent-from-pr prompt and the web's `CreateAgentFromPr` wire flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestLookup {
    pub host: String,
    pub owner_repo: String,
    pub number: u64,
}

#[derive(Clone, Debug)]
pub enum BranchWarningKind {
    /// We resolved `origin/HEAD` and know the default branch for certain.
    Known { default_branch: String },
    /// `origin/HEAD` unavailable; current branch is not `main` or `master`.
    Heuristic,
}

#[derive(Clone, Debug)]
pub enum NonDefaultBranchAction {
    AddProject {
        path: String,
        name: String,
        leading_branch: String,
    },
    CheckoutProjectDefault {
        project: Project,
    },
}

impl NonDefaultBranchAction {
    pub fn repo_path(&self) -> &str {
        match self {
            Self::AddProject { path, .. } => path,
            Self::CheckoutProjectDefault { project } => &project.path,
        }
    }

    pub fn allows_add_anyway(&self) -> bool {
        matches!(self, Self::AddProject { .. })
    }
}

/// Payload for the "create an empty initial commit, then register the project"
/// flow. A dedicated struct (rather than reusing [`NonDefaultBranchAction`]) so
/// the completion handler has no impossible variant to swallow, and so the
/// repo's real current `branch` is carried through to registration distinct
/// from its resolved `leading_branch`.
#[derive(Clone, Debug)]
pub struct InitialCommitAdd {
    pub path: String,
    pub name: String,
    /// The branch HEAD points at (unborn now; the commit lands on it).
    pub branch: String,
    /// The resolved leading branch to persist for the project.
    pub leading_branch: String,
    /// Set by dispatch: this add runs `git init` first (the adopt-a-folder
    /// flow), so the completion messages can say so.
    pub initialized_repo: bool,
    /// Set by the worker after a successful starter-.gitignore seed.
    pub seeded_gitignore: bool,
    /// Set by the worker when seeding failed non-fatally; surfaced as a
    /// persistent warning alongside the success final.
    pub seed_warning: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CreateAgentBranchInspection {
    pub current_branch: String,
    pub leading_branch: String,
}

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub name: String,
    pub pid: u32,
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    /// True for the one entry that IS the row's root process rather than a
    /// subprocess under it. The breakdown deliberately includes the root so it
    /// sums to the row's total (see `aggregate_tree`), which means one entry
    /// always restates the row above it; without this flag it reads as a
    /// phantom duplicate. Surfaces mark it rather than hide it: hiding it
    /// would leave the breakdown silently failing to add up.
    pub is_root: bool,
}

/// What a resource row describes. Lets a surface join a sampled row back to the
/// spine entity it came from (and pin the dux/total rows) without parsing the
/// human-readable `label`, which is ambiguous: a title containing `): ` would
/// break the parse, and two agents may share a title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// dux itself.
    Dux,
    /// One agent tab's provider process (the session-slot tab is keyed by the
    /// session id, extra tabs by their tab id).
    Agent,
    /// One companion terminal.
    Terminal,
    /// The synthetic sum row.
    Total,
}

/// A process tree the resource monitor should sample, with the identity the
/// caller already had when it picked the target.
#[derive(Clone, Debug)]
pub struct ResourceTarget {
    /// The spine id: a tab id for [`ResourceKind::Agent`], a terminal id for
    /// [`ResourceKind::Terminal`].
    pub id: String,
    pub kind: ResourceKind,
    /// Human-readable description, for display only. Never parse it.
    pub label: String,
    /// Root of the process tree to aggregate.
    pub pid: u32,
}

#[derive(Clone, Debug)]
pub struct ResourceStats {
    /// The spine id this row was sampled for, or `None` for the dux and total
    /// rows, which describe no single spine entity.
    pub id: Option<String>,
    pub kind: ResourceKind,
    pub label: String,
    pub pid: Option<u32>,
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    pub process_count: usize,
    pub children: Vec<ProcessInfo>,
}

impl ResourceStats {
    /// Whether this row's breakdown carries any information beyond the row
    /// itself, i.e. whether an expand affordance should be offered at all.
    ///
    /// The threshold is `> 1`, not `> 0`, because `children` always contains
    /// the root process itself. For a LEAF target (a provider that spawned no
    /// subprocesses, which is the common case) that single entry is the root,
    /// so expanding reveals nothing but a duplicate of the row just expanded.
    ///
    /// This lives in core, and is projected onto the wire as
    /// `ResourceStatsView::has_breakdown`, so the TUI and the web cannot drift
    /// on it. The rule is a policy call ("one entry is not a breakdown"), not
    /// a self-evident fact, and it is exactly the kind of off-by-one that a
    /// second implementation in a second language gets subtly wrong.
    pub fn has_breakdown(&self) -> bool {
        self.children.len() > 1
    }
}

#[derive(Clone, Debug)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub label: String,
    pub is_git_repo: bool,
    /// True only for the synthetic parent-directory ("../") row synthesized at
    /// the top of `browser_entries`. Real directory entries are always `false`.
    /// Consumers use this typed flag rather than matching the `"../"` label
    /// string to special-case the parent row.
    pub is_parent: bool,
}

#[derive(Clone, Debug)]
pub enum AgentLaunchKind {
    Create {
        status_message: String,
        repo_path: String,
        owns_worktree: bool,
        startup_result: Option<crate::startup::StartupCommandResult>,
        /// Opaque correlation id of the shared create-agent `HandlerStatusOp`
        /// (see `Engine::pending_create_ops`). Threads from the
        /// `DispatchCreateAgentRequest` dispatch through the worker so the
        /// launch-ready / launch-failed handlers can resolve the op's final.
        status_op_id: String,
    },
    Reconnect {
        status_message: String,
    },
    ForceReconnect {
        status_message: String,
    },
    ResumeFallback {
        status_message: String,
    },
    StartupAutoReopen,
    /// An extra-tab launch. Whether it resumes is decided dynamically, per
    /// provider, by `Engine::tab_resume_decision` — the same rule every other
    /// launch path uses — not hardcoded to "never". `is_fresh` distinguishes a
    /// brand-new `create_tab` (whose row should be deleted if this very first
    /// spawn fails) from an explicit relaunch of an already-persisted dormant
    /// extra tab (whose row is kept and whose real error is surfaced).
    Tab {
        is_fresh: bool,
        status_message: String,
    },
}

#[derive(Clone, Debug)]
pub struct AgentLaunchRequest {
    pub session: AgentSession,
    /// The tab this launch belongs to and the key under which its PTY / runtime
    /// state is tracked. Equals `session.id` for the session-slot tab; a distinct id for
    /// an extra tab. Intentionally has no `Default` so every construction site
    /// must set it explicitly (a missed site is a compile error).
    pub tab_id: String,
    /// The effective provider for this tab. Equals `session.provider` for the
    /// session-slot tab; may differ for an extra tab that was retargeted.
    pub provider: ProviderKind,
    pub provider_config: ProviderCommandConfig,
    pub env: Vec<(String, String)>,
    /// The terminal identity (env add/remove) applied at spawn so the agent sees a
    /// useful terminal name. Resolved from engine state at build time. Empty for
    /// `terminal_identity = "none"` (the pre-capabilities behavior).
    pub identity: crate::term_identity::TerminalIdentity,
    pub resume: bool,
    pub pty_size: (u16, u16),
    pub scrollback_lines: usize,
    pub kind: AgentLaunchKind,
    /// TUI-only landing hint (decision 10): `true` when the launch was
    /// initiated by a fullscreen-seeking gesture (the fullscreen toggle on a
    /// dormant tab, or a relaunch started from the fullscreen relaunch
    /// screen), so its completion should land fullscreen. Every other launch
    /// lands focused-but-minimized. Core builders always construct this as
    /// `false`; the TUI flips it on the built request before dispatch. The
    /// web has no fullscreen concept and never reads it, so web-originated
    /// launches keep the `false` default and behave exactly as before.
    pub wants_fullscreen: bool,
}

pub struct AgentLaunchReadyData {
    pub request: AgentLaunchRequest,
    pub client: PtyClient,
}

#[derive(Clone, Debug)]
pub struct AgentLaunchFailedData {
    pub request: AgentLaunchRequest,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum CreateAgentRequest {
    NewProject {
        project: Project,
        custom_name: Option<String>,
        use_existing_branch: bool,
        pull_before_create: bool,
        copy_uncommitted_changes: bool,
    },
    PullRequest {
        project: Project,
        host: String,
        owner_repo: String,
        number: u64,
        title: String,
        state: String,
        head_branch: String,
        custom_name: Option<String>,
        use_existing_branch: bool,
    },
    ForkSession {
        project: Project,
        source_session: Box<AgentSession>,
        source_label: String,
        custom_name: Option<String>,
    },
    ExistingManagedWorktree {
        project: Project,
        worktree_path: PathBuf,
        branch_name: String,
        custom_name: Option<String>,
    },
    ForkExternalWorktree {
        project: Project,
        source_worktree_path: PathBuf,
        source_label: String,
        source_branch: String,
        custom_name: Option<String>,
    },
}

impl CreateAgentRequest {
    /// The project id this request belongs to. Used to key the create-agent
    /// busy→final status pair so the web can dismiss the spinner on completion.
    pub fn project_id(&self) -> &str {
        match self {
            Self::NewProject { project, .. } => &project.id,
            Self::PullRequest { project, .. } => &project.id,
            Self::ForkSession { project, .. } => &project.id,
            Self::ExistingManagedWorktree { project, .. } => &project.id,
            Self::ForkExternalWorktree { project, .. } => &project.id,
        }
    }
}

pub enum WorkerEvent {
    /// Status update delivered via the worker channel so it stays FIFO with
    /// the completion event of the operation it announces. Posted by
    /// `Engine::spawn_command_worker` before the worker thread starts so the
    /// busy status is guaranteed to reach `process_worker_event` ahead of
    /// any event the worker can produce.
    CommandWorkerStarted(StatusUpdate),
    /// A progress update emitted while an agent is being created/forked/attached.
    /// `status_op_id` is the opaque id of the shared create-agent `HandlerStatusOp`
    /// (see `Engine::pending_create_ops`); the handler looks the op up and re-emits
    /// `op.progress(message)` on the same id, so the dispatch's busy and every
    /// progress update render as one in-place toast that the final state dismisses.
    CreateAgentProgress {
        status_op_id: String,
        message: String,
    },
    CreateAgentFailed {
        status_op_id: String,
        message: String,
    },
    AgentLaunchReady(Box<AgentLaunchReadyData>),
    AgentLaunchFailed(Box<AgentLaunchFailedData>),
    ChangedFilesReady {
        /// The computed (staged, unstaged) lists, or the message git failed
        /// with. The error is carried rather than flattened into empty lists:
        /// "git could not answer" and "the worktree is clean" are different
        /// facts, and reporting the first as the second tells the user nothing
        /// has changed when dux has no idea what is in the tree.
        outcome: Result<(Vec<ChangedFile>, Vec<ChangedFile>), String>,
        /// The worktree these lists were computed for. The poller snapshots the
        /// watched worktree, releases the lock, then runs `git::changed_files`
        /// off-thread; by the time this event lands the watch may have moved to
        /// a different session. Tagging the event lets the engine drop a stale
        /// poll instead of overwriting the current session's files with a
        /// different worktree's contents.
        worktree: PathBuf,
    },
    /// A `spawn_status_op` worker finished and carried back its resolved final
    /// (the success/failure message or a clear, already keyed).
    StatusOpCompleted {
        resolved: crate::engine::ResolvedFinal,
    },
    PullCompleted {
        repo_path: String,
        target: PullTarget,
        result: Result<PullOutcome, String>,
        /// The status final resolved by the dispatch-site StatusOp (the success
        /// or failure message, already keyed). The handler still uses `result`
        /// for its domain mutations and emits this for the user-facing status.
        status: crate::engine::ResolvedFinal,
    },
    BrowserEntriesReady {
        dir: PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ProjectWorktreesReady {
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in
        /// the completion handler (the final depends on whether the picker is
        /// still open, which the worker can't see). `None` for callers that
        /// don't drive a handler-resolved status.
        status_op_id: Option<String>,
    },
    /// The worktree MANAGER's listing (managed worktrees plus dirtiness),
    /// distinct from `ProjectWorktreesReady`, which feeds the adopt picker and
    /// carries the whole classification without dirtiness.
    ManageableWorktreesReady {
        project_id: String,
        result: Result<Vec<crate::worktree_manager::ManagedWorktree>, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in
        /// the completion handler (it depends on whether the manager is still
        /// open, which the worker can't see).
        status_op_id: Option<String>,
    },
    ClipboardCopyCompleted {
        /// Human-readable success message shown in the status bar.
        label: String,
        result: Result<(), String>,
        /// Status final resolved at the call site by the clipboard StatusOp:
        /// the success (`label`) or failure message, already keyed. The handler
        /// emits this for the user-facing status.
        status: crate::engine::ResolvedFinal,
    },
    BranchSyncReady(Vec<(String, String)>),
    BranchRenameCompleted {
        session_id: String,
        new_branch: String,
        previous_title: Option<String>,
        result: Result<(), String>,
        /// Status final resolved at dispatch by the rename StatusOp; the handler
        /// runs its domain revert/persist and emits this for the user message.
        status: crate::engine::ResolvedFinal,
    },
    /// Rows plus whether this sample had to re-establish its CPU baseline
    /// (see [`crate::resource_stats::ResourceCollector::sample`]): a real
    /// reading measured over the short baseline window rather than the
    /// caller's normal poll interval.
    ResourceStatsReady(Vec<ResourceStats>, bool),
    /// One run of the `gh` host probe finished.
    ///
    /// `generation` is stamped BEFORE the probe is spawned and travels on every
    /// way it can finish, including the result synthesised when the worker
    /// panics. The handler discards a stale generation FIRST, before the status
    /// changes, before the host policy changes, before it is logged, and before
    /// it can start the pull-request workers, so two probes launched close
    /// together cannot let the older answer win.
    GhStatusChecked {
        generation: u64,
        outcome: crate::gh::GhProbe,
    },
    PrStatusReady(Vec<(String, Option<crate::model::PrInfo>)>),
    /// A one-shot PR check worker panicked; carries the session id so its
    /// `InFlightKey::PrCheck` guard is cleared without wiping the PR badge.
    PrCheckAborted(String),
    /// A typed pull request reference has been matched against every project's
    /// configured address. One `git` call per project ran on the worker; the
    /// answer is not cached anywhere, because nothing dux watches would tell it
    /// when the answer changed (see
    /// [`crate::pr_reference::resolve_reference_projects`]).
    ///
    /// The match list has three interesting shapes and the surface branches on
    /// all three: exactly one project proceeds to the lookup, several mean the
    /// repository is checked out twice and the user picks, and none means dux
    /// found no project for `repository` and can only offer the project picker,
    /// because dux does not clone.
    ///
    /// `result` is a `Result` because a worker that fell over is not the same
    /// answer as an empty match list. Reporting a panic as "no project is a
    /// checkout of that repository" states, in dux's own voice, something dux
    /// never found out.
    PullRequestReferenceResolved {
        /// The text the user typed, carried through so the chosen project can
        /// be handed straight to the existing lookup.
        raw_input: String,
        /// How to name the repository in a message, from
        /// [`crate::pr_reference::TypedReference::repository_label`].
        repository: String,
        /// The matches AND what could not be inspected, or why the attempt
        /// failed outright.
        result: Result<crate::pr_reference::ReferenceResolution, String>,
        status_op_id: Option<String>,
    },
    PullRequestResolved {
        result: Result<ResolvedPullRequest, String>,
        /// Why the lookup ran. A create-flow resolution opens the name prompt
        /// (TUI) or hands off to the create dispatch (web); an attach-flow
        /// resolution is applied ENGINE-SIDE (`apply_pr_attach`) after
        /// re-checking the session still exists, under the same keyed op the
        /// dispatch opened.
        purpose: PrLookupPurpose,
        /// Correlation id for a web `HandlerStatusOp` whose final is resolved in
        /// the completion handler. Rides from the `apply_wire` dispatch through
        /// the lookup worker so the lookup FAILURE (here) and the lookup SUCCESS
        /// handoff (in `drive_pr_lookup_followup`) resolve the right op. `None`
        /// for the TUI, which keeps its prompt-after-resolution flow.
        status_op_id: Option<String>,
    },
    RefsChanged(String),
    /// Background `git worktree remove` for a session-initiated delete has
    /// finished. On `Ok`, the result says what happened to each branch the
    /// removal targeted (used for the status message). On `Err`, the message is
    /// the formatted error; the session record must be preserved so the user
    /// can retry.
    WorktreeRemoveCompleted {
        session_id: String,
        result: Result<crate::engine::RemovedBranches, String>,
    },
    /// Background `git switch <target_branch>` run from a non-default branch
    /// warning modal has finished. On `Ok`, the main loop continues the
    /// original action. On `Err`, the formatted git error is surfaced.
    NonDefaultBranchCheckoutCompleted {
        action: NonDefaultBranchAction,
        target_branch: String,
        result: Result<(), String>,
        /// Correlation id for a web `HandlerStatusOp`. For a `CheckoutProjectDefault`
        /// action it resolves the checkout op in `process_worker_event` (both
        /// outcomes); for an `AddProject` action it resolves the add-project op's
        /// switch FAILURE here (the SUCCESS resolves later in the followup).
        /// `None` for the TUI, which keeps its unkeyed `Status` finals.
        status_op_id: Option<String>,
    },
    /// Background `create_initial_commit` for a fresh (unborn) repo has
    /// finished. On `Ok`, the followup registers the project on its now-born
    /// branch. On `Err`, the formatted git error is surfaced.
    InitialCommitCreated {
        add: InitialCommitAdd,
        result: Result<(), String>,
        /// Correlation id for a web add-project `HandlerStatusOp` (resolved in
        /// `drive_add_project_followup`), or a TUI `pending_checkout_inspect_ops`
        /// op (dismissed in `drain_events`). `None` when no keyed status is driven.
        status_op_id: Option<String>,
    },
    /// Background inspection of the selected project checkout before opening
    /// the New Agent prompt.
    CreateAgentBranchInspected {
        project: Project,
        result: Result<CreateAgentBranchInspection, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose keyed busy is dismissed
        /// when the inspection completes (the visible final comes from the
        /// downstream `ContinueCreateAgentAfterInspection` prompt's `set_info` on
        /// success, or the engine's error `Status` on failure). `None` for callers
        /// that don't drive a handler-resolved status. This flow is TUI-only.
        status_op_id: Option<String>,
    },
    ProjectBranchStatusReady {
        project_id: String,
        result: Result<(String, ProjectBranchStatus), String>,
    },
    CheckoutProjectDefaultBranchInspected {
        project: Project,
        result: Result<(String, Option<BranchWarningKind>), String>,
        /// Correlation id for a web checkout `HandlerStatusOp`, carried through to
        /// worker 2 (`run_add_project_checkout_job`) so the eventual
        /// `NonDefaultBranchCheckoutCompleted` resolves the right op. `None` for
        /// the TUI.
        status_op_id: Option<String>,
    },
    ConfigReloadReady(Box<Result<Config, String>>),
    ProjectPersistenceCompleted {
        action: ProjectPersistenceAction,
        result: Result<(), String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in
        /// the completion handler (after the fallible post-worker config write).
        /// `None` for callers that don't drive a handler-resolved status (web).
        status_op_id: Option<String>,
    },
    /// A scope's startup-command runs finished listing off-thread. Carries the
    /// whole listing (newest first) plus the newest run's contents, because the
    /// picker shows both.
    StartupCommandLogsLoaded {
        scope_label: String,
        result: Result<crate::startup::StartupCommandLogListing, String>,
    },
    /// One already-listed run finished reading off-thread, because the user
    /// moved the picker's selection onto it. `path` is the correlation handle:
    /// a reply for a run that is no longer selected is dropped, so a fast walk
    /// down the list cannot land a stale body under a newer selection.
    StartupCommandLogContentLoaded {
        path: std::path::PathBuf,
        result: Result<String, String>,
    },
    /// The in-process web-server flip pre-flight finished on a worker thread.
    /// LOCAL MODE resolution (loopback:port + optional Tailscale:port) plus the
    /// actual `TcpListener::bind` of each address runs off the UI thread because
    /// it shells out to `tailscale ip`. On success the bound listeners and their
    /// display URLs are carried back so the main loop can stash the flip; on
    /// failure the formatted error is surfaced and the TUI stays up. `warning` is
    /// a non-fatal note to show (e.g. Tailscale enabled but not detected).
    ServerFlipPreflightReady {
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        warning: Option<String>,
    },
}

/// How a successful (non-erroring) pull worker run ended. `current_branch` is
/// the checkout's branch after the run, when the worker re-read it.
#[derive(Clone, Debug)]
pub enum PullOutcome {
    /// The branch was pulled from origin.
    Pulled { current_branch: Option<String> },
    /// The repo has no `origin` remote: nothing to pull, and that is a normal
    /// state for local-only repos, not a failure.
    NoOrigin { current_branch: Option<String> },
}

impl PullOutcome {
    pub fn current_branch(&self) -> Option<&String> {
        match self {
            PullOutcome::Pulled { current_branch } | PullOutcome::NoOrigin { current_branch } => {
                current_branch.as_ref()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum PullTarget {
    Project {
        project_id: String,
        project_name: String,
        leading_branch: Option<String>,
    },
    Session,
}

/// Snapshot of session data shared with the branch-sync background worker.
#[derive(Clone, Debug)]
pub struct BranchSyncEntry {
    pub session_id: String,
    pub worktree_path: String,
    pub branch_name: String,
}

/// The identity of a manually attached ("pinned") pull request, carried on a
/// [`PrSyncEntry`] so the sync planner queries the PINNED repo (which may be a
/// fork, or any repo other than the session's remote) instead of deriving a
/// target from the worktree's remote. Identity only; the cached state/title
/// ride in `known_pr`, which both construction sites set to the override row
/// for a pinned session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedPr {
    pub host: String,
    pub owner_repo: String,
    pub number: u64,
}

/// Snapshot of session data shared with the PR-sync background worker.
#[derive(Clone, Debug)]
pub struct PrSyncEntry {
    pub session_id: String,
    pub branch_name: String,
    pub worktree_path: String,
    /// If we already know a PR for this session, the worker can use `gh pr view`
    /// (works even after branch deletion) and skip terminal states (merged/closed).
    /// For a pinned session this is the OVERRIDE row, never the `session_prs`
    /// latest (which can be a different, autodetected PR).
    pub known_pr: Option<StoredPr>,
    /// Whether the agent process has exited. Used to skip PR discovery calls
    /// for sessions that are both exited and in a terminal PR state — nobody
    /// is pushing to that branch anymore.
    pub agent_exited: bool,
    /// A manually attached PR. When set, the planner short-circuits the
    /// remote-derived target: the query goes to the pinned `(host, owner_repo)`,
    /// the host policy gates the PINNED host, and the only alias emitted is the
    /// by-number one for the pinned PR (no head-ref discovery).
    pub pinned: Option<PinnedPr>,
}

#[derive(Clone, Debug)]
pub enum ProjectPersistenceAction {
    Add {
        project: Project,
        status_message: String,
    },
    Remove {
        project_id: String,
        project_name: String,
    },
    Delete {
        project_id: String,
        project_name: String,
    },
    UpdateDefaultProvider {
        project_id: String,
        project_name: String,
        provider: Option<ProviderKind>,
        global_default: ProviderKind,
    },
    UpdateAutoReopen {
        project_id: String,
        project_name: String,
        auto_reopen_agents: Option<bool>,
    },
    UpdateStartupCommand {
        project_id: String,
        project_name: String,
        startup_command: Option<String>,
    },
    UpdateEnv {
        project_id: String,
        project_name: String,
        env: BTreeMap<String, String>,
    },
}
