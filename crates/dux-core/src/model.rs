use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::ids::TabIdRef;
use crate::pty::PtyClient;

/// GitHub CLI availability status.
///
/// First probed at startup, and RE-probed whenever the GitHub integration is
/// switched from off to on and on every config reload, on both surfaces. It is
/// last-known-good rather than a once-per-process value: a decisive probe
/// replaces it, and a transient failure leaves the previous value standing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhStatus {
    /// Not yet checked.
    #[default]
    Unknown,
    /// `gh` binary not found on PATH.
    NotInstalled,
    /// `gh` found, and it said plainly that nobody is logged in. Authoritative
    /// about what `gh` reported, so it never overwrites a working state on a
    /// guess; dux still re-checks on its timer, because `gh auth login` in
    /// another terminal is exactly how this state ends.
    NotAuthenticated,
    /// `gh` found, but it could not be consulted: it timed out, failed to
    /// launch, or answered with a rate limit, an API error or a network fault.
    ///
    /// NOT authoritative. It is the honest name for "dux does not know yet",
    /// and it exists because recording it as [`Self::NotAuthenticated`] latched
    /// a momentary HTTP 403 into "you are logged out" for the whole run, hiding
    /// every pull-request affordance on both surfaces until dux restarted. dux
    /// re-probes on a timer while the status sits here.
    Unreachable,
    /// `gh` installed and authenticated.
    Available,
}

/// State of a GitHub pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// Cached information about a GitHub pull request associated with a session.
#[derive(Clone, Debug)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub title: String,
    pub host: String,
    pub owner_repo: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderKind(String);

impl ProviderKind {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub explicit_default_provider: Option<ProviderKind>,
    pub default_provider: ProviderKind,
    pub leading_branch: Option<String>,
    pub auto_reopen_agents: Option<bool>,
    pub startup_command: Option<String>,
    pub env: BTreeMap<String, String>,
    pub current_branch: String,
    pub branch_status: ProjectBranchStatus,
    pub path_missing: bool,
    /// When this project was first added, as persisted in the SQLite `projects`
    /// table. This is derived/runtime state (not portable config), surfaced so
    /// clients can show an "added" date. `None` for projects constructed
    /// in-memory before a store row exists (e.g. during creation/migration).
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectBranchStatus {
    Leading,
    NotLeading,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Detached,
    Exited,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Detached => "detached",
            Self::Exited => "exited",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "exited" => Self::Exited,
            _ => Self::Detached,
        }
    }
}

/// Where an agent's branch came from, and therefore whether deleting the agent
/// may delete the branch.
///
/// Deleting an agent with "also delete the worktree" ticked force-deletes the
/// branches involved. That is right for a branch dux minted for the agent and
/// wrong for one that existed before it: an agent attached to `develop`, or
/// adopted from a worktree the user already had, must never take `develop` with
/// it. Recorded once at creation and never mutated (see the storage layer's
/// INSERT-but-not-UPDATE handling).
///
/// Separate variants rather than a bool because the delete copy wants the
/// distinction: "existed before this agent" and "came with the worktree this
/// agent adopted" are different sentences.
///
/// What counts as "existed before" is per create arm, because what the user
/// pointed at differs. A plain create attaches to a branch the user TYPED and
/// confirmed against a preflight that names its location, so a remote-only
/// branch is still the branch they chose and it keeps its `AttachedExisting`.
/// The PR arm's name comes from the pull request rather than from the user, so
/// it asks the narrower question, and asks it whatever the user confirmed: did
/// `refs/heads/<name>` exist before dux ran?
/// Against a remote-only ref, `git worktree add` DWIMs the local branch into
/// existence, which makes it dux's own work, and a PR agent that reported "this
/// branch existed before the agent" about a branch dux had just created is the
/// bug that split the two questions apart. Nothing here ever deletes a remote
/// branch, so only local refs are in play.
///
/// There is deliberately no `Default`. The only sensible default would be
/// `CreatedByDux`, and a struct-update literal that silently filled it in
/// would be a force-delete of a user's branch decided by nobody. Every
/// construction site says which one it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchProvenance {
    /// dux created the branch for this agent: a fresh create, a fork, or a PR
    /// arm that put `refs/heads/<name>` there itself, whether by fetching the
    /// pull request head or by checking out a remote-tracking ref that had no
    /// local branch yet. Deleting the agent may delete the branch: no local
    /// branch existed before dux made it.
    CreatedByDux,
    /// The agent was attached to a branch that already existed. The branch is
    /// the user's; deleting the agent keeps it.
    AttachedExisting,
    /// The agent was adopted from a worktree that already existed, branch and
    /// all. The branch is the user's; deleting the agent keeps it.
    Adopted,
    /// The stored value is one this binary has never heard of, written by a
    /// newer dux. Treated exactly like a user's branch for every DECISION
    /// (nothing is deleted on a guess), and given its own copy, because
    /// folding it into `AttachedExisting` made the delete dialog assert
    /// "existed before this agent" about a branch nobody here can say
    /// anything about.
    Unknown,
}

