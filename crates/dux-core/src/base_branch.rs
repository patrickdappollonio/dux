//! The git half of "Change base branch": which branches a project folder can be
//! switched to, and the switch itself. Both run in background workers (the web
//! route's `spawn_blocking`, the engine's change worker, and the terminal UI's
//! listing worker), never on a surface's own thread.
//!
//! The engine half (the per-folder lock, saving the base, the messages) lives
//! in [`crate::engine::Engine::change_project_base_branch`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git::{self, BranchChoice};

/// How long the listing's `git fetch origin` may run before it is stopped and
/// the list is built from the refs as last fetched. Long enough for an ordinary
/// fetch over a slow link, short enough that a dialog waiting on it is not
/// mistaken for a hang.
pub const BASE_BRANCH_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the listing waits before fetching again after a failed fetch,
/// inside [`BASE_BRANCH_FETCH_TIMEOUT`]: long enough for a concurrent fetch in
/// the same repository to finish updating its refs.
const BASE_BRANCH_FETCH_RETRY_PAUSE: Duration = Duration::from_millis(750);

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

/// Fetch `origin` (tried a second time after a failure, both attempts inside
/// `fetch_timeout`), then list the branches of the folder at `repo`. A failed
/// fetch is recorded in the listing, never fatal; only a failed listing is an
/// `Err`.
pub fn load_branch_listing(repo: &Path, fetch_timeout: Duration) -> anyhow::Result<BranchListing> {
    let fetch = match git::has_origin_remote(repo) {
        Ok(false) => OriginFetch::NoOrigin,
        Ok(true) => {
            match fetch_with_one_retry(fetch_timeout, BASE_BRANCH_FETCH_RETRY_PAUSE, |bound| {
                git::fetch_origin_bounded(repo, bound)
            }) {
                Ok(()) => OriginFetch::Fetched,
                Err(error) => OriginFetch::Failed(error.to_string()),
            }
        }
        Err(error) => OriginFetch::Failed(error.to_string()),
    };
    let branches = git::list_branches(repo)?;
    Ok(BranchListing { branches, fetch })
}

/// Run `fetch` bounded by `budget`, and once more after `pause` if it failed
/// and enough of the budget is left, the second attempt bounded by what
/// remains. Two fetches in one repository race for the refs they update (the
/// listing against a "Pull project", or two listings), and the loser fails
/// with git's "incorrect old value provided"; a moment later the same fetch
/// succeeds. A second failure is the one reported.
fn fetch_with_one_retry(
    budget: Duration,
    pause: Duration,
    mut fetch: impl FnMut(Duration) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let first = match fetch(budget) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let remaining = budget.saturating_sub(started.elapsed());
    if remaining <= pause {
        return Err(first);
    }
    std::thread::sleep(pause);
    fetch(remaining - pause)
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
    /// out in the project folder too. `agent` is the label of the agent whose
    /// managed worktree that is, when one is.
    Held {
        holder: PathBuf,
        agent: Option<String>,
    },
    /// Creating the local tracking branch or the `git switch` failed; the text
    /// is git's reason (for the log: the message the user reads is the sticky
    /// "Couldn't check out" one).
    SwitchFailed(String),
}

