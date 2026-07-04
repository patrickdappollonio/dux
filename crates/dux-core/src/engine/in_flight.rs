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

/// The expected branch names for an in-flight intentional rename, stashed in
/// `Engine::rename_expected` so `BranchSyncReady` can distinguish the user's
/// own in-progress rename (silently skip) from an unrelated external change
/// that happened to land mid-rename (log it).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenameExpectation {
    /// The branch the worktree was on before the rename dispatched. A
    /// `BranchSyncReady` still reporting this value means the rename has not
    /// landed yet — expected, so skip quietly.
    pub old_branch: String,
    /// The branch `git branch -m` is moving to. A `BranchSyncReady` reporting
    /// this value is the rename completing — expected, so skip quietly.
    pub new_branch: String,
}

impl RenameExpectation {
    /// True when `branch` is one of the two values expected while this rename
    /// is in flight (the still-pending old name or the target new name).
    pub fn matches(&self, branch: &str) -> bool {
        branch == self.old_branch || branch == self.new_branch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_expectation_matches_only_old_and_new() {
        // The pure expected-vs-unexpected decision the branch-sync guard keys on:
        // the still-pending old name and the target new name are expected; any
        // other observed branch is unexpected (and gets logged/deferred).
        let exp = RenameExpectation {
            old_branch: "old-branch".to_string(),
            new_branch: "new-branch".to_string(),
        };
        assert!(
            exp.matches("old-branch"),
            "still-pending old name is expected"
        );
        assert!(exp.matches("new-branch"), "target new name is expected");
        assert!(
            !exp.matches("surprise-branch"),
            "an unrelated branch is unexpected"
        );
        assert!(!exp.matches(""), "empty is unexpected");
    }
}