impl BranchProvenance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CreatedByDux => "created",
            Self::AttachedExisting => "attached",
            Self::Adopted => "adopted",
            // Round-tripping this would rewrite a newer dux's value with a
            // word it does not know either. It never happens: provenance is
            // written on INSERT only, and an INSERT is a session this binary
            // just created, which always has a real variant.
            Self::Unknown => "unknown",
        }
    }

    /// Parse the stored text. **An unrecognized value is NOT treated as
    /// created**: a future binary may write a variant this one has never heard
    /// of, and guessing "dux made it" would force-delete a branch on that
    /// guess. Losing a cleanup is recoverable; losing a branch is not. This is
    /// distinct from the MIGRATION default (`'created'`, which preserves
    /// today's behavior for rows written before the column existed and whose
    /// true provenance is unknowable).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value {
            "created" => Self::CreatedByDux,
            "adopted" => Self::Adopted,
            "attached" => Self::AttachedExisting,
            _ => Self::Unknown,
        }
    }

    /// Whether deleting this agent may delete its branches **unasked**: the
    /// answer for a caller with no dialog in front of it, and the state the
    /// delete dialogs' "also delete the branch" checkbox starts in.
    ///
    /// It is no longer the last word. A user who ticks the box on a dialog
    /// that named the branch and warned about it has answered the question
    /// themselves, and [`BranchProvenance::resolve_branch_deletion`] is where
    /// the two are folded together.
    pub fn dux_may_delete_branch(&self) -> bool {
        matches!(self, Self::CreatedByDux)
    }

    /// What a delete request actually does to this agent's branches, once the
    /// user's own answer is folded into the provenance default.
    ///
    /// `requested` is the delete dialog's "also delete the branch" checkbox.
    /// `None` means nobody was asked: the project-removal cascade, the create
    /// rollback and factory reset all run with no dialog in front of them, and
    /// they keep the provenance default exactly as they always have.
    ///
    /// `Some(true)` against a pre-existing branch is the ONE thing that deletes
    /// a branch dux did not create from an agent delete. That is deliberate:
    /// it is the same explicit act the worktree manager's checkbox already is,
    /// made where the user is standing rather than only in a dialog they have
    /// to go and find. `Some(false)` against a branch dux did create is the
    /// mirror, and it is why the box is a real control rather than a label.
    pub fn resolve_branch_deletion(self, requested: Option<bool>) -> bool {
        requested.unwrap_or_else(|| self.dux_may_delete_branch())
    }

    /// Why this agent's pre-existing branch is not dux's to delete, as a
    /// sentence fragment following the branch name.
    ///
    /// Public because every surface that has to write that sentence must write
    /// THIS one: the TUI's delete-agent dialog hand-rolled a second, shorter
    /// wording of it and drifted on the adopted case.
    pub fn kept_reason(&self) -> &'static str {
        match self {
            // Never rendered: a created-by-dux agent's branches are deleted, so
            // no keep sentence is written for it. Answered anyway so the match
            // stays exhaustive and a future caller cannot get a panic.
            Self::CreatedByDux => "was created by dux",
            Self::AttachedExisting => "existed before this agent",
            Self::Adopted => "came with the worktree this agent adopted",
            // The only true thing this binary can say about a value it does
            // not recognize. It is enough to justify the branch surviving,
            // which is the whole job of this sentence.
            Self::Unknown => "is not a branch dux created",
        }
    }

    /// The sentence(s) naming every branch a worktree-removing delete
    /// deliberately KEPT, with a per-branch reason.
    ///
    /// Drift matters here: when the agent moved off the branch it was born on,
    /// two branches survive and only one of them predates the agent, so saying
    /// "existed before this agent" of both would be false. The birth branch
    /// carries the provenance reason; a distinct current branch is named as
    /// what it is, a branch created inside the agent's worktree.
    ///
    /// Names `git branch -D` because the branch outlives the worktree and the
    /// only surface that could delete it (a worktree manager) can no longer
    /// reach it once the worktree is gone.
    ///
    /// Shared by the TUI status line and the web toast so both say the same
    /// thing.
    pub fn kept_branches_note(&self, branch_name: &str, initial_branch: &str) -> String {
        let drifted = !initial_branch.is_empty() && initial_branch != branch_name;
        if drifted {
            format!(
                "Its branch \"{branch_name}\" was created inside this agent's worktree and was kept, \
                 and its branch \"{initial_branch}\" {} and was kept. \
                 Delete either yourself with git branch -D \"{branch_name}\" or \
                 git branch -D \"{initial_branch}\" if you no longer need them.",
                self.kept_reason()
            )
        } else {
            format!(
                "Its branch \"{branch_name}\" {} and was kept. \
                 Delete it yourself with git branch -D \"{branch_name}\" if you no longer need it.",
                self.kept_reason()
            )
        }
    }
}

/// Why a worktree-removing delete left the agent's branches on disk.
///
/// Two genuinely different sentences, which is why this is not a bool: "that
/// branch was never dux's to delete" and "you asked dux not to" are different
/// facts, and only the first of them tells the user something they did not
/// already know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchKeptReason {
    /// The branch predates the agent and nobody overrode that, so the
    /// provenance default stood. Carries the provenance so the sentence can
    /// say which kind of "predates" it was.
    NotDuxs(BranchProvenance),
    /// The delete dialog offered to remove the branch and the user left the
    /// box unticked. Reachable for a branch dux created too, which is exactly
    /// the case the provenance sentence cannot describe.
    UserDeclined,
}

impl BranchKeptReason {
    /// The sentence(s) naming every branch the removal kept, with its reason.
    /// See [`BranchProvenance::kept_branches_note`] for the drift rule.
    pub fn kept_branches_note(&self, branch_name: &str, initial_branch: &str) -> String {
        match self {
            Self::NotDuxs(provenance) => provenance.kept_branches_note(branch_name, initial_branch),
            Self::UserDeclined => {
                let drifted = !initial_branch.is_empty() && initial_branch != branch_name;
                if drifted {
                    format!(
                        "Its branches \"{branch_name}\" and \"{initial_branch}\" were kept \
                         because you left the branch box unticked. Delete either yourself with \
                         git branch -D \"{branch_name}\" or git branch -D \"{initial_branch}\" \
                         if you no longer need them."
                    )
                } else {
                    format!(
                        "Its branch \"{branch_name}\" was kept because you left the branch box \
                         unticked. Delete it yourself with git branch -D \"{branch_name}\" if \
                         you no longer need it."
                    )
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompanionTerminalStatus {
    NotLaunched,
    Running,
    Exited,
}

impl CompanionTerminalStatus {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSurface {
    Agent,
    Terminal,
}

/// Which of the two shapes an [`AgentWorkspace`] is, as a bare tag. Persisted
/// in its own additive SQLite column and carried on the wire, so a reader can
/// decide how to read the rest of the row BEFORE believing any git field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentWorkspaceKind {
    /// A working copy dux created and owns.
    Managed,
    /// A folder the user already had, which dux only visits.
    Folder,
}

impl AgentWorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Folder => "folder",
        }
    }

    /// Parse the stored kind. **An unrecognized value reads as managed**, which
    /// is the opposite of the safety direction [`BranchProvenance::from_str`]
    /// takes, and deliberately so. The danger here is the reverse one: reading
    /// an unknown kind as a folder would hand a path dux does not understand to
    /// the folder rules, where the delete flow believes the directory is the
    /// user's and every git question answers "none". Reading it as managed
    /// keeps every guard that protects a directory dux might own, which is the
    /// side to be wrong on for a row this binary cannot classify. What such a
    /// row's git columns actually contain is unknown, so the surfaces render
    /// whatever is there rather than anything trustworthy; that is the accepted
    /// cost. The arm is reachable only from a row written by a newer dux, or one
    /// edited by hand.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value {
            "folder" => Self::Folder,
            _ => Self::Managed,
        }
    }
}

