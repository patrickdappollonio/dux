use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectBranchStatus {
    Leading,
    NotLeading,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub worktree_path: String,
    pub title: Option<String>,
    pub started_providers: Vec<String>,
    pub desired_running: bool,
    pub auto_reopen_enabled: bool,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
}

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}
