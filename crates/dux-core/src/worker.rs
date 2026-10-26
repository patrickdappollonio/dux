//! Background-worker events and their domain payloads. `WorkerEvent` is the
//! channel message a worker sends back to the owner (the TUI today, the Engine
//! in E2+); the payload types are plain data describing worker results.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::{Config, ProviderCommandConfig};
use crate::engine::StatusUpdate;
use crate::ids::TabId;
use crate::model::{AgentSession, ChangedFile, Project, ProjectBranchStatus, ProviderKind};
use crate::pty::PtyClient;
use crate::storage::StoredPr;

/// One worktree of a project, as the adopt picker lists it.
#[derive(Clone, Debug)]
pub struct ProjectWorktreeEntry {
    /// Where the worktree is on disk.
    pub path: PathBuf,
    /// The row LABEL: the branch when there is one, else a "detached <sha>"
    /// stand-in. Good for display and useless for deciding anything.
    pub branch_name: String,
    /// The real branch, `None` for a detached worktree. Separate from
    /// `branch_name` because "is there a branch here to delete?" cannot be
    /// answered from a label that invents one.
    pub branch: Option<String>,
    /// Whether the worktree lives in dux's managed folder for its project.
    pub is_managed_by_dux: bool,
    /// The agent already working in this worktree, if any.
    pub existing_session_id: Option<String>,
    /// Whether this is the project's own checkout rather than an extra
    /// worktree.
    pub is_project_checkout: bool,
}

impl ProjectWorktreeEntry {
    /// Whether the worktree lives outside dux's managed folder for its project.
    pub fn is_external(&self) -> bool {
        !self.is_managed_by_dux
    }

    /// Whether an agent can be started on it: no agent holds it already, and
    /// it is not the project's own checkout.
    pub fn is_selectable(&self) -> bool {
        self.existing_session_id.is_none() && !self.is_project_checkout
    }

    /// The worktree's folder name, or its whole path when it has none.
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|part| part.to_str())
            .map_or_else(|| self.path.display().to_string(), str::to_string)
    }
}

/// A pull request `gh` resolved, with the project it belongs to.
#[derive(Clone, Debug)]
pub struct ResolvedPullRequest {
    /// The project whose repository the pull request is in.
    pub project: Project,
    /// The host the pull request lives on, such as `github.com`.
    pub host: String,
    /// The repository as `owner/repo`.
    pub owner_repo: String,
    /// The pull request's number.
    pub number: u64,
    /// The pull request's title.
    pub title: String,
    /// The pull request's state as `gh` reports it (`OPEN`, `MERGED`,
    /// `CLOSED`).
    pub state: String,
    /// The pull request's head branch.
    pub head_ref_name: String,
    /// Caller-supplied display name to carry through the resolution. The TUI
    /// resolves the PR first and then prompts for a name, so it passes `None`
    /// and the prompt seeds the head branch as the default. The web sends the
    /// name UPFRONT (no post-resolution prompt), so its lookup carries
    /// `Some(name)` here and the web follow-up dispatches the create directly.
    pub custom_name: Option<String>,
    /// Whether the user agreed to a review copy, carried to the create the
    /// lookup hands off to. The TUI asks after resolving, so it passes the
    /// default; the web and the command line decide before the lookup.
    pub review_copy: ReviewCopyConsent,
    /// The id the asking client gave this create, echoed on the question a
    /// refusal puts so the client can tell it is the answer to that create.
    pub ask_id: Option<String>,
}

/// Whether the user agreed that an agent from a pull request may be made on a
/// review copy of it. A copy is a branch of dux's own, so a push from the agent
/// lands there and not on the pull request, and dux makes one only when asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReviewCopyConsent {
    /// Nobody agreed: a create that needs a copy is refused and asks.
    #[default]
    Withheld,
    /// Nobody agreed, and the request came from the command line, which agrees
    /// with a flag instead of a box: the refusal names the flag.
    WithheldByFlag,
    /// Agreed without having been shown a warning, as the command line's flag
    /// does. Covers whatever copy turns out to be needed.
    Blanket,
    /// Agreed after being shown the warning identified by this text, which
    /// covers a copy for that reason under that name and nothing else.
    Warned(String),
    /// Agreed before the copy's name could be known (before the project
    /// existed), after being shown a warning of this reason, the word a
    /// warning's identifier begins with. Covers a copy for that reason under
    /// whatever name turns out free, and nothing for another reason.
    Reason(Box<ShownReason>),
}

/// The warning an agreement given before the project existed was given for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownReason {
    /// The word the warning's reason began with.
    pub kind: String,
    /// The branch the copy would be made on, when the warning named one; an
    /// agreement that names none covers the reason under any name.
    pub branch: Option<String>,
}

/// What a surface sent about a review copy, before the warning it was shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReviewCopyAnswer {
    /// Whether the box was ticked or the flag given.
    pub agreed: bool,
    /// Whether the answer came from the command line, which agrees with a
    /// flag instead of a box.
    pub by_flag: bool,
}

impl ReviewCopyConsent {
    /// Read the fields every surface sends: the answer, and the identifier of
    /// the warning the user was shown.
    pub(crate) fn from_request(answer: ReviewCopyAnswer, warned: Option<String>) -> Self {
        match (answer.agreed, warned) {
            (false, _) if answer.by_flag => Self::WithheldByFlag,
            (false, _) => Self::Withheld,
            (true, None) => Self::Blanket,
            (true, Some(warned)) => Self::Warned(warned),
        }
    }

    /// [`Self::from_request`] for an agreement given before the project
    /// existed, which names the reason it was shown rather than a full warning.
    pub(crate) fn from_upfront(
        answer: ReviewCopyAnswer,
        reason: Option<String>,
        branch: Option<String>,
    ) -> Self {
        match (answer.agreed, reason) {
            (true, Some(kind)) => Self::Reason(Box::new(ShownReason { kind, branch })),
            _ => Self::from_request(answer, None),
        }
    }

