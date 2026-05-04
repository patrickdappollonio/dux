use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    pub owner_repo: String,
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

    #[allow(clippy::should_implement_trait)] // existing API; FromStr trait migration tracked separately
    pub fn from_str(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_provider: ProviderKind,
    pub current_branch: String,
    pub path_missing: bool,
    /// `false` while metadata (is_git_repo, current_branch, remote default)
    /// is still being resolved on a worker thread. Render code must show a
    /// "(loading…)" placeholder for any field whose value depends on git
    /// until this flips to `true`. See
    /// [`crate::app::workers::dispatch_project_meta`].
    pub meta_loaded: bool,
}

impl Project {
    /// Construct a half-populated `Project` whose git metadata
    /// (`current_branch`, `path_missing`) is filled in later via a
    /// `WorkerEvent::ProjectMetaReady`. Render code must check
    /// [`Project::meta_loaded`] before displaying git-derived fields.
    pub fn placeholder(
        id: String,
        name: String,
        path: String,
        default_provider: ProviderKind,
    ) -> Self {
        Self {
            id,
            name,
            path,
            default_provider,
            current_branch: String::new(),
            path_missing: false,
            meta_loaded: false,
        }
    }
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

    #[allow(clippy::should_implement_trait)] // existing API; FromStr trait migration tracked separately
    pub fn from_str(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "exited" => Self::Exited,
            _ => Self::Detached,
        }
    }
}

/// Explicit per-session lifecycle state for audit02 P1-Z (Phase 18).
///
/// This enum is the long-term replacement for [`SessionStatus`]. It is
/// being introduced **alongside** the older string-tagged status as
/// "phase 1 of 2" — phase 2 will retire `SessionStatus` and fold the
/// PTY handle into the `Live` / `Detached` variants (typestate). For
/// now `SessionState` is purely persistable metadata: timestamps and
/// exit codes, no PTY references. That keeps the schema changes here
/// minimal and back-compat-friendly while still giving us a single
/// gate for legal transitions.
///
/// The variants intentionally mirror Phase 18's plan:
///
/// - `Created` — row exists, no spawn attempt yet.
/// - `Spawning` — spawn job dispatched to a worker.
/// - `Live` — PTY accepting input; the user is interacting.
/// - `Detached` — PTY still alive but no UI pane attached.
/// - `Exited` — child terminated; no PTY.
///
/// Persistence note: `Live` is **never** persisted as `Live` — when a
/// session is reloaded from disk on the next dux start there cannot,
/// by definition, be a running PTY for it yet. The `From<SessionState>
/// for PersistedSessionState` impl folds `Live` into `Detached` for
/// storage so the round-trip is faithful.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionState {
    Created {
        created_at: DateTime<Utc>,
    },
    Spawning {
        since: DateTime<Utc>,
    },
    Live {
        spawned_at: DateTime<Utc>,
        last_active_at: DateTime<Utc>,
    },
    Detached {
        detached_at: DateTime<Utc>,
    },
    Exited {
        exit_code: Option<i32>,
        exited_at: DateTime<Utc>,
    },
}

