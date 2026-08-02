use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::pty::PtyClient;

/// GitHub CLI availability status, checked once at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhStatus {
    /// Not yet checked.
    #[default]
    Unknown,
    /// `gh` binary not found on PATH.
    NotInstalled,
    /// `gh` found but `gh auth status` failed.
    NotAuthenticated,
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

#[derive(Clone, Debug)]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub project_path: Option<String>,
    pub provider: ProviderKind,
    pub source_branch: String,
    pub branch_name: String,
    /// The branch this agent was created on. Immutable identity: unlike
    /// `branch_name` (which the branch-sync poller and intentional renames keep
    /// in step with the worktree's actual current branch), `initial_branch` is
    /// set once at creation and never mutated afterward. Surfaced so the UI can
    /// show branch lineage and flag drift when `branch_name != initial_branch`.
    pub initial_branch: String,
    pub worktree_path: String,
    pub title: Option<String>,
    pub started_providers: Vec<String>,
    pub desired_running: bool,
    pub auto_reopen_enabled: bool,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The tab id the user last focused on this agent, remembered so switching
    /// away and back (either surface, and across restarts) restores it. `None`
    /// means "no memory recorded" and resolves to the session-slot tab (id ==
    /// `self.id`). A remembered value equal to `self.id`, or one that no longer
    /// names a live extra tab, also resolves to the session-slot tab — see
    /// [`AgentSession::resolved_focused_tab`]. Derived runtime/UI state: kept in
    /// SQLite via a dedicated setter, never in portable config, and deliberately
    /// excluded from `upsert_session`'s hot-path SET/INSERT lists so status
    /// churn can never clobber it.
    pub last_focused_tab: Option<String>,
}

impl AgentSession {
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

    /// Resolve the tab to focus: the remembered tab when it is a real, still-open
    /// extra tab of this session; the session-slot tab (== session id) otherwise.
    /// `None`, the session id itself, and a closed/foreign tab id all resolve to
    /// the session-slot tab.
    pub fn resolved_focused_tab<'a>(
        &'a self,
        live_extra_tab_ids: impl IntoIterator<Item = &'a str>,
    ) -> &'a str {
        match self.last_focused_tab.as_deref() {
            Some(id) if id != self.id && live_extra_tab_ids.into_iter().any(|t| t == id) => id,
            _ => &self.id,
        }
    }
}

/// A persisted **extra tab** (a secondary provider tab) belonging to an agent
/// session. The session-slot tab is synthesized from the `AgentSession` row and has no
/// `AgentTab`; only tabs 2..N are stored here. Kept in SQLite (derived runtime
/// state), never in portable config. `sort_order` is an append-only stamp that
/// fixes creation order (Main renders first, then these by `sort_order`).
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
/// owner: an agent session (spawned in that agent's worktree) or a project
/// (a "project terminal", spawned at the project's repo root with no agent
/// attached). Ownership never changes after spawn.
///
/// Deliberately no bare-id accessor: every consumer must `match` so the
/// `Project` variant can never be silently ignored by code written for the
/// session-owned shape.
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
}

/// A nested terminal address in the REST/websocket route space: the owner id
/// baked into the path (`/api/v1/sessions/:id/terminals/...`,
/// `/ws/projects/:id/terminals/...`). Paired with
/// [`TerminalOwner::is_at_route`] so a cross-owner attach/delete can only ever
/// be decided by an exhaustive match over BOTH the owner and the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRoute<'a> {
    /// The session-nested address; the payload is the `:id` from the path.
    Session(&'a str),
    /// The project-nested address; the payload is the `:id` from the path.
    Project(&'a str),
}

/// A borrowed view of a [`TerminalOwner`], for consumers that only need to name
/// the owner (rendering a row, projecting the wire shape) and must not clone.
/// Produced by the exhaustive [`TerminalOwner::as_ref`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOwnerRef<'a> {
    Session(&'a str),
    Project(&'a str),
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
            (Self::Session(_), TerminalRoute::Project(_))
            | (Self::Project(_), TerminalRoute::Session(_)) => false,
        }
    }

    /// Teardown: does deleting the agent session `session_id` close this
    /// terminal? Deleting an agent tears down the terminals spawned in its
    /// worktree; a project terminal is untouched by it.
    pub fn closed_by_session_delete(&self, session_id: &str) -> bool {
        match self {
            Self::Session(owner) => owner == session_id,
            Self::Project(_) => false,
        }
    }

    /// Teardown: does removing the project `project_id` close this terminal?
    /// Removing a project closes the terminals opened at its repo root. A
    /// SESSION-owned terminal answers `false` here on purpose: the project
    /// removal deletes that project's agents, and each agent's own delete is
    /// what closes its terminals, so answering `true` would close them twice.
    pub fn closed_by_project_removal(&self, project_id: &str) -> bool {
        match self {
            Self::Project(owner) => owner == project_id,
            Self::Session(_) => false,
        }
    }

    /// Presentation: borrow the owner for naming/projection. Exhaustive, so a
    /// new kind must decide how it is presented before this compiles.
    #[allow(clippy::should_implement_trait)]
    pub fn as_ref(&self) -> TerminalOwnerRef<'_> {
        match self {
            Self::Session(id) => TerminalOwnerRef::Session(id),
            Self::Project(id) => TerminalOwnerRef::Project(id),
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
    fn as_ref_borrows_the_owner_id() {
        assert_eq!(
            TerminalOwner::Session("s1".to_string()).as_ref(),
            TerminalOwnerRef::Session("s1")
        );
        assert_eq!(
            TerminalOwner::Project("p1".to_string()).as_ref(),
            TerminalOwnerRef::Project("p1")
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
            project_id: "p1".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "s1".to_string(),
            initial_branch: "s1".to_string(),
            worktree_path: "/tmp/s1".to_string(),
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

    #[test]
    fn resolved_focused_tab_none_falls_back_to_session_slot() {
        let session = session_with_focus(None);
        assert_eq!(session.resolved_focused_tab(["t1"]), "s1");
    }

    #[test]
    fn resolved_focused_tab_session_id_falls_back_to_session_slot() {
        let session = session_with_focus(Some("s1"));
        assert_eq!(session.resolved_focused_tab(["t1"]), "s1");
    }

    #[test]
    fn resolved_focused_tab_gone_tab_falls_back_to_session_slot() {
        let session = session_with_focus(Some("gone"));
        assert_eq!(session.resolved_focused_tab(["t1"]), "s1");
    }

    #[test]
    fn resolved_focused_tab_live_extra_tab_wins() {
        let session = session_with_focus(Some("t1"));
        assert_eq!(session.resolved_focused_tab(["t1", "t2"]), "t1");
    }

    #[test]
    fn resolved_focused_tab_empty_live_set_falls_back() {
        let session = session_with_focus(Some("t1"));
        let empty: Vec<&str> = Vec::new();
        assert_eq!(session.resolved_focused_tab(empty), "s1");
    }
}