    /// Whether this agreement covers the copy described by `warned`.
    pub fn covers(&self, warned: &str) -> bool {
        match self {
            Self::Withheld | Self::WithheldByFlag => false,
            Self::Blanket => true,
            Self::Warned(seen) => seen == warned,
            Self::Reason(shown) => {
                // `reason:#number:copy:detail`
                let mut parts = warned.split(':');
                parts.next() == Some(shown.kind.as_str())
                    && shown
                        .branch
                        .as_deref()
                        .is_none_or(|branch| parts.nth(1) == Some(branch))
            }
        }
    }

    /// Whether this agreement covers the copy on `copy_branch` for `kind`
    /// that a warning shown now would describe.
    pub(crate) fn covers_shown(&self, kind: &str, copy_branch: &str) -> bool {
        match self {
            Self::Withheld | Self::WithheldByFlag | Self::Warned(_) => false,
            Self::Blanket => true,
            Self::Reason(shown) => {
                shown.kind == kind && shown.branch.as_deref().is_none_or(|b| b == copy_branch)
            }
        }
    }
}

/// Why a PR lookup ran: to create a new agent from the PR, or to manually
/// attach the PR to an existing session. Carried on
/// [`WorkerEvent::PullRequestResolved`] so the completion handler routes the
/// resolved PR to the right consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrLookupPurpose {
    /// Resolving to create a new agent from the pull request.
    CreateAgent,
    /// Resolving for a manual attach; carries the target session so the
    /// engine-side handler can re-check it still exists before applying.
    Attach {
        /// The agent the pull request is attached to.
        session_id: String,
    },
}

/// A parsed PR-lookup target: the `host/owner_repo` the PR belongs to and its
/// number. Produced by [`crate::gh::parse_pull_request_lookup`] from a raw URL
/// or `#N`/`N` string and consumed by the `gh pr view` lookup. Shared by the
/// TUI's new-agent-from-pr prompt and the web's `CreateAgentFromPr` wire flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullRequestLookup {
    pub host: String,
    pub owner_repo: String,
    pub number: u64,
}

/// How sure dux is that a checkout is not on its repository's default branch.
#[derive(Clone, Debug)]
pub enum BranchWarningKind {
    /// We resolved `origin/HEAD` and know the default branch for certain.
    Known {
        /// The repository's default branch, as `origin/HEAD` names it.
        default_branch: String,
    },
    /// `origin/HEAD` unavailable; current branch is not `main` or `master`.
    Heuristic,
}

/// What to do once a checkout found on a non-default branch has been switched.
#[derive(Clone, Debug)]
pub enum NonDefaultBranchAction {
    /// Register the folder as a project.
    AddProject {
        /// The folder to register.
        path: String,
        /// The project's display name.
        name: String,
        /// The branch the project leads with.
        leading_branch: String,
    },
    /// Switch an existing project's checkout to its default branch.
    CheckoutProjectDefault {
        /// The project whose checkout is switched.
        project: Project,
    },
}

impl NonDefaultBranchAction {
    /// The folder the switch runs in.
    pub fn repo_path(&self) -> &str {
        match self {
            Self::AddProject { path, .. } => path,
            Self::CheckoutProjectDefault { project } => &project.path,
        }
    }
}

/// Payload for the "create an empty initial commit, then register the project"
/// flow. A dedicated struct (rather than reusing [`NonDefaultBranchAction`]) so
/// the completion handler has no impossible variant to swallow, and so the
/// repo's real current `branch` is carried through to registration distinct
/// from its resolved `leading_branch`.
#[derive(Clone, Debug)]
pub struct InitialCommitAdd {
    /// The folder to register as a project.
    pub path: String,
    /// The project's display name.
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

/// What a project checkout is on, read before the New Agent prompt opens.
#[derive(Clone, Debug)]
pub struct CreateAgentBranchInspection {
    /// The branch the checkout has checked out.
    pub current_branch: String,
    /// The branch the project leads with.
    pub leading_branch: String,
}

/// One process in a sampled process tree.
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    /// The process's name.
    pub name: String,
    /// The process id.
    pub pid: u32,
    /// CPU use over the sample window, as a percentage of one core.
    pub cpu_percent: f32,
    /// Resident memory, in bytes.
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
    /// One agent tab's provider process, keyed by that tab's own id (the first
    /// tab included).
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
    pub(crate) kind: ResourceKind,
    /// Human-readable description, for display only. Never parse it.
    pub label: String,
    /// Root of the process tree to aggregate.
    pub pid: u32,
}

/// One row of the resource monitor: a process tree and what it uses.
#[derive(Clone, Debug)]
pub struct ResourceStats {
    /// The spine id this row was sampled for, or `None` for the dux and total
    /// rows, which describe no single spine entity.
    pub id: Option<String>,
    /// What the row describes.
    pub kind: ResourceKind,
    /// Human-readable description, for display only. Never parse it.
    pub label: String,
    /// The root process id, or `None` for a row with no single process.
    pub pid: Option<u32>,
    /// CPU use of the whole tree, as a percentage of one core.
    pub cpu_percent: f32,
    /// Resident memory of the whole tree, in bytes.
    pub rss_bytes: u64,
    /// How many processes the tree holds.
    pub process_count: usize,
    /// The tree's processes, the root included.
    pub children: Vec<ProcessInfo>,
}

impl ResourceStats {
    /// Whether this row's breakdown carries any information beyond the row
    /// itself, i.e. whether an expand affordance should be offered at all.
    ///
    /// The threshold is `> 1`, not `> 0`, because `children` always contains the
    /// root process itself, so for a target that spawned no subprocesses an
    /// expand would reveal a duplicate of the row just expanded.
    ///
    /// It lives in core and is projected onto the wire as
    /// `ResourceStatsView::has_breakdown`, so the two surfaces cannot drift on
    /// what is a policy call rather than a self-evident fact.
    pub fn has_breakdown(&self) -> bool {
        self.children.len() > 1
    }
}

