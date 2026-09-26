//! The git half of "Change base branch": which branches a project folder can be
//! switched to, and the switch itself. Both run in background workers (the web
//! route's `spawn_blocking`, the engine's change worker, and the terminal UI's
//! listing worker), never on a surface's own thread.
//!
//! The engine half (the per-folder lock, saving the base, the messages) lives
//! in [`crate::engine::Engine::change_project_base_branch`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git::{self, BranchChoice, BranchLocation};

/// How long the listing's `git fetch origin` may run before it is stopped and
/// the list is built from the refs as last fetched. Long enough for an ordinary
/// fetch over a slow link, short enough that a dialog waiting on it is not
/// mistaken for a hang.
pub const BASE_BRANCH_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// What the listing's fetch of `origin` came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginFetch {
    /// Fetched: the origin-only branches are origin's branches right now.
    Fetched,
    /// The project has no `origin` remote, so there was nothing to fetch and
    /// only local branches can be listed.
    NoOrigin,
    /// The fetch failed or timed out; the listing shows origin's branches as
    /// last fetched. The text is the reason.
    Failed(String),
}

/// Every branch a project folder can be switched to, with how fresh the
/// origin-only ones are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchListing {
    pub branches: Vec<BranchChoice>,
    pub fetch: OriginFetch,
}

impl BranchListing {
    /// Whether the origin-only branches were just fetched.
    pub fn fetched(&self) -> bool {
        matches!(self.fetch, OriginFetch::Fetched)
    }

    /// Why they were not, when they were not: the reason a surface shows in
    /// its "listed as last fetched" note.
    pub fn fetch_error(&self) -> Option<String> {
        match &self.fetch {
            OriginFetch::Fetched => None,
            OriginFetch::NoOrigin => Some(
                "the project has no origin remote, so only its local branches are listed"
                    .to_string(),
            ),
            OriginFetch::Failed(reason) => Some(reason.clone()),
        }
    }
}

/// Fetch `origin` once, bounded by `fetch_timeout`, then list the branches of
/// the folder at `repo`. A failed fetch is recorded in the listing, never
/// fatal; only a failed listing is an `Err`.
pub fn load_branch_listing(repo: &Path, fetch_timeout: Duration) -> anyhow::Result<BranchListing> {
    let fetch = match git::has_origin_remote(repo) {
        Ok(false) => OriginFetch::NoOrigin,
        Ok(true) => match git::fetch_origin_bounded(repo, fetch_timeout) {
            Ok(()) => OriginFetch::Fetched,
            Err(error) => OriginFetch::Failed(error.to_string()),
        },
        Err(error) => OriginFetch::Failed(error.to_string()),
    };
    let branches = git::list_branches(repo)?;
    Ok(BranchListing { branches, fetch })
}

/// The folder was switched to the new base (or already was on it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBranchSwitched {
    /// The folder was on the branch before anything ran, so nothing switched.
    pub folder_was_on_it: bool,
}

/// Why the folder was not switched. Nothing was saved in any of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaseBranchChangeFailure {
    /// The project folder does not exist.
    FolderMissing,
    /// The branches could not be listed; the text is git's reason.
    ListFailed(String),
    /// The branch is not a local branch, not a branch on origin, or a name git
    /// refuses as a branch name.
    NotListed,
    /// Another worktree has the branch checked out, so git will not check it
    /// out in the project folder too.
    Held { holder: PathBuf },
    /// Creating the local tracking branch or the `git switch` failed; the text
    /// is git's reason (for the log: the message the user reads is the sticky
    /// "Couldn't check out" one).
    SwitchFailed(String),
}

/// Switch the project folder at `repo` to `branch`, creating the local
/// tracking branch first when only origin has it.
///
/// Validates against the listing rather than trusting the caller: the branch
/// must still be listed (which also refuses a name git rejects as a branch
/// name) and must not be held by another worktree. No fetch runs here: the
/// branch was chosen from a listing that already fetched.
pub fn switch_to_base_branch(
    repo: &Path,
    branch: &str,
) -> Result<BaseBranchSwitched, BaseBranchChangeFailure> {
    if !repo.is_dir() {
        return Err(BaseBranchChangeFailure::FolderMissing);
    }
    let choices = git::list_branches(repo)
        .map_err(|error| BaseBranchChangeFailure::ListFailed(error.to_string()))?;
    let Some(choice) = choices.into_iter().find(|choice| choice.name == branch) else {
        return Err(BaseBranchChangeFailure::NotListed);
    };
    if let Some(holder) = choice.held_by {
        return Err(BaseBranchChangeFailure::Held { holder });
    }
    let folder_was_on_it = git::current_branch_opt(repo)
        .ok()
        .flatten()
        .is_some_and(|current| current == branch);
    if folder_was_on_it {
        return Ok(BaseBranchSwitched {
            folder_was_on_it: true,
        });
    }
    if choice.location == BranchLocation::Remote {
        git::create_tracking_branch(repo, branch)
            .map_err(|error| BaseBranchChangeFailure::SwitchFailed(error.to_string()))?;
    }
    git::switch_branch(repo, branch)
        .map_err(|error| BaseBranchChangeFailure::SwitchFailed(error.to_string()))?;
    Ok(BaseBranchSwitched {
        folder_was_on_it: false,
    })
}
