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
    pub branch_name: String,
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
}

#[derive(Clone, Debug)]
pub struct ResourceStats {
    pub label: String,
    pub pid: Option<u32>,
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    pub process_count: usize,
    pub children: Vec<ProcessInfo>,
}

#[derive(Clone, Debug)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub label: String,
    pub is_git_repo: bool,
}

#[derive(Clone, Debug)]
pub enum AgentLaunchKind {
    Create {
        status_message: String,
        repo_path: String,
        owns_worktree: bool,
        startup_result: Option<crate::startup::StartupCommandResult>,
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
}

#[derive(Clone, Debug)]
pub struct AgentLaunchRequest {
    pub session: AgentSession,
    pub provider_config: ProviderCommandConfig,
    pub env: Vec<(String, String)>,
    pub resume: bool,
    pub pty_size: (u16, u16),
    pub scrollback_lines: usize,
    pub kind: AgentLaunchKind,
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

pub enum WorkerEvent {
    /// Status update delivered via the worker channel so it stays FIFO with
    /// the completion event of the operation it announces. Posted by
    /// `Engine::spawn_command_worker` before the worker thread starts so the
    /// busy status is guaranteed to reach `process_worker_event` ahead of
    /// any event the worker can produce.
    CommandWorkerStarted(StatusUpdate),
    CreateAgentProgress(String),
    CreateAgentFailed(String),
    AgentLaunchReady(Box<AgentLaunchReadyData>),
    AgentLaunchFailed(Box<AgentLaunchFailedData>),
    ChangedFilesReady {
        staged: Vec<ChangedFile>,
        unstaged: Vec<ChangedFile>,
        /// The worktree these lists were computed for. The poller snapshots the
        /// watched worktree, releases the lock, then runs `git::changed_files`
        /// off-thread; by the time this event lands the watch may have moved to
        /// a different session. Tagging the event lets the engine drop a stale
        /// poll instead of overwriting the current session's files with a
        /// different worktree's contents.
        worktree: PathBuf,
    },
    /// A one-shot commit-message run succeeded. Carries the session it was
    /// generated for so the result lands in the matching commit dialog and never
    /// leaks into another session's draft (two web dialogs / rapid switches).
    CommitMessageGenerated {
        session_id: String,
        message: String,
    },
    /// A one-shot commit-message run failed. Carries the session so a failure can
    /// be scoped to the dialog that requested it.
    CommitMessageFailed {
        session_id: String,
        error: String,
    },
    PushCompleted(Result<(), String>),
    PullCompleted {
        repo_path: String,
        target: PullTarget,
        result: Result<Option<String>, String>,
    },
    BrowserEntriesReady {
        dir: PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ProjectWorktreesReady {
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
    },
    ClipboardCopyCompleted {
        /// Human-readable success message shown in the status bar.
        label: String,
        result: Result<(), String>,
    },
    BranchSyncReady(Vec<(String, String)>),
    BranchRenameCompleted {
        session_id: String,
        new_branch: String,
        previous_title: Option<String>,
        result: Result<(), String>,
    },
    ResourceStatsReady(Vec<ResourceStats>),
    GhStatusChecked(crate::model::GhStatus),
    PrStatusReady(Vec<(String, Option<crate::model::PrInfo>)>),
    PullRequestResolved {
        result: Result<ResolvedPullRequest, String>,
    },
    RefsChanged(String),
    /// Background `git worktree remove` for a session-initiated delete has
    /// finished. On `Ok`, the boolean indicates whether the branch was
    /// already gone (used for the status message). On `Err`, the message is
    /// the formatted error; the session record must be preserved so the user
    /// can retry.
    WorktreeRemoveCompleted {
        session_id: String,
        result: Result<bool, String>,
    },
    /// Background `git switch <target_branch>` run from a non-default branch
    /// warning modal has finished. On `Ok`, the main loop continues the
    /// original action. On `Err`, the formatted git error is surfaced.
    NonDefaultBranchCheckoutCompleted {
        action: NonDefaultBranchAction,
        target_branch: String,
        result: Result<(), String>,
    },
    /// Background inspection of the selected project checkout before opening
    /// the New Agent prompt.
    CreateAgentBranchInspected {
        project: Project,
        result: Result<CreateAgentBranchInspection, String>,
    },
    ProjectBranchStatusReady {
        project_id: String,
        result: Result<(String, ProjectBranchStatus), String>,
    },
    CheckoutProjectDefaultBranchInspected {
        project: Project,
        result: Result<(String, Option<BranchWarningKind>), String>,
    },
    ConfigReloadReady(Box<Result<Config, String>>),
    ProjectPersistenceCompleted {
        action: ProjectPersistenceAction,
        result: Result<(), String>,
    },
    /// A web UI `[auth]` user add/update/remove was hashed (for adds) and
    /// persisted to config.toml on a background thread. On success the worker
    /// carries the updated user list so the engine can refresh its in-memory
    /// config; `message` is the verbose, already-formatted status line to show.
    /// On failure `result` is the formatted error and the config is left
    /// untouched. `warn` requests a warning-tone status on success (set when a
    /// removal leaves zero users, which disables the login gate).
    AuthUsersPersisted {
        users: Vec<String>,
        message: String,
        warn: bool,
        result: Result<(), String>,
    },
    StartupCommandRerunCompleted(crate::startup::StartupCommandResult),
    StartupCommandLogsLoaded {
        scope_label: String,
        result: Result<crate::startup::StartupCommandLatestLog, String>,
    },
    OpenPathCompleted {
        target: String,
        result: Result<(), String>,
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

/// Snapshot of session data shared with the PR-sync background worker.
#[derive(Clone, Debug)]
pub struct PrSyncEntry {
    pub session_id: String,
    pub branch_name: String,
    pub worktree_path: String,
    /// If we already know a PR for this session, the worker can use `gh pr view`
    /// (works even after branch deletion) and skip terminal states (merged/closed).
    pub known_pr: Option<StoredPr>,
    /// Whether the agent process has exited. Used to skip PR discovery calls
    /// for sessions that are both exited and in a terminal PR state — nobody
    /// is pushing to that branch anymore.
    pub agent_exited: bool,
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