/// One row of the project folder browser.
#[derive(Clone, Debug)]
pub struct BrowserEntry {
    /// The directory the row opens.
    pub path: PathBuf,
    /// The row's label.
    pub label: String,
    /// Whether the directory is a git repository.
    pub is_git_repo: bool,
    /// True only for the synthetic parent-directory ("../") row synthesized at
    /// the top of `browser_entries`. Real directory entries are always `false`.
    /// Consumers use this typed flag rather than matching the `"../"` label
    /// string to special-case the parent row.
    pub is_parent: bool,
}

/// The pull request a freshly created agent is pinned to once its row is
/// committed, exactly as a manual attach would pin it.
///
/// Only a fresh copy of a pull request carries one: its branch is named after
/// the pull request's branch rather than being it, so finding the pull request
/// by branch name, which every other pull-request agent relies on, would find
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestPin {
    pub(crate) host: String,
    pub(crate) owner_repo: String,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) state: String,
    /// The pull request's own branch, which is checked out elsewhere; named in
    /// the sentence that says where a push from the copy goes.
    pub(crate) busy_branch: String,
    /// The copy's own branch, where a push from it goes.
    pub(crate) new_branch: String,
}

/// Why an agent launch runs, and what its completion has to report.
#[derive(Clone, Debug)]
pub enum AgentLaunchKind {
    /// The first launch of a newly created agent.
    Create {
        /// The create's success line.
        status_message: crate::status_text::StatusText,
        /// The success line is a warning rather than an info: it carries
        /// something the user must act on, such as a fresh copy of a pull
        /// request that lacks commits the busy branch has.
        status_warns: bool,
        /// What the create's notes said, on their own (any leading warning
        /// included), so a create whose startup command fails can still say
        /// them after its failure sentence. `None` when there were none.
        status_notes: Option<Box<crate::status_text::StatusText>>,
        /// A pull request to pin the new agent to once it is committed.
        pull_request_pin: Option<Box<PullRequestPin>>,
        /// The project repository the create made its worktree in.
        repo_path: String,
        /// Whether this create made the worktree, so a failure removes it.
        owns_worktree: bool,
        /// How the project's startup command ran, when it ran.
        startup_result: Option<crate::startup::StartupCommandResult>,
        /// Opaque correlation id of the shared create-agent `HandlerStatusOp`
        /// (see `Engine::pending_create_ops`). Threads from the
        /// `DispatchCreateAgentRequest` dispatch through the worker so the
        /// launch-ready / launch-failed handlers can resolve the op's final.
        status_op_id: String,
    },
    /// A relaunch of a detached agent.
    Reconnect {
        /// The line the relaunch reports.
        status_message: crate::status_text::StatusText,
    },
    /// A relaunch that stops the running provider first and starts fresh.
    ForceReconnect {
        /// The line the relaunch reports.
        status_message: crate::status_text::StatusText,
    },
    /// A fresh launch after a resume did not come up.
    ResumeFallback {
        /// The line the relaunch reports.
        status_message: crate::status_text::StatusText,
    },
    /// A launch of an agent reopened at startup.
    StartupAutoReopen,
    /// An extra-tab launch. Whether it resumes is decided dynamically, per
    /// provider, by `Engine::tab_resume_decision` (the same rule every other
    /// launch path uses), not hardcoded to "never". `is_fresh` distinguishes a
    /// brand-new `create_tab` (whose row should be deleted if this very first
    /// spawn fails) from an explicit relaunch of an already-persisted dormant
    /// extra tab (whose row is kept and whose real error is surfaced).
    Tab {
        /// Whether the tab was created just now, so a failed first spawn
        /// removes its row.
        is_fresh: bool,
        /// The line the launch reports.
        status_message: crate::status_text::StatusText,
    },
}

/// Everything a launch worker needs to spawn one tab's provider.
#[derive(Clone, Debug)]
pub struct AgentLaunchRequest {
    /// The agent the tab belongs to.
    pub session: AgentSession,
    /// The tab this launch belongs to and the key under which its PTY / runtime
    /// state is tracked. Resolves to `AgentSession::slot_tab_id` for the
    /// session-slot tab; a distinct id for an extra tab. Intentionally has no
    /// `Default` so every construction site must set it explicitly (a missed site
    /// is a compile error), and typed as a [`TabId`] so a construction site
    /// cannot reach for `session.id` instead of asking the resolver.
    pub tab_id: TabId,
    /// The effective provider for this tab. Equals `session.provider` for the
    /// session-slot tab; may differ for an extra tab that was retargeted.
    pub provider: ProviderKind,
    /// The command and arguments to run.
    pub provider_config: ProviderCommandConfig,
    /// The environment the provider runs with.
    pub env: Vec<(String, String)>,
    /// The terminal identity (env add/remove) applied at spawn so the agent sees a
    /// useful terminal name. Resolved from engine state at build time. Empty for
    /// `terminal_identity = "none"` (the pre-capabilities behavior).
    pub identity: crate::term_identity::TerminalIdentity,
    /// Whether the provider continues its previous conversation.
    pub resume: bool,
    /// The terminal size to spawn at, as rows and columns.
    pub pty_size: (u16, u16),
    /// How many lines of scrollback the terminal keeps.
    pub scrollback_lines: usize,
    /// Why the launch runs.
    pub kind: AgentLaunchKind,
    /// TUI-only landing hint: `true` when a fullscreen-seeking gesture started
    /// the launch, so its completion lands fullscreen; every other launch lands
    /// focused-but-minimized. Core builders always construct it `false` and the
    /// TUI flips it before dispatch; the web has no fullscreen concept and never
    /// reads it.
    pub wants_fullscreen: bool,
    /// Which surfaces withhold this launch's completion message. A launch that
    /// relaunches a pane and streams into it confirms nothing; one that lands on
    /// an empty transcript still has to say why it is empty.
    pub status_quiet: crate::statusline::QuietSurfaces,
}

