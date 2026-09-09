use crate::ids::TabId;
use std::collections::HashSet;

/// Typed key into the `Engine::in_flight` set. Every command or worker
/// that needs single-instance semantics inserts one of these variants.
///
/// Not for rate limits, which need a `HashMap<Key, Instant>` as
/// `Engine::pr_last_checked` has, nor for arm/disarm switches, which need a
/// lock of their own as the `pr_sync_control` module has.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InFlightKey {
    CreateAgent,
    /// A provider launch is in flight for this tab. Keyed by tab id, never by
    /// session id: `Engine::tab_resume_decision` asks per tab whether a
    /// launching sibling already owns a provider's conversation. The id is
    /// typed because as bare strings the two keyspaces are interchangeable by
    /// accident (see [`crate::ids`]).
    AgentLaunch(TabId),
    /// An intentional git branch rename is in flight for this session id (the
    /// worker running `git::rename_branch` that later posts
    /// `BranchRenameCompleted`). While set, the branch-sync poller must NOT
    /// classify this session's in-progress rename as external drift.
    BranchRename(String),
    Pull(String),
    /// A standalone agent's folder is being classified by
    /// `Engine::spawn_folder_repo_probe`. Bounds the probe to one at a time per
    /// agent: `git::repo_path_kind` runs several git subprocesses and every
    /// question about the folder asks for a refresh, the web's changed-files
    /// poller included. Cleared by the `FolderRepoStatusReady` handler.
    FolderRepoProbe(String),
    ResourceStats,
    /// A one-shot PR check (foreground/refs-watcher/exit trigger) is running for
    /// this session id. Bounds concurrent `gh` subprocesses for one session (a
    /// call can run up to `GH_CALL_TIMEOUT`, longer than the debounce). Cleared
    /// by the `PrStatusReady`/`PrCheckAborted` handlers.
    PrCheck(String),
    /// A manual pull-request attach is resolving for this session id, spanning
    /// the `gh` lookup and the attach that follows it. While set, this
    /// session's other pull-request operations (detach, resume autodetection, a
    /// second attach) are refused rather than left to race the attach's own
    /// writes. Marked in `Engine::dispatch_attach_pull_request` after
    /// validation and cleared in the `PullRequestResolved` attach arm, keyed on
    /// the purpose's session id so the path where the keyed op has gone missing
    /// clears it too.
    ///
    /// No timed expiry is needed: every dispatch terminates in exactly one
    /// `PullRequestResolved` for the session, because the lookup worker runs
    /// inside `catch_unwind` with the sender held outside it, so a panicking
    /// job still posts a failed resolution.
    PrAttach(String),
    /// Creating an initial commit for the repo at this path, then registering
    /// it. Keyed by canonical path so two concurrent requests for the same repo
    /// cannot both run and append two commits. The adopt-a-folder flow shares
    /// this key, which also makes init-and-commit and commit-only on one path
    /// mutually exclusive.
    InitialCommit(String),
}

/// Convenience alias so call sites can spell the storage shape once.
pub type InFlightSet = HashSet<InFlightKey>;

/// The expected branch names for an in-flight intentional rename, stashed in
/// `Engine::rename_expected` so `BranchSyncReady` tells the user's own rename
/// (skip silently) from unrelated external drift landing mid-rename (log it).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenameExpectation {
    /// The branch the worktree was on before the rename dispatched. A
    /// `BranchSyncReady` still reporting it means the rename has not landed
    /// yet, which is expected, so skip quietly.
    pub old_branch: String,
    /// The branch `git branch -m` is moving to. A `BranchSyncReady` reporting
    /// it is the rename completing, which is expected, so skip quietly.
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
/// to unwind on a spawn failure. The surface runs
/// `git::rename_branch(worktree_path, old_branch, new_branch)` in its own
/// background worker and, on a synchronous spawn failure, rolls back through
/// `Engine::revert_optimistic_rename(session_id, previous_title)`.
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

/// The decision and side effects computed by `Engine::prepare_branch_rename`.
/// The engine owns the semantics (name validation, the overlap guard, the
/// optimistic title write, no-op detection, the expectation stash) and every
/// surface reads this value to drive its own status, worker and UI wiring, so
/// the decision cannot drift between surfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchRenamePlan {
    /// The rename was refused before any state change; surface the matching
    /// error and stop.
    Rejected(BranchRenameRejection),
    /// The display title was updated and no git branch rename is required: the
    /// caller asked for title-only, or the new name already matches the current
    /// branch. `sync_branches` is true only on the title-only path, which also
    /// refreshes branch-sync state, and false for the name-equals-branch no-op.
    TitleWritten { name: String, sync_branches: bool },
    /// The display title was updated and the expectation stashed; the surface
    /// must dispatch the git rename worker with these parameters.
    RenameBranch(BranchRenameDispatch),
    /// The target session vanished before the rename could be prepared, so the
    /// optimistic title write found nothing either. The surface stays silent.
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
