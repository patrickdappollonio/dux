//! Backend/concurrency state grouped from the App god-object (audit02 P1-V).
//!
//! These fields are all "runtime plumbing": worker channels, the PTY map, the
//! single-instance lockfile, OS-level atomics, and the GitHub/PR tracking
//! caches. They are accessed from worker callbacks and the main loop, but are
//! only incidentally touched by rendering code.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::lockfile::SingleInstanceLock;
use crate::model::ProviderKind;

use super::super::{BranchSyncEntry, CompanionTerminal, PrSyncEntry, WorkerEvent};

pub(crate) struct RuntimeState {
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    // audit02 P1-Z phase 2 (Phase 18): the legacy `providers:
    // HashMap<String, PtyClient>` field is gone. PTY ownership now
    // lives inside `SessionState::Live` / `SessionState::Detached` on
    // each `AgentSession`. Look up handles via `App::find_pty_handle`.
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub(crate) running_provider_pins: HashMap<String, ProviderKind>,
    pub(crate) companion_terminals: HashMap<String, CompanionTerminal>,
    pub(crate) pulls_in_flight: HashSet<String>,
    pub(crate) watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub(crate) has_active_processes: Arc<AtomicBool>,
    pub(crate) sigwinch_flag: Arc<AtomicBool>,
    pub(crate) branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub(crate) gh_status: crate::model::GhStatus,
    pub(crate) github_integration_enabled: bool,
    pub(crate) pr_statuses: HashMap<String, crate::model::PrInfo>,
    pub(crate) pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub(crate) pr_sync_enabled: Arc<AtomicBool>,
    /// Timestamps of the last PR check per session, to avoid hammering on rapid
    /// state transitions.
    pub(crate) pr_last_checked: HashMap<String, Instant>,
    /// File-system watcher for `.git/refs/heads/` directories. `None` if the
    /// watcher could not be created (graceful fallback to poll-only).
    pub(crate) refs_watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Maps watched worktree paths back to session IDs so the refs watcher
    /// can route change events.
    pub(crate) refs_watch_paths: HashMap<PathBuf, String>,
    /// Exclusive lock held for the lifetime of this `App` so only one dux
    /// instance runs against a given config directory. Released
    /// automatically on drop (including crashes), so there is nothing to
    /// clean up on exit.
    pub(crate) _single_instance_lock: SingleInstanceLock,
    /// Per-session watch-rule engines. Attached when a session
    /// transitions into `SessionState::Live`, removed on exit. Sessions
    /// whose provider has no rules in the config never get an entry —
    /// the per-tick scan is then skipped entirely. See `crate::watch`.
    pub(crate) watch_engines: HashMap<String, crate::watch::WatchEngine>,
}