impl AgentLaunchRequest {
    /// Withhold this launch's completion message from the named surfaces
    /// (builder form). Every builder constructs a launch loud, so a site that
    /// quiets one says which tenet case it fails.
    #[must_use]
    pub fn quiet_status_on(mut self, quiet_on: crate::statusline::QuietSurfaces) -> Self {
        self.status_quiet = quiet_on;
        self
    }
}

/// A launch whose provider came up, with the client attached to it.
pub struct AgentLaunchReadyData {
    /// The launch that ran.
    pub request: AgentLaunchRequest,
    /// The running provider's terminal.
    pub client: PtyClient,
    /// The spawn's ticket, kept until the engine has registered the new
    /// session (see [`crate::process_sessions::SpawnGate`]).
    pub spawn_ticket: Option<crate::process_sessions::SpawnTicket>,
}

impl std::fmt::Debug for AgentLaunchReadyData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLaunchReadyData")
            .field("request", &self.request)
            .field("spawn_ticket", &self.spawn_ticket)
            .finish_non_exhaustive()
    }
}

/// A launch whose provider could not be spawned.
#[derive(Clone, Debug)]
pub struct AgentLaunchFailedData {
    /// The launch that ran.
    pub request: AgentLaunchRequest,
    /// Why the spawn failed.
    pub message: String,
}

/// A managed agent and the folder it works in, carried on a pull request
/// create so the job can name the agent that already holds the pull request's
/// code.
#[derive(Clone, Debug)]
pub struct AgentAtWorktree {
    pub(crate) name: String,
    pub(crate) worktree_path: PathBuf,
}

/// A request to create an agent, by where its working copy comes from.
#[derive(Clone, Debug)]
pub enum CreateAgentRequest {
    /// A new worktree in a project, on a new or existing branch.
    NewProject {
        /// The project the agent is created in.
        project: Project,
        /// The agent's name, or `None` for a generated one.
        custom_name: Option<String>,
        /// Attach to a branch of the agent's name that already exists.
        use_existing_branch: bool,
        /// Pull the project's checkout before branching from it.
        pull_before_create: bool,
        /// Copy the checkout's uncommitted changes into the new worktree.
        copy_uncommitted_changes: bool,
        /// Set for the first agent of a clone dux just made: the agent must
        /// start a fresh branch (one that already exists is refused, never
        /// attached to), and a create that fails says what the clone did.
        cloned: Option<crate::clone_project::ClonedFor>,
    },
    /// A new worktree on a pull request's code.
    PullRequest {
        /// The project the pull request belongs to.
        project: Project,
        /// The host the pull request lives on.
        host: String,
        /// The repository as `owner/repo`.
        owner_repo: String,
        /// The pull request's number.
        number: u64,
        /// The pull request's title.
        title: String,
        /// The pull request's state as `gh` reports it.
        state: String,
        /// The pull request's head branch.
        head_branch: String,
        /// The agent's name, or `None` for one named after the branch.
        custom_name: Option<String>,
        /// Attach to a branch of that name that already exists.
        use_existing_branch: bool,
        /// Whether the user agreed to a review copy. Without it a create that
        /// needs one creates nothing and asks.
        review_copy: ReviewCopyConsent,
        /// The asking client's id for this create; see
        /// [`ResolvedPullRequest::ask_id`].
        ask_id: Option<String>,
        /// Every managed agent of the project, as the engine knows them when
        /// the create is dispatched, so the job can tell a branch held by an
        /// agent from one held by anything else. Empty from every constructor;
        /// the dispatch fills it.
        agents: Vec<AgentAtWorktree>,
        /// Set for the first agent of a clone dux just made, so a create that
        /// fails says what the clone already did. Every other create leaves
        /// it empty.
        cloned: Option<crate::clone_project::ClonedFor>,
    },
    /// A new worktree branched from another agent's branch.
    ForkSession {
        /// The project both agents belong to.
        project: Project,
        /// The agent forked from.
        source_session: Box<AgentSession>,
        /// The forked agent's display name.
        source_label: String,
        /// The new agent's name, or `None` for a generated one.
        custom_name: Option<String>,
    },
    /// An agent adopting a dux-managed worktree no agent holds.
    ExistingManagedWorktree {
        /// The project the worktree belongs to.
        project: Project,
        /// The worktree adopted.
        worktree_path: PathBuf,
        /// The branch the worktree is on.
        branch_name: String,
        /// The agent's name, or `None` for one named after the branch.
        custom_name: Option<String>,
    },
    /// A new worktree branched from a worktree dux does not manage.
    ForkExternalWorktree {
        /// The project the worktree belongs to.
        project: Project,
        /// The worktree forked from.
        source_worktree_path: PathBuf,
        /// The forked worktree's display name.
        source_label: String,
        /// The branch the forked worktree is on.
        source_branch: String,
        /// The new agent's name, or `None` for a generated one.
        custom_name: Option<String>,
    },
    /// A standalone agent: run the provider in a folder the user already has.
    ///
    /// It carries no project, because a standalone agent belongs to none, and its
    /// job path provisions nothing: no branch, no worktree, no startup command.
    /// On failure there is nothing on disk to roll back, only the record, so
    /// `rollback_created_worktree` is never reachable from this arm.
    Standalone {
        /// The folder to run in, exactly as the user chose it. Never created,
        /// moved or removed by dux.
        folder: PathBuf,
        /// The agent's display name, already resolved and guaranteed non-empty
        /// by [`crate::git::standalone_agent_title`].
        title: String,
        /// The provider to launch. Resolved at dispatch, from the request's
        /// optional override or the global default.
        provider: ProviderKind,
    },
}

impl CreateAgentRequest {
    /// The project id this request belongs to, or `None` for a standalone
    /// agent, which belongs to no project.
    ///
    /// Status keying does NOT go through this: the create-agent busy and its
    /// final share an opaque `status_op_id`, which is already project-free.
    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::NewProject { project, .. }
            | Self::PullRequest { project, .. }
            | Self::ForkSession { project, .. }
            | Self::ExistingManagedWorktree { project, .. }
            | Self::ForkExternalWorktree { project, .. } => Some(&project.id),
            Self::Standalone { .. } => None,
        }
    }
}