// Phase 18 ships the typed transition functions ahead of the
// runtime call sites that will use them — they are exercised by
// `tests/session_state.rs` (integration target, separate crate from
// the `dux` bin) and will be wired into `RuntimeState` in phase 2.
// Until then `cargo build --bin dux` sees them as unused.
#[allow(dead_code)]
impl SessionState {
    /// Short tag used in error messages and the legacy
    /// [`SessionStatus`] mapping. The string values match the targets
    /// accepted by [`SessionState::transition`] so that
    /// `state.transition(other.name())` is meaningful when both states
    /// are known.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Created { .. } => "created",
            Self::Spawning { .. } => "spawning",
            Self::Live { .. } => "live",
            Self::Detached { .. } => "detached",
            Self::Exited { .. } => "exited",
        }
    }

    /// Returns `true` if `target` is a legal next state from `self`.
    ///
    /// The legal transitions are deliberately narrow:
    ///
    /// - `Created -> Spawning`
    /// - `Spawning -> Live | Exited` (success or spawn failure)
    /// - `Live -> Detached | Exited`
    /// - `Detached -> Live | Exited` (reattach or child exit while detached)
    /// - `Exited -> Spawning` (re-spawn after exit)
    ///
    /// Anything else — including `Self -> Self` — is rejected. Targets
    /// are matched by string tag to keep the API simple for phase 1;
    /// phase 2 will replace this with a proper typestate.
    pub fn can_transition_to(&self, target: &str) -> bool {
        matches!(
            (self, target),
            (Self::Created { .. }, "spawning")
                | (Self::Spawning { .. }, "live")
                | (Self::Spawning { .. }, "exited")
                | (Self::Live { .. }, "detached")
                | (Self::Live { .. }, "exited")
                | (Self::Detached { .. }, "live")
                | (Self::Detached { .. }, "exited")
                | (Self::Exited { .. }, "spawning")
        )
    }

    /// Apply a transition, consuming `self` and returning the new
    /// state. Fails with a descriptive error if the transition is not
    /// legal — callers should treat that as a programming bug, not a
    /// recoverable runtime condition.
    ///
    /// `now` is the wall-clock timestamp to stamp on the resulting
    /// state. Threading it through (rather than calling `Utc::now()`
    /// inside) keeps the function pure and tests deterministic.
    pub fn transition(self, target: &str, now: DateTime<Utc>) -> Result<SessionState> {
        if !self.can_transition_to(target) {
            return Err(anyhow!(
                "illegal session-state transition: {} -> {}",
                self.name(),
                target
            ));
        }
        let next = match (self, target) {
            (Self::Created { .. } | Self::Exited { .. }, "spawning") => {
                Self::Spawning { since: now }
            }
            (Self::Spawning { .. } | Self::Detached { .. }, "live") => Self::Live {
                spawned_at: now,
                last_active_at: now,
            },
            (Self::Live { .. }, "detached") => Self::Detached { detached_at: now },
            (_, "exited") => Self::Exited {
                exit_code: None,
                exited_at: now,
            },
            // can_transition_to already ruled out other shapes, so this
            // arm is unreachable in practice.
            (state, target) => {
                return Err(anyhow!(
                    "internal: missing transition handler {} -> {}",
                    state.name(),
                    target
                ));
            }
        };
        Ok(next)
    }

    /// Map a legacy [`SessionStatus`] string tag onto an initial
    /// [`SessionState`]. `Active` is treated as `Detached` because we
    /// have no live PTY at load time — see the persistence note on the
    /// enum doc-comment. `now` provides the timestamp used for the
    /// reconstructed state's "since" / "exited_at" fields when no
    /// better value is available from the row.
    pub fn from_legacy_status(status: &SessionStatus, now: DateTime<Utc>) -> Self {
        match status {
            SessionStatus::Active | SessionStatus::Detached => Self::Detached { detached_at: now },
            SessionStatus::Exited => Self::Exited {
                exit_code: None,
                exited_at: now,
            },
        }
    }

    /// Convenience for the storage layer: serialize to JSON for the
    /// new `state_json` column. Folds `Live` into `Detached` because a
    /// running PTY cannot be represented across process restarts.
    pub fn to_json(&self) -> Result<String> {
        let persisted: PersistedSessionState = self.clone().into();
        serde_json::to_string(&persisted)
            .map_err(|e| anyhow!("failed to serialize SessionState: {e}"))
    }

    /// Inverse of [`SessionState::to_json`].
    pub fn from_json(json: &str) -> Result<Self> {
        let persisted: PersistedSessionState = serde_json::from_str(json)
            .map_err(|e| anyhow!("failed to parse SessionState JSON: {e}"))?;
        Ok(persisted.into())
    }
}

/// Wire format used by the `agent_sessions.state_json` column. The
/// `Live` variant is intentionally absent — a "live" session by
/// definition has a running PTY in this process, and that handle
/// cannot survive a restart. Persisting `Live` would lie about the
/// invariant, so we collapse it to `Detached` on the way out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedSessionState {
    Created {
        created_at: DateTime<Utc>,
    },
    Spawning {
        since: DateTime<Utc>,
    },
    Detached {
        detached_at: DateTime<Utc>,
    },
    Exited {
        exit_code: Option<i32>,
        exited_at: DateTime<Utc>,
    },
}

impl From<SessionState> for PersistedSessionState {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Created { created_at } => Self::Created { created_at },
            SessionState::Spawning { since } => Self::Spawning { since },
            // Live folds into Detached on persist — see enum doc.
            SessionState::Live { last_active_at, .. } => Self::Detached {
                detached_at: last_active_at,
            },
            SessionState::Detached { detached_at } => Self::Detached { detached_at },
            SessionState::Exited {
                exit_code,
                exited_at,
            } => Self::Exited {
                exit_code,
                exited_at,
            },
        }
    }
}

impl From<PersistedSessionState> for SessionState {
    fn from(persisted: PersistedSessionState) -> Self {
        match persisted {
            PersistedSessionState::Created { created_at } => Self::Created { created_at },
            PersistedSessionState::Spawning { since } => Self::Spawning { since },
            PersistedSessionState::Detached { detached_at } => Self::Detached { detached_at },
            PersistedSessionState::Exited {
                exit_code,
                exited_at,
            } => Self::Exited {
                exit_code,
                exited_at,
            },
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