/// Everything a managed working copy carries: the project it belongs to and the
/// full git identity of the branch dux minted or attached for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedWorkspace {
    pub project_id: String,
    pub project_path: Option<String>,
    pub source_branch: String,
    pub branch_name: String,
    /// The branch this agent was created on. Immutable identity: unlike
    /// `branch_name` (which the branch-sync poller and intentional renames keep
    /// in step with the worktree's actual current branch), `initial_branch` is
    /// set once at creation and never mutated afterward. Surfaced so the UI can
    /// show branch lineage and flag drift when `branch_name != initial_branch`.
    pub initial_branch: String,
    /// Where this agent's branch came from, and therefore whether deleting the
    /// agent may delete the branch. Set once at creation and never mutated (the
    /// storage layer writes it on INSERT only). See [`BranchProvenance`].
    pub branch_provenance: BranchProvenance,
    pub worktree_path: String,
}

/// Everything a standalone agent carries: a folder the user chose, and nothing
/// else. No project, no branch, no provenance, because there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderWorkspace {
    /// The directory the user pointed the agent at. dux runs the provider here
    /// and never creates, moves or removes it.
    pub folder_path: String,
}

/// Where an agent lives, and therefore what dux is allowed to do there: a
/// working copy dux created and owns, or a folder the user already had.
///
/// Faking the git fields with empty strings for the folder case would be a lie
/// that some screen eventually believes, so the two shapes are exclusive by
/// construction and every git field lives inside the managed one.
///
/// **A comment is not a guard, so the decisions live on the type.** This is the
/// same discipline [`TerminalOwner`] enforces for terminal ownership: every
/// question code asks about an agent's home is answered by a method here whose
/// body is an EXHAUSTIVE match, never by a `matches!` at the call site. A
/// `matches!` keeps compiling when a variant is added and silently answers "no"
/// for it, which is how a folder the user owns ends up in a code path written
/// for a directory dux may delete. The families of decision are:
///
/// - **Location** ([`AgentWorkspace::directory`],
///   [`AgentWorkspace::managed_worktree`]): where does this agent run, and is
///   that directory a git working copy dux owns?
/// - **Capability** ([`AgentWorkspace::supports_branch_git`]): may the
///   branch-identity git features (push, pull, fork, pull requests, branch
///   rename, provenance, the worktree manager) run against this agent at all?
/// - **Teardown** ([`AgentWorkspace::deletion_may_remove_directory`],
///   [`AgentWorkspace::dux_may_delete_branch`]): what may deleting this agent
///   remove?
/// - **Association** ([`AgentWorkspace::project_id`],
///   [`AgentWorkspace::project_path`]): which project owns this agent, if any?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentWorkspace {
    /// A working copy dux created for this agent, inside dux's managed area,
    /// on a branch dux minted, attached or adopted.
    Managed(ManagedWorkspace),
    /// A folder the user already had. dux runs the provider in it and touches
    /// nothing else, ever.
    Folder(FolderWorkspace),
}

impl AgentWorkspace {
    /// The bare kind tag, for persistence and the wire.
    pub fn kind(&self) -> AgentWorkspaceKind {
        match self {
            Self::Managed(_) => AgentWorkspaceKind::Managed,
            Self::Folder(_) => AgentWorkspaceKind::Folder,
        }
    }

    /// The directory this agent occupies: where the provider is spawned, where
    /// the editor is rooted, where uploads land. Both shapes have one, so this
    /// is what every consumer that only needs a working directory should ask
    /// for. It is NOT a promise that git can run here.
    pub fn directory(&self) -> &str {
        match self {
            Self::Managed(managed) => managed.worktree_path.as_str(),
            Self::Folder(folder) => folder.folder_path.as_str(),
        }
    }