/// A managed agent's worktree and the label its row shows, handed to the
/// switch so a branch held by that worktree can name the agent rather than a
/// path. Only managed agents: the refusal's way out is deleting the agent,
/// which frees the branch only where deleting removes the worktree, and a
/// standalone agent's folder is never removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorktree {
    pub directory: String,
    pub label: String,
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
    guard: &crate::checkout_move::CheckoutMoveGuard,
    agents: &[AgentWorktree],
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
        // Canonical paths, so another spelling of the worktree still matches.
        let agent = agents
            .iter()
            .find(|agent| {
                crate::project_browser::same_directory(&agent.directory, &holder.to_string_lossy())
            })
            .map(|agent| agent.label.clone());
        return Err(BaseBranchChangeFailure::Held { holder, agent });
    }
    // The full ref, not `--short`: with a tag of the same name the short form
    // is `heads/<branch>` and would never match.
    let folder_was_on_it = git::head_ref_opt(repo)
        .ok()
        .flatten()
        .is_some_and(|head| head == format!("refs/heads/{branch}"));
    if folder_was_on_it {
        return Ok(BaseBranchSwitched {
            folder_was_on_it: true,
        });
    }
    // A branch only origin has is created by the switch itself, which also
    // removes it again when the switch is refused, by the move check or by git.
    git::switch_branch(repo, branch, guard)
        .map_err(|error| BaseBranchChangeFailure::SwitchFailed(error.to_string()))?;
    Ok(BaseBranchSwitched {
        folder_was_on_it: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkout_move::CheckoutMoveGuard;
    use crate::git::test_support::git_command;

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = git_command()
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(dir: &Path) {
        run_git(dir, &["init", "-q", "-b", "main"]);
        run_git(dir, &["config", "user.name", "t"]);
        run_git(dir, &["config", "user.email", "t@t"]);
        std::fs::write(dir.join("f"), "one\n").unwrap();
        run_git(dir, &["add", "f"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// A clone of a scratch `origin` on `main`, both on this machine, so
    /// nothing reaches a network.
    fn clone_of_local_origin() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = tempfile::tempdir().unwrap();
        init_repo(origin.path());
        let repo = tempfile::tempdir().unwrap();
        run_git(
            origin.path(),
            &[
                "clone",
                "-q",
                origin.path().to_string_lossy().as_ref(),
                repo.path().to_string_lossy().as_ref(),
            ],
        );
        run_git(repo.path(), &["config", "user.name", "t"]);
        run_git(repo.path(), &["config", "user.email", "t@t"]);
        (origin, repo)
    }

    /// A fetch that fails once (another fetch in the same repository holding
    /// the ref it wanted to update, most often) is tried again before the
    /// listing settles for the refs as last fetched. The stand-in
    /// `upload-pack` refuses its first call and serves every later one, so the
    /// failure is deterministic and no network is used.
    #[test]
    fn the_listing_fetches_again_once_after_a_failed_fetch() {
        let (origin, repo) = clone_of_local_origin();
        run_git(origin.path(), &["branch", "develop"]);
        let marker = repo.path().join("first-fetch-refused");
        let refuse_once = format!(
            "test -e '{m}' || {{ touch '{m}'; exit 1; }}; git upload-pack",
            m = marker.display()
        );
        run_git(
            repo.path(),
            &["config", "remote.origin.uploadpack", &refuse_once],
        );

        let listing = load_branch_listing(repo.path(), Duration::from_secs(30)).unwrap();

        assert!(marker.exists(), "the first fetch ran and was refused");
        assert_eq!(listing.fetch, OriginFetch::Fetched);
        assert!(
            listing
                .branches
                .iter()
                .any(|choice| choice.name == "develop"),
            "{listing:?}"
        );
    }

    /// The retry fits inside the one budget: the second attempt gets what is
    /// left after the pause, no second attempt runs when nothing is left, and
    /// there is never a third.
    #[test]
    fn the_fetch_retry_stays_inside_the_budget() {
        let mut bounds = Vec::new();
        let result = fetch_with_one_retry(
            Duration::from_secs(10),
            Duration::from_millis(10),
            |bound| {
                bounds.push(bound);
                if bounds.len() == 1 {
                    anyhow::bail!("cannot lock ref")
                }
                Ok(())
            },
        );
        assert!(result.is_ok());
        assert_eq!(bounds.len(), 2);
        assert_eq!(bounds[0], Duration::from_secs(10));
        assert!(bounds[1] <= Duration::from_secs(10) - Duration::from_millis(10));

        let mut calls = 0;
        let result = fetch_with_one_retry(Duration::ZERO, Duration::from_millis(10), |_| {
            calls += 1;
            anyhow::bail!("timed out")
        });
        assert_eq!(result.unwrap_err().to_string(), "timed out");
        assert_eq!(calls, 1, "a spent budget is not retried");

        let mut calls = 0;
        let result =
            fetch_with_one_retry(Duration::from_secs(10), Duration::from_millis(1), |_| {
                calls += 1;
                anyhow::bail!("failure {calls}")
            });
        assert_eq!(calls, 2, "one retry, never more");
        assert_eq!(result.unwrap_err().to_string(), "failure 2");
    }

    /// A tag with the branch's name makes `symbolic-ref --short` answer
    /// `heads/develop`; the full ref is compared instead, so a folder already
    /// on the branch is still recognised as on it.
    #[test]
    fn a_tag_named_like_the_branch_does_not_hide_that_the_folder_is_on_it() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["switch", "-q", "-c", "develop"]);
        run_git(repo.path(), &["tag", "develop"]);
        assert_eq!(
            run_git(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "heads/develop",
            "the short form really is ambiguous here"
        );

        let switched =
            switch_to_base_branch(repo.path(), "develop", &CheckoutMoveGuard::default(), &[])
                .unwrap();

        assert!(switched.folder_was_on_it, "{switched:?}");
    }

    /// A branch held by an agent's worktree names the agent, matched on
    /// canonical paths so another spelling of the directory still matches; a
    /// worktree no agent has names nobody.
    #[test]
    fn a_held_branch_names_the_agent_whose_worktree_holds_it() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let worktrees = tempfile::tempdir().unwrap();
        let held = worktrees.path().join("agent");
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix-login",
                held.to_string_lossy().as_ref(),
            ],
        );
        let spelled_differently = worktrees.path().join(".").join("agent");
        let agents = [
            AgentWorktree {
                directory: "/nowhere/else".to_string(),
                label: "other".to_string(),
            },
            AgentWorktree {
                directory: spelled_differently.to_string_lossy().into_owned(),
                label: "Fix the login".to_string(),
            },
        ];
        let guard = CheckoutMoveGuard::default();

        let named = switch_to_base_branch(repo.path(), "fix-login", &guard, &agents).unwrap_err();
        let BaseBranchChangeFailure::Held { agent, .. } = &named else {
            panic!("expected a held refusal: {named:?}");
        };
        assert_eq!(agent.as_deref(), Some("Fix the login"));

        let unnamed =
            switch_to_base_branch(repo.path(), "fix-login", &guard, &agents[..1]).unwrap_err();
        let BaseBranchChangeFailure::Held { holder, agent } = &unnamed else {
            panic!("expected a held refusal: {unnamed:?}");
        };
        assert_eq!(*agent, None);
        assert_eq!(holder.canonicalize().unwrap(), held.canonicalize().unwrap());
    }
}
