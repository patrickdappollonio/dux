//! The checks the web routes make for a command before handing it to the
//! engine, so that the git and filesystem reads behind them run on the
//! blocking pool rather than on the engine thread.
//!
//! Each check is the same one the engine's wire handler makes when it is
//! given a command with no check attached (a direct `apply_wire` caller), and
//! refuses with the same sentence, so a refusal is still a synchronous answer
//! to the request. The result rides the command in a field a client cannot
//! set (`serde(skip)`), and the engine re-checks only what can change in
//! memory between the two (the registered projects, the agents holding a
//! worktree), which needs no I/O.

use std::path::{Path, PathBuf};

use crate::git;
use crate::model::{AgentSession, Project};

/// What adding a repository as a project read about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddProjectChecked {
    /// The repository root, canonicalized.
    pub validated: PathBuf,
    /// The checked-out branch, `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The base new worktrees branch from, the folder staying where it is.
    pub base: String,
}

/// What checking out the default branch before an add read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutDefaultChecked {
    /// The repository root, canonicalized.
    pub validated: PathBuf,
    /// The checked-out branch, `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The default branch the add switches to.
    pub default_branch: String,
}

/// What creating an initial commit before an add read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitialCommitChecked {
    /// A commit has landed since the client looked: the add is a plain one.
    Born(AddProjectChecked),
    /// Still no commits: the bootstrap runs.
    Unborn {
        /// The repository root, canonicalized.
        validated: PathBuf,
        /// The branch the commit lands on, empty when git names none.
        branch: String,
        /// The project's base once added.
        leading_branch: String,
    },
}

impl InitialCommitChecked {
    /// The repository root the check validated.
    pub fn validated(&self) -> &Path {
        match self {
            InitialCommitChecked::Born(checked) => &checked.validated,
            InitialCommitChecked::Unborn { validated, .. } => validated,
        }
    }
}

/// The managed worktree an agent is adopting, as the listing found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptChecked {
    /// The worktree's path as the listing names it.
    pub path: PathBuf,
    /// Its branch's name.
    pub branch_name: String,
}

/// The repository at `path`, validated against `registered` (the project
/// paths as stored), for a plain add.
///
/// # Errors
///
/// The add's refusal: not an addable repository root, already a project, no
/// commits yet, or git could not read its branch.
pub fn check_add_project(path: &str, registered: &[String]) -> anyhow::Result<AddProjectChecked> {
    let validated = crate::project_browser::validate_add_path(path, registered)
        .map_err(|e| anyhow::anyhow!(e))?;
    if git::repo_commit_state(&validated) == git::CommitState::Unborn {
        anyhow::bail!(
            "\"{}\" has no commits yet, so it can't back a worktree. Create an initial commit first (the app offers to do this for you when adding the project).",
            validated.display()
        );
    }
    plain_add_checked(validated)
}

fn plain_add_checked(validated: PathBuf) -> anyhow::Result<AddProjectChecked> {
    let branch = git::current_branch_opt(&validated)?;
    // A plain add is the dialog's unticked box: the folder stays where it is
    // and new worktrees branch from it.
    let base = crate::project_browser::project_base_for_add(
        &validated,
        branch.as_deref(),
        crate::add_project_plan::DefaultCheckout::StayOnCurrent,
    )
    .into_branch();
    Ok(AddProjectChecked {
        validated,
        branch,
        base,
    })
}

/// The repository at `path` for a check-out-the-default-then-add.
///
/// # Errors
///
/// The add's refusal: not an addable repository root, already a project, no
/// commits yet, or no default branch dux can name for certain.
pub fn check_add_project_checkout_default(
    path: &str,
    registered: &[String],
) -> anyhow::Result<CheckoutDefaultChecked> {
    let validated = crate::project_browser::validate_add_path(path, registered)
        .map_err(|e| anyhow::anyhow!(e))?;
    // A confirmed unborn repo has no default branch to check out; reject
    // explicitly rather than relying on `branch_warning_kind` to bail with a
    // less specific message. Fail open on an indeterminate git result.
    if git::repo_commit_state(&validated) == git::CommitState::Unborn {
        anyhow::bail!(
            "\"{}\" has no commits yet, so there's no default branch to check out. Create an initial commit first.",
            validated.display()
        );
    }
    let branch = git::current_branch_opt(&validated)?;
    let default_branch = crate::wire::checkout_default_target(&validated, branch.as_deref())?;
    Ok(CheckoutDefaultChecked {
        validated,
        branch,
        default_branch,
    })
}

/// The repository at `path` for an initial-commit-then-add.
///
/// # Errors
///
/// The add's refusal: not an addable repository root, already a project, a
/// commit state git cannot settle, or git could not read its branch.
pub fn check_add_project_create_initial_commit(
    path: &str,
    registered: &[String],
) -> anyhow::Result<InitialCommitChecked> {
    let validated = crate::project_browser::validate_add_path(path, registered)
        .map_err(|e| anyhow::anyhow!(e))?;
    // Handle the race where a commit landed between the client's inspect and
    // this request. Fail-closed commit state: only Unborn proceeds to the
    // bootstrap; a now-Born repo is just registered (no commit needed), and an
    // indeterminate git state is a hard, synchronous error.
    match git::repo_commit_state(&validated) {
        git::CommitState::Unborn => {}
        git::CommitState::Born => {
            return plain_add_checked(validated).map(InitialCommitChecked::Born);
        }
        git::CommitState::Indeterminate => {
            anyhow::bail!(
                "Couldn't determine the commit state of \"{}\"; not creating an initial commit. Check the repository and retry.",
                validated.to_string_lossy()
            );
        }
    }
    // Real branch the commit will land on (propagate a git error rather than
    // silently defaulting the branch, matching the sibling add paths).
    let branch = git::current_branch_opt(&validated)?.unwrap_or_default();
    let leading_branch = crate::project_browser::leading_branch_for_project(
        &validated,
        (!branch.is_empty()).then_some(branch.as_str()),
    );
    Ok(InitialCommitChecked::Unborn {
        validated,
        branch,
        leading_branch,
    })
}

/// The managed worktree at `worktree_path` that an agent of `project` may be
/// started in: one dux manages, with no agent in it yet.
///
/// # Errors
///
/// Git cannot list the project's worktrees, or the path is not one of those
/// an agent may adopt.
pub fn check_adopt_worktree(
    project: &Project,
    paths: &crate::config::DuxPaths,
    sessions: &[AgentSession],
    worktree_path: &str,
) -> anyhow::Result<AdoptChecked> {
    let requested = crate::project_browser::canonical_or_original(Path::new(worktree_path));
    let worktrees = git::list_worktrees(Path::new(&project.path))?;
    crate::project_browser::classify_project_worktrees(project, paths, sessions, worktrees)
        .into_iter()
        .filter(|entry| entry.is_selectable() && entry.is_managed_by_dux)
        .find(|entry| entry.path == requested)
        .map(|entry| AdoptChecked {
            path: entry.path,
            branch_name: entry.branch_name,
        })
        .ok_or_else(|| not_adoptable(worktree_path, &project.name))
}

/// The refusal for a worktree no agent may be started in.
pub(crate) fn not_adoptable(worktree_path: &str, project_name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Worktree \"{worktree_path}\" is not an adoptable managed worktree for \
         project \"{project_name}\". Refresh the list and pick a worktree that has no agent yet."
    )
}