    /// The directory as a git working copy dux owns, or `None` when it is the
    /// user's folder. Ask this, not [`Self::directory`], whenever the answer
    /// feeds a git command that assumes dux minted the checkout.
    pub fn managed_worktree(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => Some(managed.worktree_path.as_str()),
            Self::Folder(_) => None,
        }
    }

    /// The user's folder, or `None` for a managed working copy.
    pub fn folder_path(&self) -> Option<&str> {
        match self {
            Self::Managed(_) => None,
            Self::Folder(folder) => Some(folder.folder_path.as_str()),
        }
    }

    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => Some(managed.project_id.as_str()),
            // Structurally project-less, forever: a faked empty project id
            // would let the sidebar drop the row, collapse per-agent logs into
            // one directory, and let a project removal mass-delete every
            // standalone agent.
            Self::Folder(_) => None,
        }
    }

    pub fn project_path(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => managed.project_path.as_deref(),
            Self::Folder(_) => None,
        }
    }

    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => Some(managed.branch_name.as_str()),
            Self::Folder(_) => None,
        }
    }

    pub fn source_branch(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => Some(managed.source_branch.as_str()),
            Self::Folder(_) => None,
        }
    }

    pub fn initial_branch(&self) -> Option<&str> {
        match self {
            Self::Managed(managed) => Some(managed.initial_branch.as_str()),
            Self::Folder(_) => None,
        }
    }

    pub fn branch_provenance(&self) -> Option<BranchProvenance> {
        match self {
            Self::Managed(managed) => Some(managed.branch_provenance),
            // There is no provenance to consult, which is stronger than
            // "provenance says no": the question cannot be asked.
            Self::Folder(_) => None,
        }
    }

    /// Whether the branch-identity git features exist for this agent at all:
    /// push, pull, fork, pull requests, branch rename and display, provenance,
    /// the worktree manager. These are about a branch dux manages, and a
    /// standalone agent has none whatever its folder contains.
    ///
    /// This is NOT the question the changes panel asks. That one is folder
    /// driven and answered live by repository detection, because a standalone
    /// agent pointed at a repository gets a real changes panel.
    pub fn supports_branch_git(&self) -> bool {
        match self {
            Self::Managed(_) => true,
            Self::Folder(_) => false,
        }
    }

    /// Whether deleting this agent may remove the directory it occupies.
    ///
    /// The absolute rule of the standalone agent: dux never creates, moves or
    /// removes the user's folder. Deleting a standalone agent deletes only dux's
    /// own record of it.
    pub fn deletion_may_remove_directory(&self) -> bool {
        match self {
            Self::Managed(_) => true,
            Self::Folder(_) => false,
        }
    }

    /// Whether deleting this agent may delete a branch. Delegates to the
    /// provenance for a managed working copy, and is unconditionally false for
    /// a folder, which has no branch of dux's to delete.
    pub fn dux_may_delete_branch(&self) -> bool {
        match self {
            Self::Managed(managed) => managed.branch_provenance.dux_may_delete_branch(),
            Self::Folder(_) => false,
        }
    }

    /// The managed payload, for the few sites that legitimately need several
    /// git fields at once and have already established the agent is managed.
    pub fn as_managed(&self) -> Option<&ManagedWorkspace> {
        match self {
            Self::Managed(managed) => Some(managed),
            Self::Folder(_) => None,
        }
    }

    /// Mutable managed payload, for the branch-sync poller and the rename path,
    /// which are the only writers of a branch name after creation.
    pub fn as_managed_mut(&mut self) -> Option<&mut ManagedWorkspace> {
        match self {
            Self::Managed(managed) => Some(managed),
            Self::Folder(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentSession {
    pub id: String,
    /// The id of this agent's **session-slot tab**: a pointer into `agent_tabs`,
    /// stored in the session row. Read it through
    /// [`AgentSession::slot_tab_id`], never directly, so slot-ness stays one
    /// question with one answer. Every tab, the first one included, has its own
    /// `agent_tabs` row, and this names which of them currently occupies the
    /// slot. It is a generated id and is deliberately NOT the session id.
    pub slot_tab_id: String,
    pub provider: ProviderKind,
    /// Where this agent lives and what dux may do there. See [`AgentWorkspace`].
    pub workspace: AgentWorkspace,
    pub title: Option<String>,
    pub started_providers: Vec<String>,
    pub desired_running: bool,
    pub auto_reopen_enabled: bool,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The tab id the user last focused on this agent, remembered so switching
    /// away and back (either surface, and across restarts) restores it. `None`
    /// means "no memory recorded" and resolves to the session-slot tab (see
    /// [`AgentSession::slot_tab_id`]). A remembered value naming the slot tab, or
    /// one that no longer names a live extra tab, also resolves to it — see
    /// [`AgentSession::resolved_focused_tab`]. Derived runtime/UI state: kept in
    /// SQLite via a dedicated setter, never in portable config, and deliberately
    /// excluded from `upsert_session`'s hot-path SET/INSERT lists so status
    /// churn can never clobber it.
    pub last_focused_tab: Option<String>,
}

impl AgentSession {
    /// Where this agent runs. Both kinds of workspace have a directory; this
    /// one is NOT a promise that git can run in it. See
    /// [`AgentWorkspace::directory`].
    pub fn directory(&self) -> &str {
        self.workspace.directory()
    }

    /// The agent's directory as a git working copy dux owns, or `None` for a
    /// standalone agent's folder. See [`AgentWorkspace::managed_worktree`].
    pub fn managed_worktree(&self) -> Option<&str> {
        self.workspace.managed_worktree()
    }

    pub fn folder_path(&self) -> Option<&str> {
        self.workspace.folder_path()
    }

    pub fn project_id(&self) -> Option<&str> {
        self.workspace.project_id()
    }

    pub fn project_path(&self) -> Option<&str> {
        self.workspace.project_path()
    }

    pub fn branch_name(&self) -> Option<&str> {
        self.workspace.branch_name()
    }

    pub fn source_branch(&self) -> Option<&str> {
        self.workspace.source_branch()
    }

    pub fn initial_branch(&self) -> Option<&str> {
        self.workspace.initial_branch()
    }

    pub fn branch_provenance(&self) -> Option<BranchProvenance> {
        self.workspace.branch_provenance()
    }

    /// Whether the branch-identity git features exist for this agent. See
    /// [`AgentWorkspace::supports_branch_git`].
    pub fn supports_branch_git(&self) -> bool {
        self.workspace.supports_branch_git()
    }

    /// The name to show for this agent: its durable title when it has one, the
    /// branch it tracks otherwise.
    ///
    /// Creation always gives a standalone agent a title, but the folder-name
    /// fallback below is genuinely reachable: the web's rename CLEARS the title
    /// when it is submitted empty (deliberately, so a managed agent's row goes
    /// back to tracking its branch), and for a standalone agent the folder's own
    /// name is what the row falls back to. That fallback is the reason clearing
    /// a standalone title is allowed at all rather than being a way to make an
    /// agent nameless.
    pub fn display_label(&self) -> String {
        if let Some(title) = self.title.clone() {
            return title;
        }
        match &self.workspace {
            AgentWorkspace::Managed(managed) => managed.branch_name.clone(),
            AgentWorkspace::Folder(folder) => std::path::Path::new(&folder.folder_path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| folder.folder_path.clone()),
        }
    }

    pub fn has_started_provider(&self, provider: &ProviderKind) -> bool {
        self.started_providers
            .iter()
            .any(|started| started == provider.as_str())
    }

    pub fn mark_provider_started(&mut self, provider: &ProviderKind) -> bool {
        if self.has_started_provider(provider) {
            return false;
        }
        self.started_providers.push(provider.as_str().to_string());
        true
    }

    /// The id of this agent's **session-slot tab**: its first tab. Closing that
    /// tab hands the slot to the next tab in strip order, so this pointer moves;
    /// what cannot be closed is an agent's only tab.
    ///
    /// A stored pointer into `agent_tabs`, read from the
    /// [`slot_tab_id`](AgentSession::slot_tab_id) column. It is a generated tab
    /// id and never the session id, so every slot-ness decision in both crates
    /// has to come here (or to [`AgentSession::is_slot_tab`] and the `Engine`
    /// wrappers) rather than compare a tab id against `self.id` inline.
    /// The answer is a [`TabIdRef`], not a `&str`, so a caller wanting a tab id
    /// cannot reach for `session.id` instead: that does not type-check in a
    /// tab-id position. See [`crate::ids`].
    pub fn slot_tab_id(&self) -> &TabIdRef {
        TabIdRef::new(&self.slot_tab_id)
    }

    /// Whether `tab_id` names this agent's session-slot tab. See
    /// [`AgentSession::slot_tab_id`].
    pub fn is_slot_tab(&self, tab_id: &TabIdRef) -> bool {
        self.slot_tab_id() == tab_id
    }

    /// Resolve the tab to focus: the remembered tab when it is a real, still-open
    /// extra tab of this session; the session-slot tab otherwise. `None`, the
    /// slot tab's own id, and a closed/foreign tab id all resolve to the
    /// session-slot tab.
    ///
    /// `last_focused_tab` is a remembered string of unknown provenance (it comes
    /// back out of SQLite), so it is named as a tab id here, at the point where
    /// it is first compared against one.
    pub fn resolved_focused_tab<'a>(
        &'a self,
        live_extra_tab_ids: impl IntoIterator<Item = &'a TabIdRef>,
    ) -> &'a TabIdRef {
        match self.last_focused_tab.as_deref().map(TabIdRef::new) {
            Some(id)
                if !self.is_slot_tab(id) && live_extra_tab_ids.into_iter().any(|t| t == id) =>
            {
                id
            }
            _ => self.slot_tab_id(),
        }
    }
}

/// A persisted provider tab belonging to an agent session. EVERY tab is one of
/// these, the agent's first included; which of them occupies the session slot is
/// the session record's `slot_tab_id` pointer. Kept in SQLite (derived runtime
/// state), never in portable config. `sort_order` is an append-only stamp: the
/// slot tab is written at 0 and extras are appended above it. It orders the
/// EXTRAS in practice, because both surfaces put the slot tab first by following
/// the session's pointer rather than by comparing stamps. A promotion keeps the
/// promoted row's stamp exactly as it was, so the remaining extras stay in the
/// order the user has been looking at; keeping the original slot tab at the
/// bottom of the range is what makes that true.
#[derive(Clone, Debug)]
pub struct AgentTab {
    pub id: String,
    pub session_id: String,
    pub provider: ProviderKind,
    pub sort_order: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

/// Who a companion terminal belongs to. A terminal is owned by exactly one
/// owner: an agent session (spawned in that agent's worktree), a project
/// (a "project terminal", spawned at the project's repo root with no agent
/// attached), or nothing at all (a "standalone terminal", spawned in the user's
/// home directory, belonging to no project and no agent). Ownership never
/// changes after spawn.
///
/// Deliberately no bare-id accessor: every consumer must `match` so the
/// `Project` and `Standalone` variants can never be silently ignored by code
/// written for the session-owned shape. `Standalone` in particular carries NO
/// id, so any code reaching for one has to say what it does without.
///
/// **A comment is not a guard, so the decisions live on the type.** Every
/// question code asks about ownership is answered by a method here whose body is
/// an EXHAUSTIVE match, never by a `matches!` at the call site. A `matches!`
/// keeps compiling when a variant is added and silently answers "no" for it,
/// which is how a whole class of terminal could be left out of the projections,
/// the routes and the teardown paths with no error anywhere. The three families
/// of decision are:
///
/// - **Route membership** ([`TerminalOwner::is_at_route`]): may this terminal be
///   reached at a given nested REST/websocket address?
/// - **Teardown** ([`TerminalOwner::closed_by_session_delete`],
///   [`TerminalOwner::closed_by_project_removal`]): does removing that owner
///   close this terminal?
/// - **Presentation** ([`TerminalOwner::as_ref`] and
///   [`crate::viewmodel::TerminalOwnerView`]): how is the owner named to the
///   user, and what does the browser receive?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOwner {
    /// Owned by an agent session; the payload is the session id.
    Session(String),
    /// Owned by a project; the payload is the project id.
    Project(String),
    /// Owned by nothing at all: a standalone terminal, opened in the user's home
    /// directory with no project and no agent behind it. There is no payload
    /// because there is no owner to name; the row identifies it by the directory
    /// it is in ([`crate::viewmodel::TerminalView::cwd_label`]) instead.
    Standalone,
}

/// A terminal address in the REST/websocket route space: the owner id baked into
/// the path (`/api/v1/sessions/:id/terminals/...`,
/// `/ws/projects/:id/terminals/...`), or the un-nested standalone address
/// (`/api/v1/terminals/...`, `/ws/terminals/...`) which names no owner because a
/// standalone terminal has none. Paired with [`TerminalOwner::is_at_route`] so a
/// cross-owner attach/delete can only ever be decided by an exhaustive match
/// over BOTH the owner and the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRoute<'a> {
    /// The session-nested address; the payload is the `:id` from the path.
    Session(&'a str),
    /// The project-nested address; the payload is the `:id` from the path.
    Project(&'a str),
    /// The un-nested address, which carries no owner id at all.
    Standalone,
}

/// A borrowed view of a [`TerminalOwner`], for consumers that only need to name
/// the owner (rendering a row, projecting the wire shape) and must not clone.
/// Produced by the exhaustive [`TerminalOwner::as_ref`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOwnerRef<'a> {
    Session(&'a str),
    Project(&'a str),
    Standalone,
}

impl TerminalOwner {
    /// Route membership: is this terminal reachable at `route`? A session-owned
    /// terminal is a 404 on a project address and vice versa, and an id that
    /// matches the wrong owner kind never resolves.
    ///
    /// The match is over the (owner, address) PAIR with no wildcard arm, so a
    /// new owner kind fails to compile here and every route that calls this is
    /// forced through the answer rather than quietly rejecting the new kind.
    pub fn is_at_route(&self, route: TerminalRoute<'_>) -> bool {
        match (self, route) {
            (Self::Session(owner), TerminalRoute::Session(id)) => owner == id,
            (Self::Project(owner), TerminalRoute::Project(id)) => owner == id,
            // A standalone terminal lives at the un-nested address and nowhere
            // else, and no owned terminal is reachable there: the un-nested
            // address names no owner, so serving an owned terminal from it would
            // be a way around the cross-owner rejections above.
            (Self::Standalone, TerminalRoute::Standalone) => true,
            (Self::Session(_), TerminalRoute::Project(_) | TerminalRoute::Standalone)
            | (Self::Project(_), TerminalRoute::Session(_) | TerminalRoute::Standalone)
            | (Self::Standalone, TerminalRoute::Session(_) | TerminalRoute::Project(_)) => false,
        }
    }

    /// Teardown: does deleting the agent session `session_id` close this
    /// terminal? Deleting an agent tears down the terminals spawned in its
    /// worktree; a project terminal is untouched by it.
    ///
    /// A STANDALONE terminal answers `false`, and the omission is deliberate
    /// rather than an oversight to be tidied up later. It belongs to no agent,
    /// so deleting an agent has nothing to do with it: it opened in the user's
    /// home directory and it ends when the user closes it or dux shuts down.
    /// Nothing closes it automatically. The same note sits on
    /// [`Self::closed_by_project_removal`].
    pub fn closed_by_session_delete(&self, session_id: &str) -> bool {
        match self {
            Self::Session(owner) => owner == session_id,
            Self::Project(_) | Self::Standalone => false,
        }
    }

    /// Teardown: does removing the project `project_id` close this terminal?
    /// Removing a project closes the terminals opened at its repo root. A
    /// SESSION-owned terminal answers `false` here on purpose: the project
    /// removal deletes that project's agents, and each agent's own delete is
    /// what closes its terminals, so answering `true` would close them twice.
    ///
    /// A STANDALONE terminal answers `false`, and the omission is deliberate
    /// rather than an oversight to be tidied up later. It belongs to no project,
    /// so removing a project has nothing to do with it: it ends when the user
    /// closes it or dux shuts down. Nothing closes it automatically. The same
    /// note sits on [`Self::closed_by_session_delete`].
    pub fn closed_by_project_removal(&self, project_id: &str) -> bool {
        match self {
            Self::Project(owner) => owner == project_id,
            Self::Session(_) | Self::Standalone => false,
        }
    }

    /// Presentation: borrow the owner for naming/projection. Exhaustive, so a
    /// new kind must decide how it is presented before this compiles.
    #[allow(clippy::should_implement_trait)]
    pub fn as_ref(&self) -> TerminalOwnerRef<'_> {
        match self {
            Self::Session(id) => TerminalOwnerRef::Session(id),
            Self::Project(id) => TerminalOwnerRef::Project(id),
            Self::Standalone => TerminalOwnerRef::Standalone,
        }
    }
}

#[cfg(test)]
mod terminal_owner_tests {
    use super::{TerminalOwner, TerminalOwnerRef, TerminalRoute};

    #[test]
    fn session_terminal_is_only_at_its_own_session_route() {
        let owner = TerminalOwner::Session("s1".to_string());
        assert!(owner.is_at_route(TerminalRoute::Session("s1")));
        assert!(!owner.is_at_route(TerminalRoute::Session("s2")));
        // A session-owned terminal is never reachable at a project address,
        // even when the ids happen to collide.
        assert!(!owner.is_at_route(TerminalRoute::Project("s1")));
    }

    #[test]
    fn project_terminal_is_only_at_its_own_project_route() {
        let owner = TerminalOwner::Project("p1".to_string());
        assert!(owner.is_at_route(TerminalRoute::Project("p1")));
        assert!(!owner.is_at_route(TerminalRoute::Project("p2")));
        assert!(!owner.is_at_route(TerminalRoute::Session("p1")));
    }

    #[test]
    fn teardown_only_follows_the_matching_owner() {
        let session = TerminalOwner::Session("s1".to_string());
        let project = TerminalOwner::Project("p1".to_string());
        assert!(session.closed_by_session_delete("s1"));
        assert!(!session.closed_by_session_delete("s2"));
        assert!(!session.closed_by_project_removal("p1"));
        assert!(project.closed_by_project_removal("p1"));
        assert!(!project.closed_by_project_removal("p2"));
        assert!(!project.closed_by_session_delete("s1"));
    }

    #[test]
    fn standalone_terminal_is_only_at_the_un_nested_route() {
        let owner = TerminalOwner::Standalone;
        assert!(owner.is_at_route(TerminalRoute::Standalone));
        assert!(!owner.is_at_route(TerminalRoute::Session("s1")));
        assert!(!owner.is_at_route(TerminalRoute::Project("p1")));
    }

    #[test]
    fn an_owned_terminal_is_never_at_the_un_nested_route() {
        assert!(!TerminalOwner::Session("s1".to_string()).is_at_route(TerminalRoute::Standalone));
        assert!(!TerminalOwner::Project("p1".to_string()).is_at_route(TerminalRoute::Standalone));
    }

    #[test]
    fn nothing_closes_a_standalone_terminal() {
        // The user closes it, or dux shuts down. Deleting an agent and removing
        // a project both close their OWN terminals and leave this one alone.
        let owner = TerminalOwner::Standalone;
        assert!(!owner.closed_by_session_delete("s1"));
        assert!(!owner.closed_by_project_removal("p1"));
    }

    #[test]
    fn as_ref_borrows_the_owner_id() {
        assert_eq!(
            TerminalOwner::Session("s1".to_string()).as_ref(),
            TerminalOwnerRef::Session("s1")
        );
        assert_eq!(
            TerminalOwner::Project("p1".to_string()).as_ref(),
            TerminalOwnerRef::Project("p1")
        );
        assert_eq!(
            TerminalOwner::Standalone.as_ref(),
            TerminalOwnerRef::Standalone
        );
    }
}

pub struct CompanionTerminal {
    pub owner: TerminalOwner,
    pub label: String,
    pub foreground_cmd: Option<String>,
    pub client: PtyClient,
    /// The manual (drag) display position of this terminal within the flat
    /// Terminals section, ascending. Stamped at spawn from a monotonic engine
    /// counter so the default order equals creation order, and rewritten only by
    /// [`crate::engine::Engine::reorder_terminals`]. RUNTIME ONLY: terminals have
    /// no SQLite row, so this is never persisted and resets to creation order on
    /// restart.
    pub sort_order: u64,
    /// Wall-clock spawn time of this terminal. Immutable after spawn. Same type
    /// and representation as [`AgentSession::created_at`], so both surfaces can
    /// compute the same "recently created" order over terminals and agents.
    /// RUNTIME ONLY (memory-only, like the terminal itself).
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_focus(last_focused_tab: Option<&str>) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: "s1".to_string(),
            // Deliberately not the session id: the slot is a stored pointer at
            // a generated tab id, and a fixture that reused the session id
            // would let a comparison against `id` pass by coincidence.
            slot_tab_id: "slot-1".to_string(),
            provider: ProviderKind::new("claude"),
            workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
                project_id: "p1".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: "s1".to_string(),
                initial_branch: "s1".to_string(),
                branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                worktree_path: "/tmp/s1".to_string(),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: last_focused_tab.map(|s| s.to_string()),
        }
    }

    /// An unrecognized stored value is a variant this binary has never heard
    /// of, written by a newer one. It must not be told to the user as one of
    /// the variants this binary DOES know: folding it into "attached" made the
    /// delete dialog assert "existed before this agent" about a branch nobody
    /// here can say anything about.
    #[test]
    fn an_unknown_provenance_says_only_what_is_known() {
        let unknown = BranchProvenance::from_str("something-a-newer-dux-writes");
        assert_eq!(unknown, BranchProvenance::Unknown);
        assert!(
            !unknown.dux_may_delete_branch(),
            "the safe direction is losing a cleanup, never losing a branch"
        );
        let note = unknown.kept_branches_note("feature", "feature");
        assert!(
            note.contains("is not a branch dux created"),
            "the one true thing about it, got {note:?}"
        );
        assert!(
            !note.contains("existed before this agent"),
            "and nothing this binary cannot know, got {note:?}"
        );
    }

    /// The delete dialogs' checkbox starts in the provenance default, and the
    /// callers with no dialog behind them keep it.
    #[test]
    fn unanswered_branch_deletion_follows_the_provenance() {
        for provenance in [
            BranchProvenance::CreatedByDux,
            BranchProvenance::AttachedExisting,
            BranchProvenance::Adopted,
            BranchProvenance::Unknown,
        ] {
            assert_eq!(
                provenance.resolve_branch_deletion(None),
                provenance.dux_may_delete_branch(),
                "an unasked delete must behave exactly as it did before the checkbox existed, \
                 for {provenance:?}"
            );
        }
    }

    /// The checkbox is a real control in both directions: it can spare a branch
    /// dux created, and it is the only thing that removes one dux did not.
    #[test]
    fn an_answered_branch_deletion_wins_over_the_provenance() {
        assert!(!BranchProvenance::CreatedByDux.resolve_branch_deletion(Some(false)));
        assert!(BranchProvenance::CreatedByDux.resolve_branch_deletion(Some(true)));
        for provenance in [
            BranchProvenance::AttachedExisting,
            BranchProvenance::Adopted,
            BranchProvenance::Unknown,
        ] {
            assert!(
                !provenance.resolve_branch_deletion(Some(false)),
                "{provenance:?} unticked keeps the branch"
            );
            assert!(
                provenance.resolve_branch_deletion(Some(true)),
                "{provenance:?} ticked is the explicit override the dialog exists to offer"
            );
        }
    }

    /// "Not dux's branch" and "you said no" are different sentences, and the
    /// second one must be available for a branch dux DID create.
    #[test]
    fn a_declined_branch_deletion_says_so_rather_than_blaming_the_provenance() {
        let declined = BranchKeptReason::UserDeclined.kept_branches_note("feature", "feature");
        assert!(
            declined.contains("you left the branch box unticked"),
            "got {declined:?}"
        );
        assert!(
            !declined.contains("existed before this agent"),
            "a branch dux created gets no provenance excuse, got {declined:?}"
        );
        assert!(
            declined.contains("git branch -D \"feature\""),
            "the branch outlives every dux surface, so the note names the way out, got {declined:?}"
        );

        let not_duxs = BranchKeptReason::NotDuxs(BranchProvenance::AttachedExisting)
            .kept_branches_note("feature", "feature");
        assert_eq!(
            not_duxs,
            BranchProvenance::AttachedExisting.kept_branches_note("feature", "feature"),
            "the provenance sentence has one author"
        );
    }

    /// A drifted agent keeps two branches, and the declined note must name both.
    #[test]
    fn a_declined_branch_deletion_names_both_branches_after_drift() {
        let note = BranchKeptReason::UserDeclined.kept_branches_note("moved-to", "born-on");
        assert!(note.contains("moved-to"), "got {note:?}");
        assert!(note.contains("born-on"), "got {note:?}");
    }

    fn folder_session(title: Option<&str>, folder_path: &str) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: "sa1".to_string(),
            slot_tab_id: "sa1-slot".to_string(),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Folder(FolderWorkspace {
                folder_path: folder_path.to_string(),
            }),
            title: title.map(str::to_string),
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: false,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    // ── SHARED VECTORS with agentWorkspace.test.ts `sessionLabel` ──────────────
    //
    // The web has its own `sessionLabel`, and the two must name one agent the
    // same thing. The path cases below are the ones a split-on-slash gets wrong;
    // they are MEASURED here against `Path::file_name` and mirrored there.
    #[test]
    fn display_label_names_a_standalone_agent_after_its_folder() {
        // A title always wins, whatever the folder is called.
        assert_eq!(
            folder_session(Some("My notes"), "/home/someone/notes").display_label(),
            "My notes"
        );
        for (path, expected) in [
            ("/home/someone/notes", "notes"),
            // A trailing slash names the same folder.
            ("/home/someone/notes/", "notes"),
            // A trailing "." is not a name of its own.
            ("/home/someone/notes/.", "notes"),
            // A path whose last component is ".." has no name at all, so the
            // label falls back to the whole path rather than the word "..".
            ("/home/someone/notes/..", "/home/someone/notes/.."),
            // Nor has the root, nor the empty string.
            ("/", "/"),
            ("", ""),
        ] {
            assert_eq!(
                folder_session(None, path).display_label(),
                expected,
                "for {path:?}"
            );
        }
    }

    #[test]
    fn slot_tab_id_names_the_agents_first_tab() {
        // The one resolver every slot-ness decision routes through: the answer
        // is whatever the session's stored pointer names, and it is NOT the
        // session id.
        let session = session_with_focus(None);
        assert_eq!(session.slot_tab_id().as_str(), "slot-1");
        assert_ne!(session.slot_tab_id().as_str(), session.id);
    }

    #[test]
    fn is_slot_tab_accepts_only_the_slot_tab_id() {
        // TWIN of the web's `isFirstTab` cases in `lib/agentTabs.test.ts`, which
        // asks the same question of the published `slot_tab_id` field.
        let session = session_with_focus(None);
        assert!(session.is_slot_tab(session.slot_tab_id()));
        assert!(!session.is_slot_tab(TabIdRef::new("t1")));
        assert!(!session.is_slot_tab(TabIdRef::new("")));
        // The session id is not a tab id, and slot-ness must not say it is.
        assert!(!session.is_slot_tab(TabIdRef::new(&session.id)));
    }

    #[test]
    fn resolved_focused_tab_none_falls_back_to_session_slot() {
        let session = session_with_focus(None);
        assert_eq!(
            session.resolved_focused_tab([TabIdRef::new("t1")]).as_str(),
            "slot-1"
        );
    }

    #[test]
    fn resolved_focused_tab_session_id_falls_back_to_session_slot() {
        // A remembered value naming the SESSION is not a tab at all, so it
        // resolves to the slot the way any other unusable memory does.
        let session = session_with_focus(Some("s1"));
        assert_eq!(
            session.resolved_focused_tab([TabIdRef::new("t1")]).as_str(),
            "slot-1"
        );
    }

    #[test]
    fn resolved_focused_tab_gone_tab_falls_back_to_session_slot() {
        let session = session_with_focus(Some("gone"));
        assert_eq!(
            session.resolved_focused_tab([TabIdRef::new("t1")]).as_str(),
            "slot-1"
        );
    }

    #[test]
    fn resolved_focused_tab_live_extra_tab_wins() {
        let session = session_with_focus(Some("t1"));
        assert_eq!(
            session
                .resolved_focused_tab([TabIdRef::new("t1"), TabIdRef::new("t2")])
                .as_str(),
            "t1"
        );
    }

    #[test]
    fn resolved_focused_tab_empty_live_set_falls_back() {
        let session = session_with_focus(Some("t1"));
        let empty: Vec<&TabIdRef> = Vec::new();
        assert_eq!(session.resolved_focused_tab(empty).as_str(), "slot-1");
    }

    fn managed() -> AgentWorkspace {
        crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: Some("/repo".to_string()),
            source_branch: "main".to_string(),
            branch_name: "s1".to_string(),
            initial_branch: "s1".to_string(),
            branch_provenance: BranchProvenance::CreatedByDux,
            worktree_path: "/tmp/s1".to_string(),
        })
    }

    fn folder() -> AgentWorkspace {
        AgentWorkspace::Folder(FolderWorkspace {
            folder_path: "/home/someone/notes".to_string(),
        })
    }

    /// The whole point of the either/or: a standalone agent has no branch, and
    /// asking for one gets an honest "there is none" rather than an empty
    /// string some screen believes.
    #[test]
    fn a_folder_workspace_has_no_branch_identity_at_all() {
        let workspace = folder();
        assert_eq!(workspace.branch_name(), None);
        assert_eq!(workspace.source_branch(), None);
        assert_eq!(workspace.initial_branch(), None);
        assert_eq!(workspace.branch_provenance(), None);
        assert_eq!(workspace.project_id(), None);
        assert_eq!(workspace.project_path(), None);
        assert!(!workspace.supports_branch_git());
    }

    #[test]
    fn a_managed_workspace_answers_every_git_question() {
        let workspace = managed();
        assert_eq!(workspace.branch_name(), Some("s1"));
        assert_eq!(workspace.source_branch(), Some("main"));
        assert_eq!(workspace.initial_branch(), Some("s1"));
        assert_eq!(
            workspace.branch_provenance(),
            Some(BranchProvenance::CreatedByDux)
        );
        assert_eq!(workspace.project_id(), Some("p1"));
        assert_eq!(workspace.project_path(), Some("/repo"));
        assert!(workspace.supports_branch_git());
    }

    /// Both variants occupy a directory, and every consumer that only needs
    /// "where do I run" gets it without asking which kind this is.
    #[test]
    fn both_workspaces_name_the_directory_the_agent_occupies() {
        assert_eq!(managed().directory(), "/tmp/s1");
        assert_eq!(folder().directory(), "/home/someone/notes");
    }

    /// Nothing deletes the folder. This is the pin the whole feature rests on.
    #[test]
    fn deleting_a_standalone_agent_may_never_remove_its_folder() {
        assert!(
            !folder().deletion_may_remove_directory(),
            "the folder is the user's, and dux never created it"
        );
        assert!(
            managed().deletion_may_remove_directory(),
            "a managed worktree is dux's own and is removed as always"
        );
    }

    /// A folder workspace has no provenance to consult, so branch deletion is
    /// not merely refused, it is unaskable.
    #[test]
    fn a_folder_workspace_never_lets_dux_delete_a_branch() {
        assert!(!folder().dux_may_delete_branch());
        assert!(managed().dux_may_delete_branch());
    }

    #[test]
    fn the_workspace_kind_round_trips_through_its_stored_text() {
        assert_eq!(
            AgentWorkspaceKind::from_str(AgentWorkspaceKind::Managed.as_str()),
            AgentWorkspaceKind::Managed
        );
        assert_eq!(
            AgentWorkspaceKind::from_str(AgentWorkspaceKind::Folder.as_str()),
            AgentWorkspaceKind::Folder
        );
    }

    /// An unrecognized kind is a variant a newer dux wrote. Reading it as a
    /// folder would hand the user's directory to code that believes it may
    /// delete it, so the safe guess is the shape whose git fields are all
    /// present in the row already.
    #[test]
    fn an_unknown_workspace_kind_reads_as_managed() {
        assert_eq!(
            AgentWorkspaceKind::from_str("something-a-newer-dux-writes"),
            AgentWorkspaceKind::Managed
        );
    }
}
