use std::collections::HashSet;

/// Typed key into the `Engine::in_flight` set. Every command or worker
/// that needs single-instance semantics inserts one of these variants.
///
/// Reasons not to add a variant here: the field is a rate-limit (use a
/// `HashMap<Key, Instant>` instead) or a kill-switch (use an
/// `AtomicBool`). The `pr_last_checked` map and `pr_sync_enabled` flag
/// are deliberately NOT migrated here for exactly that reason.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InFlightKey {
    CreateAgent,
    AgentLaunch(String),
    /// An intentional git branch rename is in flight for this session id (the
    /// worker running `git::rename_branch` that later posts
    /// `BranchRenameCompleted`). While set, the branch-sync poller must NOT
    /// classify this session's in-progress rename as external drift.
    BranchRename(String),
    Pull(String),
    ResourceStats,
    /// Creating an initial commit for the repo at this path, then registering
    /// it. Keyed by canonical path so two concurrent "create initial commit &
    /// add" requests for the same repo can't both run and append two commits.
    InitialCommit(String),
}

/// Convenience alias so call sites can spell the storage shape once.
pub type InFlightSet = HashSet<InFlightKey>;
