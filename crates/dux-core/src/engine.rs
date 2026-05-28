//! The headless `Engine`: the single owner of dux's domain state. Surfaces (the
//! TUI `App` today, the web server later) embed/drive it. In E2 it is a passive
//! state container; domain operations and workers move into `Engine` methods in E3.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::{Config, DuxPaths};
use crate::lockfile::SingleInstanceLock;
use crate::model::{
    AgentSession, ChangedFile, CompanionTerminal, GhStatus, PrInfo, Project, ProviderKind,
};
use crate::pty::PtyClient;
use crate::storage::SessionStore;
use crate::worker::{BranchSyncEntry, PrSyncEntry, WorkerEvent};

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

    // Batch B fields
    pub worker_tx: Sender<WorkerEvent>,
    pub worker_rx: Receiver<WorkerEvent>,
    pub providers: HashMap<String, PtyClient>,
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub running_provider_pins: HashMap<String, ProviderKind>,
    pub companion_terminals: HashMap<String, CompanionTerminal>,
    pub gh_status: GhStatus,
    pub pr_statuses: HashMap<String, PrInfo>,
    pub branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub pr_sync_enabled: Arc<AtomicBool>,
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
    /// Maps session IDs to the exact Busy message set by
    /// `begin_delete_session`. Used by the worker event handler to decide
    /// whether the current status-line content was set by this deletion (and
    /// should be cleared) or by an unrelated operation (and should be left
    /// alone). Cleared per-session when the worker event arrives.
    pub deletion_busy_messages: HashMap<String, String>,
    pub watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub has_active_processes: Arc<AtomicBool>,
    pub create_agent_in_flight: bool,
    pub agent_launches_in_flight: HashSet<String>,
    pub pulls_in_flight: HashSet<String>,
    pub resource_stats_in_flight: bool,
}