/// A message a background worker sends back to the engine.
#[derive(Debug)]
pub enum WorkerEvent {
    /// Status update delivered via the worker channel so it stays FIFO with
    /// the completion event of the operation it announces. Posted by
    /// `Engine::spawn_command_worker` before the worker thread starts so the
    /// busy status is guaranteed to reach `process_worker_event` ahead of
    /// any event the worker can produce.
    CommandWorkerStarted(StatusUpdate),
    /// A recreate of an agent's working copy finished, however it ended. It is
    /// the recreate's only completion: the handler releases the in-flight guard,
    /// records what the checkout did, and resolves the op that produces the
    /// final, which needs the engine to see what is running by then.
    WorkingCopyRecreated {
        /// The agent whose working copy was recreated.
        session_id: String,
        /// The branch arm the checkout took, or the sentence explaining what
        /// stopped it. The in-flight guard is released either way.
        outcome: Result<crate::working_copy::RecreatedBranch, String>,
    },
    /// A status a background poller produced, delivered through the worker lane
    /// so it reaches whichever surface is draining rather than only the one
    /// that raised it. The web's changed-files service is the producer today.
    PollerStatus(StatusUpdate),
    /// A progress update emitted while an agent is being created/forked/attached.
    /// `status_op_id` is the opaque id of the shared create-agent `HandlerStatusOp`
    /// (see `Engine::pending_create_ops`); the handler looks the op up and re-emits
    /// `op.progress(message)` on the same id, so the dispatch's busy and every
    /// progress update render as one in-place toast that the final state dismisses.
    CreateAgentProgress {
        /// The create op the update belongs to.
        status_op_id: String,
        /// What the create is doing now.
        message: crate::status_text::StatusText,
    },
    /// A create stopped before its agent existed.
    CreateAgentFailed {
        /// The create op that failed.
        status_op_id: String,
        /// Why the create failed.
        message: crate::status_text::StatusText,
        /// The name the create had settled on when it failed (a pull request's
        /// review copy has a different one from the name asked for), or
        /// `None` when it had not settled one.
        attempted_name: Option<String>,
    },
    /// A provider launch came up.
    AgentLaunchReady(Box<AgentLaunchReadyData>),
    /// A provider launch could not be spawned.
    AgentLaunchFailed(Box<AgentLaunchFailedData>),
    /// A changed-files listing was computed for the watched worktree.
    ChangedFilesReady {
        /// The computed (staged, unstaged) lists, or the message git failed
        /// with. The error is carried rather than flattened into empty lists:
        /// "git could not answer" and "the worktree is clean" are different
        /// facts, and reporting the first as the second tells the user nothing
        /// has changed when dux has no idea what is in the tree.
        outcome: Result<(Vec<ChangedFile>, Vec<ChangedFile>), String>,
        /// The worktree these lists were computed for. The poller runs
        /// `git::changed_files` off-thread, so by the time this event lands the
        /// watch may have moved; the tag lets the engine drop a stale poll rather
        /// than overwrite the current session's files.
        worktree: PathBuf,
    },
    /// A standalone agent's folder was classified by
    /// `Engine::spawn_folder_repo_probe`. The engine stores the verdict and, if
    /// that agent's changes panel is the one on screen, enrols or drops the
    /// changed-files watch to match, so a folder that became a repository since
    /// the last look starts working without the user doing anything else.
    FolderRepoStatusReady {
        /// The standalone agent whose folder was classified.
        session_id: String,
        /// What the folder is.
        status: crate::git::FolderRepoStatus,
    },
    /// A `spawn_status_op` worker finished and carried back its resolved final
    /// (the success/failure message or a clear, already keyed).
    StatusOpCompleted {
        /// The final to show.
        resolved: crate::engine::ResolvedFinal,
    },
    /// A worker changed a worktree's files (a discard) and has sent its final:
    /// the changed-files listing is read again.
    WorktreeFilesChanged,
    /// A pull of a project checkout or an agent's worktree finished.
    PullCompleted {
        /// The repository pulled.
        repo_path: String,
        /// What was pulled.
        target: PullTarget,
        /// How the pull ended, or git's error.
        result: Result<PullOutcome, String>,
        /// The status final resolved by the dispatch-site `StatusOp` (the success
        /// or failure message, already keyed). The handler still uses `result`
        /// for its domain mutations and emits this for the user-facing status.
        status: crate::engine::ResolvedFinal,
    },
    /// A folder browser listing was read.
    BrowserEntriesReady {
        /// The directory listed.
        dir: PathBuf,
        /// Its rows.
        entries: Vec<BrowserEntry>,
    },
    /// A project's worktrees were listed for the adopt picker.
    ProjectWorktreesReady {
        /// The project listed.
        project_id: String,
        /// The worktrees, or why they could not be listed.
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
        /// The project listed.
        project_id: String,
        /// The managed worktrees, or why they could not be listed.
        result: Result<Vec<crate::worktree_manager::ManagedWorktree>, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose final is resolved in
        /// the completion handler (it depends on whether the manager is still
        /// open, which the worker can't see).
        status_op_id: Option<String>,
    },
    /// A copy to the clipboard finished.
    ClipboardCopyCompleted {
        /// Human-readable success message shown in the status bar.
        label: String,
        /// Whether the copy worked, or why not.
        result: Result<(), String>,
        /// Status final resolved at the call site by the clipboard `StatusOp`:
        /// the success (`label`) or failure message, already keyed. The handler
        /// emits this for the user-facing status.
        status: crate::engine::ResolvedFinal,
    },
    /// Agents whose worktree is on a branch other than the recorded one, as
    /// session id and branch.
    BranchSyncReady(Vec<(String, String)>),
    /// A git branch rename for an agent finished.
    BranchRenameCompleted {
        /// The agent renamed.
        session_id: String,
        /// The branch's new name.
        new_branch: String,
        /// The title the agent had before, restored when the rename failed.
        previous_title: Option<String>,
        /// Whether git renamed the branch, or its error.
        result: Result<(), String>,
        /// Status final resolved at dispatch by the rename `StatusOp`; the handler
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
    /// `generation` is stamped before the probe is spawned and travels on every
    /// way it can finish, the result synthesised on a worker panic included. The
    /// handler discards a stale generation before anything else it would do, so
    /// two probes launched close together cannot let the older answer win.
    GhStatusChecked {
        /// The probe run this answers.
        generation: u64,
        /// What `gh` said.
        outcome: crate::gh::GhProbe,
    },
    /// Pull-request answers for the agents a sync cycle asked about.
    PrStatusReady(Vec<PrStatusResult>),
    /// The debounce that refused an agent's owed branch check has passed.
    PrCheckOwedDue(String),
    /// A one-shot PR check worker panicked; carries the session id so its
    /// `InFlightKey::PrCheck` guard is cleared without wiping the PR badge.
    PrCheckAborted(String),
    /// A typed pull request reference has been matched against every project's
    /// configured address. One `git` call per project ran on the worker; the
    /// answer is not cached anywhere, because nothing dux watches would tell it
    /// when the answer changed (see
    /// [`crate::pr_reference::resolve_reference_projects`]).
    ///
    /// The surface branches on the shape of the match list: exactly one project
    /// proceeds to the lookup, several mean the repository is checked out twice
    /// and the user picks, and none means dux can only offer the project picker,
    /// because dux does not clone.
    ///
    /// `result` is a `Result` because a worker that fell over is not the same
    /// answer as an empty match list: reporting a panic as "no project is a
    /// checkout of that repository" states something dux never found out.
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
        /// With no match and an offer possible: the pull request as `gh`
        /// reported it, for the confirmation to show.
        offer: Option<crate::pr_reference::PullRequestCloneFacts>,
        /// With no match and an offer possible but the lookup failed: the
        /// lookup's own sentence.
        clone_unavailable: Option<String>,
        /// The op the lookup reports under, when a surface keyed one.
        status_op_id: Option<String>,
    },
    /// A `gh pr view` lookup finished.
    PullRequestResolved {
        /// The pull request, or why it could not be resolved.
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
    /// Background `git worktree remove` for a session-initiated delete has
    /// finished. On `Ok`, the result says what happened to each branch the
    /// removal targeted (used for the status message). On `Err`, the message is
    /// the formatted error; the session record must be preserved so the user
    /// can retry.
    WorktreeRemoveCompleted {
        /// The agent whose worktree was removed.
        session_id: String,
        /// What happened to each branch, or the formatted error.
        result: Result<crate::engine::RemovedBranches, String>,
    },
    /// A worktree removal found operations still running in the worktree and
    /// is waiting for them. `waiting_for` names them ("a pull and a push").
    WorktreeRemoveWaiting {
        /// The agent whose worktree is being removed.
        session_id: String,
        /// What the removal is waiting for.
        waiting_for: String,
    },
    /// A project deletion that was waiting for an agent being created in the
    /// project may go on: the create finished, or the wait ran out.
    ProjectDeletionContinue {
        /// The project being deleted.
        project_id: String,
    },
    /// Background `git switch <target_branch>` run from a non-default branch
    /// warning modal has finished. On `Ok`, the main loop continues the
    /// original action. On `Err`, the formatted git error is surfaced.
    NonDefaultBranchCheckoutCompleted {
        /// What runs once the switch landed.
        action: NonDefaultBranchAction,
        /// The branch switched to.
        target_branch: String,
        /// Whether the switch worked, or git's error.
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
        /// The project add the commit was made for.
        add: InitialCommitAdd,
        /// Whether the commit was made, or git's error.
        result: Result<(), String>,
        /// Correlation id for a web add-project `HandlerStatusOp` (resolved in
        /// `drive_add_project_followup`), or a TUI `pending_checkout_inspect_ops`
        /// op (dismissed in `drain_events`). `None` when no keyed status is driven.
        status_op_id: Option<String>,
    },
    /// A clone's worker finished, at `path` (the destination's identity, which
    /// names its in-flight key). `Ok` is a clone ready to add as a project;
    /// `Err` says how it stopped short of one (never `HandedOff`).
    RepositoryCloned {
        /// The clone's op, when one was opened.
        status_op_id: Option<String>,
        /// The clone's destination.
        path: PathBuf,
        /// The name of the agent the clone is for.
        agent_name: String,
        /// The repository key a pull request clone holds, released when the
        /// pending clone is gone.
        repository_key: Option<String>,
        /// Whether the clone made a new copy or added an existing folder.
        mode: crate::clone_project::CloneMode,
        /// The clone ready to add, or how it stopped short.
        result: Result<crate::clone_project::ClonedRepository, crate::clone_project::CloneOutcome>,
    },
    /// Background inspection of the selected project checkout before opening
    /// the New Agent prompt.
    CreateAgentBranchInspected {
        /// The project inspected.
        project: Project,
        /// What its checkout is on, or why it could not be read.
        result: Result<CreateAgentBranchInspection, String>,
        /// Correlation id for a TUI `HandlerStatusOp` whose keyed busy is dismissed
        /// when the inspection completes (the visible final comes from the
        /// downstream `ContinueCreateAgentAfterInspection` prompt's `set_info` on
        /// success, or the engine's error `Status` on failure). `None` for callers
        /// that don't drive a handler-resolved status. This flow is TUI-only.
        status_op_id: Option<String>,
    },
    /// A project checkout's branch was read for the sidebar.
    ProjectBranchStatusReady {
        /// The project read.
        project_id: String,
        /// The checkout's branch and how it relates to the leading branch, or
        /// the error.
        result: Result<(String, ProjectBranchStatus), String>,
    },
    /// A project checkout was inspected before switching it to its default
    /// branch.
    CheckoutProjectDefaultBranchInspected {
        /// The project inspected.
        project: Project,
        /// The checkout's branch and how sure dux is it is not the default, or
        /// the error.
        result: Result<(String, Option<BranchWarningKind>), String>,
        /// Correlation id for a web checkout `HandlerStatusOp`, carried through to
        /// worker 2 (`run_add_project_checkout_job`) so the eventual
        /// `NonDefaultBranchCheckoutCompleted` resolves the right op. `None` for
        /// the TUI.
        status_op_id: Option<String>,
    },
    /// The "Change base branch" worker finished with the project folder
    /// (`crate::base_branch::switch_to_base_branch`). The handler releases the
    /// folder, saves the base on success, and resolves the op registered in
    /// `Engine::pending_change_base_ops` under `status_op_id`, for either
    /// surface.
    ProjectBaseBranchChanged {
        /// The project whose base branch changed.
        project: Project,
        /// The branch asked for.
        branch: String,
        /// What the switch did, or why it was refused.
        result: Result<
            crate::base_branch::BaseBranchSwitched,
            crate::base_branch::BaseBranchChangeFailure,
        >,
        /// The op the change reports under.
        status_op_id: String,
    },
    /// A config reload read the file: the new config, or why it could not be
    /// loaded.
    ConfigReloadReady(Box<Result<Config, String>>),
    /// A project change finished saving to the session database.
    ProjectPersistenceCompleted {
        /// The change saved.
        action: ProjectPersistenceAction,
        /// Whether it was saved, or the error.
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
        /// Whose startup command runs were listed.
        scope_label: String,
        /// The listing, or why it could not be read.
        result: Result<crate::startup::StartupCommandLogListing, String>,
    },
    /// One already-listed run finished reading off-thread, because the user
    /// moved the picker's selection onto it. `path` is the correlation handle:
    /// a reply for a run that is no longer selected is dropped, so a fast walk
    /// down the list cannot land a stale body under a newer selection.
    StartupCommandLogContentLoaded {
        /// The run file that was read.
        path: std::path::PathBuf,
        /// Its contents, or why it could not be read.
        result: Result<String, String>,
    },
    /// The in-process web-server flip pre-flight finished on a worker thread,
    /// where local-mode address resolution and each `TcpListener::bind` run
    /// because resolution shells out to `tailscale ip`. On success the bound
    /// listeners and their display URLs come back so the main loop can stash the
    /// flip; on failure the TUI stays up. `warning` is a non-fatal note.
    ServerFlipPreflightReady {
        /// The bound listeners and their display URLs, or why binding failed.
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        /// A non-fatal note to show beside the result.
        warning: Option<String>,
        /// What the pre-flight learned, carried to the flip's log so it opens
        /// with the same lines `dux server` prints.
        startup: crate::serve_log::StartupNotes,
    },
    /// The background web server's bind pre-flight finished on a worker thread.
    ///
    /// Same payload and same reason as `ServerFlipPreflightReady`, and a separate
    /// variant rather than a flag on it because the two go to different places:
    /// the flip's listeners end a TUI session, and these start a serve alongside
    /// one.
    BackgroundServerPreflightReady {
        /// The bound listeners and their display URLs, or why binding failed.
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        /// A non-fatal note to show beside the result.
        warning: Option<String>,
        /// What the pre-flight learned, carried to the serve's log so it opens
        /// with the same lines `dux server` prints.
        startup: crate::serve_log::StartupNotes,
    },
    /// A live `[server] tailscale` change the BACKGROUND web server carried out.
    ///
    /// It arrives on this lane rather than as a return value because applying a
    /// mode can run a bounded address detection, and the caller is the terminal
    /// UI's run loop, which is also the serve's engine servicer: blocking it
    /// would stop both surfaces for the length of a subprocess call.
    TailscaleModeApplied {
        /// The mode asked for.
        mode: crate::config::TailscaleMode,
        /// What applying it did.
        outcome: crate::config::TailscaleModeOutcome,
    },
}

/// How a successful (non-erroring) pull worker run ended. `current_branch` is
/// the checkout's branch after the run, when the worker re-read it.
#[derive(Clone, Debug)]
pub enum PullOutcome {
    /// The branch was pulled from origin.
    Pulled {
        /// The branch the checkout is on after the pull, when it was re-read.
        current_branch: Option<String>,
    },
    /// The repo has no `origin` remote: nothing to pull, and that is a normal
    /// state for local-only repos, not a failure.
    NoOrigin {
        /// The branch the checkout is on, when it was re-read.
        current_branch: Option<String>,
    },
}

impl PullOutcome {
    pub(crate) fn current_branch(&self) -> Option<&String> {
        match self {
            PullOutcome::Pulled { current_branch } | PullOutcome::NoOrigin { current_branch } => {
                current_branch.as_ref()
            }
        }
    }
}

/// What a pull runs in.
#[derive(Clone, Debug)]
pub enum PullTarget {
    /// A project's own checkout.
    Project {
        /// The project pulled.
        project_id: String,
        /// The project's display name.
        project_name: String,
        /// The branch the project leads with, when known.
        leading_branch: Option<String>,
    },
    /// An agent's worktree.
    Session,
}

/// Snapshot of session data shared with the branch-sync background worker.
#[derive(Clone, Debug)]
pub struct BranchSyncEntry {
    pub(crate) session_id: String,
    pub(crate) worktree_path: String,
    pub(crate) branch_name: String,
}

/// The identity of a manually attached ("pinned") pull request, carried on a
/// [`PrSyncEntry`] so the sync planner queries the PINNED repo (which may be a
/// fork, or any repo other than the session's remote) instead of deriving a
/// target from the worktree's remote. Identity only; the cached state/title
/// ride in `known_pr`, which both construction sites set to the override row
/// for a pinned session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedPr {
    pub(crate) host: String,
    pub(crate) owner_repo: String,
    pub(crate) number: u64,
}

