use crate::ids::TabId;
use std::collections::HashSet;

/// Typed key into the `Engine::in_flight` set. Every command or worker
/// that needs single-instance semantics inserts one of these variants.
///
/// Reasons not to add a variant here: the field is a rate-limit (use a
/// `HashMap<Key, Instant>` instead) or a kill-switch (use an
/// `AtomicBool`). The `pr_last_checked` map and `pr_sync` control
/// are deliberately NOT migrated here for exactly that reason.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InFlightKey {
    CreateAgent,
    /// A provider launch is in flight for this TAB. Keyed by tab id, never by
    /// session id: `Engine::tab_resume_decision` reads this key to decide whether
    /// a launching sibling already owns a provider's conversation, and it asks
    /// per tab. The type says so, because as bare strings the two keyspaces are
    /// interchangeable by accident (see [`crate::ids`]).
    AgentLaunch(TabId),
    /// An intentional git branch rename is in flight for this session id (the
    /// worker running `git::rename_branch` that later posts
    /// `BranchRenameCompleted`). While set, the branch-sync poller must NOT
    /// classify this session's in-progress rename as external drift.
    BranchRename(String),
    Pull(String),
    /// A standalone agent's folder is being classified by
    /// `Engine::spawn_folder_repo_probe`. Bounds the probe to one at a time per
    /// agent: `repo_path_kind` runs up to four git subprocesses, and every
    /// question about the folder asks for a refresh, including the web's
    /// changed-files poller, which asks every two seconds. Without this the
    /// probe was an unbounded thread-and-subprocess loop for as long as a
    /// standalone agent's changes panel stayed open. Cleared by the
    /// `FolderRepoStatusReady` handler.
    FolderRepoProbe(String),
    ResourceStats,
    /// A one-shot PR check (foreground/refs-watcher/exit trigger) is running for
    /// this session id. Bounds concurrent `gh` subprocesses for one session (a
    /// call can run up to `GH_CALL_TIMEOUT`, longer than the debounce). Cleared
    /// by the `PrStatusReady`/`PrCheckAborted` handlers.
    PrCheck(String),
    /// A manual pull-request attach is resolving for this session id: the one
    /// keyed op that spans the `gh` lookup and the attach that follows it.
    /// While set, this session's other pull-request operations (detach, resume
    /// autodetection, and a second attach) are refused rather than allowed to
    /// race the attach's own writes. Marked in
    /// `Engine::dispatch_attach_pull_request` after validation (the
    /// `BranchRename` precedent) and cleared in the `PullRequestResolved`
    /// attach arm, keyed on the purpose's session id so the path where the
    /// keyed op has gone missing clears it too.
    ///
    /// Liveness, and why there is no timed expiry: every dispatch terminates
    /// in exactly one `PullRequestResolved` for the session. The lookup worker
    /// posts one, and it runs inside `catch_unwind` with a sender held outside
    /// it, so even a panicking job posts a failed resolution. Success,
    /// failure, a session deleted mid-lookup, and a panic therefore all reach
    /// the same clear point.
    PrAttach(String),
    /// Creating an initial commit for the repo at this path, then registering
    /// it. Keyed by canonical path so two concurrent "create initial commit &
    /// add" requests for the same repo can't both run and append two commits.
    /// The adopt-a-folder flow (`git init` + seed + commit) shares this key:
    /// same hazard class, and sharing makes init-and-commit and commit-only on
    /// the same path mutually exclusive for free.
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

/// Why `Engine::prepare_branch_rename` refused to start a rename. The engine
/// mutated nothing for any of these; the surface owns the user-facing copy so
/// each variant maps to that surface's own error string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchRenameRejection {
    /// The requested name was empty (after trimming).
    EmptyName,
    /// The requested name failed `git::is_valid_agent_name`.
    MalformedName,
    /// A branch rename is already in flight for this session.
    AlreadyInFlight,
}

/// The parameters a surface needs to dispatch the git branch-rename worker and
/// to unwind on a spawn failure. Produced by `Engine::prepare_branch_rename`
/// once it has written the optimistic title and stashed the expectation; the
/// surface runs `git::rename_branch(worktree_path, old_branch, new_branch)` in
/// its own background worker and, on a synchronous spawn failure, rolls back
/// through `Engine::revert_optimistic_rename(session_id, previous_title)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchRenameDispatch {
    /// The session whose branch is being renamed.
    pub session_id: String,
    /// The worktree the `git branch -m` runs in.
    pub worktree_path: String,
    /// The branch name before the rename (the `git branch -m` source).
    pub old_branch: String,
    /// The branch name to move to (the `git branch -m` target).
    pub new_branch: String,
    /// The title the session carried before the optimistic write, so the
    /// surface (or the completion/unwind handlers) can restore it on failure.
    pub previous_title: Option<String>,
}

/// The decision + side effects computed by `Engine::prepare_branch_rename`.
/// The engine owns the semantics here (name validation, the overlap guard, the
/// optimistic title write, no-op detection, and the expectation stash); the
/// surface reads this value to drive its own status/worker/UI wiring. This is
/// the single core-owned rename primitive both the TUI and a future web rename
/// consume, so the decision cannot drift between surfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchRenamePlan {
    /// The rename was refused before any state change; surface the matching
    /// error and stop.
    Rejected(BranchRenameRejection),
    /// The display title was updated and no git branch rename is required
    /// (either the caller asked for title-only, or the new name already matches
    /// the current branch). `sync_branches` is true only for the deliberate
    /// title-only path (`rename_branch == false`), which also refreshes
    /// branch-sync state; it is false for the name-equals-branch no-op.
    TitleWritten { name: String, sync_branches: bool },
    /// The display title was updated and the expectation stashed; the surface
    /// must dispatch the git rename worker with these parameters.
    RenameBranch(BranchRenameDispatch),
    /// The target session vanished before the rename could be prepared (the
    /// optimistic title write also found nothing). The surface stays silent,
    /// matching the pre-extraction early return.
    Noop,
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