/// One agent's pull-request answer, with the branch generation it was asked
/// under (see `Engine::pr_branch_generations`), so an answer that lands after
/// the agent drifted onto another branch is dropped instead of being saved
/// against the new one, while one that lands after an explicit rename stands.
#[derive(Clone, Debug)]
pub struct PrStatusResult {
    pub(crate) session_id: String,
    pub(crate) branch_generation: u64,
    pub(crate) pr: Option<crate::model::PrInfo>,
}

/// Snapshot of session data shared with the PR-sync background worker.
#[derive(Clone, Debug)]
pub struct PrSyncEntry {
    pub(crate) session_id: String,
    pub(crate) branch_name: String,
    pub(crate) worktree_path: String,
    /// If we already know a PR for this session, the worker can use `gh pr view`
    /// (works even after branch deletion) and skip a merged PR entirely. A CLOSED
    /// one is still refreshed, because it can be reopened.
    /// For a pinned session this is the OVERRIDE row, never the `session_prs`
    /// latest (which can be a different, autodetected PR).
    pub(crate) known_pr: Option<StoredPr>,
    /// Whether the agent process has exited. Used to skip PR discovery calls for
    /// sessions that are both exited and merged (nobody is pushing to that branch
    /// anymore, and a merge is final), and to keep an exited session whose PR is
    /// CLOSED out of the blind poll while a one-shot trigger still refreshes it.
    pub(crate) agent_exited: bool,
    /// A manually attached PR. When set, the planner short-circuits the
    /// remote-derived target: the query goes to the pinned `(host, owner_repo)`,
    /// the host policy gates the PINNED host, and the only alias emitted is the
    /// by-number one for the pinned PR (no head-ref discovery).
    pub pinned: Option<PinnedPr>,
    /// Whether the agent sits in the list's INACTIVE tail (detached or exited),
    /// as decided by the one `flat_list::is_inactive` predicate the sidebar
    /// uses. The blind poll puts these entries on `ui.pr_poll_inactive_interval_seconds`
    /// instead of the active clock; every other trigger ignores the flag.
    ///
    /// Distinct from `agent_exited`, which is about a live PTY rather than the
    /// session's own status, and which the alias planner reads for a different
    /// question (whether a terminal pull request is worth a call at all).
    pub inactive: bool,
    /// When dux minted the branch, for an agent still on a branch dux created
    /// for it; `None` for a branch that existed before the agent, or one the
    /// agent has since moved off. No pull request from a minted branch can be
    /// older than this, which is what tells it apart from an earlier branch
    /// that used the same name when a pull request is found by head name.
    pub(crate) branch_minted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The agent's branch generation when this entry was planned, carried
    /// into the result (see [`PrStatusResult`]).
    pub(crate) branch_generation: u64,
}

/// A project change to save to the session database.
#[derive(Clone, Debug)]
pub enum ProjectPersistenceAction {
    /// Register a project.
    Add {
        /// The project registered.
        project: Project,
        /// The line reported once it is saved.
        status_message: crate::status_text::StatusText,
    },
    /// Forget a project, keeping its worktrees.
    Remove {
        /// The project forgotten.
        project_id: String,
        /// Its display name.
        project_name: String,
    },
    /// Delete a project and its worktrees.
    Delete {
        /// The project deleted.
        project_id: String,
        /// Its display name.
        project_name: String,
    },
    /// Change a project's default provider.
    UpdateDefaultProvider {
        /// The project changed.
        project_id: String,
        /// Its display name.
        project_name: String,
        /// The provider it now defaults to, or `None` to follow the global
        /// default.
        provider: Option<ProviderKind>,
        /// The global default, named when the project follows it.
        global_default: ProviderKind,
    },
    /// Change whether a project's agents reopen at startup.
    UpdateAutoReopen {
        /// The project changed.
        project_id: String,
        /// Its display name.
        project_name: String,
        /// The project's choice, or `None` to follow the global setting.
        auto_reopen_agents: Option<bool>,
    },
    /// Change a project's startup command.
    UpdateStartupCommand {
        /// The project changed.
        project_id: String,
        /// Its display name.
        project_name: String,
        /// The command new agents run, or `None` for none.
        startup_command: Option<String>,
    },
    /// Change a project's environment variables.
    UpdateEnv {
        /// The project changed.
        project_id: String,
        /// Its display name.
        project_name: String,
        /// The variables the project's agents and terminals get on top of the
        /// global ones.
        env: BTreeMap<String, String>,
    },
}

#[cfg(test)]
mod review_copy_consent_tests {
    use super::{ReviewCopyAnswer, ReviewCopyConsent, ShownReason};

    #[test]
    fn a_request_reads_as_the_consent_it_gave() {
        let read = |agreed, warned: Option<&str>, by_flag| {
            ReviewCopyConsent::from_request(
                ReviewCopyAnswer { agreed, by_flag },
                warned.map(str::to_string),
            )
        };
        assert_eq!(
            read(false, Some("w"), true),
            ReviewCopyConsent::WithheldByFlag
        );
        assert_eq!(read(false, None, false), ReviewCopyConsent::Withheld);
        assert_eq!(read(true, None, true), ReviewCopyConsent::Blanket);
        assert_eq!(
            read(true, Some("w"), false),
            ReviewCopyConsent::Warned("w".to_string())
        );
    }

    #[test]
    fn an_upfront_agreement_names_its_reason_or_reads_as_a_request() {
        let read = |agreed, reason: Option<&str>, by_flag| {
            ReviewCopyConsent::from_upfront(
                ReviewCopyAnswer { agreed, by_flag },
                reason.map(str::to_string),
                Some("b".to_string()),
            )
        };
        assert_eq!(
            read(true, Some("busy"), false),
            ReviewCopyConsent::Reason(Box::new(ShownReason {
                kind: "busy".to_string(),
                branch: Some("b".to_string()),
            }))
        );
        assert_eq!(read(true, None, false), ReviewCopyConsent::Blanket);
        assert_eq!(
            read(false, Some("busy"), true),
            ReviewCopyConsent::WithheldByFlag
        );
    }
}
