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
            match fetch_with_one_retry(
                fetch_timeout,
                BASE_BRANCH_FETCH_RETRY_PAUSE,
                BASE_BRANCH_FETCH_RETRY_MIN,
                |bound| git::fetch_origin_bounded(repo, bound),
            ) {
                Ok(()) => OriginFetch::Fetched,
                Err(error) => OriginFetch::Failed(error.to_string()),
            }
        }
        Err(error) => OriginFetch::Failed(error.to_string()),
    };
    let branches = git::list_branches(repo)?;
    Ok(BranchListing { branches, fetch })
}

/// The least a retry is given: with less of the budget left after the pause,
/// the first failure is reported as it is. A retry squeezed into a sliver
/// cannot finish a real fetch, and its timeout would only replace the reason
/// the first attempt gave.
const BASE_BRANCH_FETCH_RETRY_MIN: Duration = Duration::from_secs(5);

/// Run `fetch` bounded by `budget`, and once more after `pause` if it failed
/// and at least `min_retry` of the budget is left after the pause, the second
/// attempt bounded by what remains. Two fetches in one repository race for the
/// refs they update (the listing against a "Pull project", or two listings),
/// and the loser fails with git's "incorrect old value provided"; a moment
/// later the same fetch succeeds.
///
/// A second failure that is a reason git gave is the one reported. A second
/// failure that is only the retry running out of its own time is not: the
/// first attempt's reason is the more useful one, so that is reported.
fn fetch_with_one_retry(
    budget: Duration,
    pause: Duration,
    min_retry: Duration,
    mut fetch: impl FnMut(Duration) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let first = match fetch(budget) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let remaining = budget.saturating_sub(started.elapsed());
    let Some(retry_budget) = remaining
        .checked_sub(pause)
        .filter(|left| *left >= min_retry)
    else {
        return Err(first);
    };
    std::thread::sleep(pause);
    match fetch(retry_budget) {
        Ok(()) => Ok(()),
        Err(second) if second.downcast_ref::<git::FetchTimedOut>().is_some() => Err(first),
        Err(second) => Err(second),
    }
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
    /// out in the project folder too. `by` says whose that worktree is, which
    /// decides the way out the refusal can honestly offer.
    Held { holder: PathBuf, by: BranchHolder },
    /// Creating the local tracking branch or the `git switch` failed; the text
    /// is git's reason (for the log: the message the user reads is the sticky
    /// "Couldn't check out" one).
    SwitchFailed(String),
}

/// Whose worktree holds a branch the project folder cannot check out. Each
/// variant has a different way to free the branch WITHOUT deleting it, and the
/// refusal names exactly that one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchHolder {
    /// A managed agent's worktree; the text is the label its row shows.
    /// Deleting the agent together with its worktree, with the branch box
    /// unticked, frees the branch and keeps it.
    /// Holds whether or not the folder is still there: deleting the agent
    /// with its worktree forgets a registration whose folder is gone too.
    Agent(String),
    /// A managed worktree several agents share; the labels in the order the
    /// agents are listed. Only the LAST of them to be deleted is offered the
    /// worktree box, so freeing the branch means deleting all of them.
    SharedAgentWorktree(Vec<String>),
    /// A standalone agent's folder; the text is the label its row shows.
    /// Deleting the agent never removes its folder, so it frees nothing.
    StandaloneAgent(String),
    /// A worktree in dux's managed area that no agent holds: the project's
    /// worktree manager lists it and can remove it while keeping the branch.
    UnheldManagedWorktree,
    /// A worktree whose folder is gone, which git still counts as holding the
    /// branch. No agent's (an agent's own is [`Self::Agent`]), and the
    /// worktree manager does not list it, since there is no folder to classify:
    /// `git worktree remove` with that one path is what forgets it.
    MissingWorktree,
    /// A worktree folder dux could not look at (permission denied, say): it
    /// may well exist, so the only ways out offered are picking another
    /// branch or fixing access, never anything that removes it.
    UnreadableWorktree,
    /// A worktree outside dux's managed area, which dux does not remove.
    OtherWorktree,
}

/// An agent's directory and the label its row shows, handed to the switch so a
/// branch held there can name the agent rather than a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDirectory {
    pub directory: String,
    pub label: String,
    /// A standalone agent's folder rather than a managed worktree.
    pub standalone: bool,
}

/// What the switch needs to say who holds a branch: every agent's directory
/// and the project's managed worktree area (`<worktrees root>/<project>`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolderContext {
    pub agents: Vec<AgentDirectory>,
    pub managed_root: PathBuf,
}

impl HolderContext {
    /// Classify the worktree at `holder`. Touches the filesystem, so it runs
    /// in the worker.
    ///
    /// Paths are compared the way the worktree registry compares them
    /// ([`crate::worktree_ops::folder_contains`]): resolved as far as they
    /// exist and spelled on from there. A full canonicalization fails for a
    /// folder that is gone, and would then read a missing managed worktree, or
    /// one reached through a symlinked worktrees root, as somebody else's.
    pub fn classify(&self, holder: &Path) -> BranchHolder {
        let agents: Vec<&AgentDirectory> = self
            .agents
            .iter()
            .filter(|agent| same_folder(Path::new(&agent.directory), holder))
            .collect();
        let standalone = agents.iter().find(|agent| agent.standalone);
        let mut managed: Vec<String> = agents
            .iter()
            .filter(|agent| !agent.standalone)
            .map(|agent| agent.label.clone())
            .collect();
        // A managed agent's way out works whether or not its folder is there.
        // A standalone agent in the same folder means deleting never removes
        // it, so none of these applies then.
        if standalone.is_none() {
            match managed.len() {
                0 => {}
                1 => return BranchHolder::Agent(managed.remove(0)),
                _ => return BranchHolder::SharedAgentWorktree(managed),
            }
        }
        // Every other way out needs the folder, so a gone one says so first.
        // Gone means git's own answer for a missing path and nothing else: a
        // folder dux merely could not look at may be full of work, and the
        // gone verdict's advice would remove it.
        match std::fs::symlink_metadata(holder) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return BranchHolder::MissingWorktree;
            }
            Err(error) => {
                crate::logger::warn(&format!(
                    "could not look at the worktree folder {} that holds a branch: {error}",
                    holder.display()
                ));
                return BranchHolder::UnreadableWorktree;
            }
        }
        if let Some(agent) = standalone {
            return BranchHolder::StandaloneAgent(agent.label.clone());
        }
        let in_managed_area = crate::worktree_ops::folder_contains(&self.managed_root, holder)
            && crate::worktree_ops::path_key(holder)
                != crate::worktree_ops::path_key(&self.managed_root);
        if in_managed_area {
            BranchHolder::UnheldManagedWorktree
        } else {
            BranchHolder::OtherWorktree
        }
    }
}

/// Whether two recorded paths name one folder, under every spelling of both.
fn same_folder(a: &Path, b: &Path) -> bool {
    crate::worktree_ops::folder_contains(a, b) && crate::worktree_ops::folder_contains(b, a)
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
    holders: &HolderContext,
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
        let by = holders.classify(&holder);
        return Err(BaseBranchChangeFailure::Held { holder, by });
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
        let pause = Duration::from_millis(10);
        let min = Duration::from_secs(5);
        let mut bounds = Vec::new();
        let result = fetch_with_one_retry(Duration::from_secs(10), pause, min, |bound| {
            bounds.push(bound);
            if bounds.len() == 1 {
                anyhow::bail!("cannot lock ref")
            }
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(bounds.len(), 2);
        assert_eq!(bounds[0], Duration::from_secs(10));
        assert!(bounds[1] <= Duration::from_secs(10) - pause);
        assert!(bounds[1] >= min, "a retry always gets at least the minimum");

        let mut calls = 0;
        let result = fetch_with_one_retry(Duration::ZERO, pause, min, |_| {
            calls += 1;
            anyhow::bail!("timed out")
        });
        assert_eq!(result.unwrap_err().to_string(), "timed out");
        assert_eq!(calls, 1, "a spent budget is not retried");

        let mut calls = 0;
        let result = fetch_with_one_retry(Duration::from_secs(10), pause, min, |_| {
            calls += 1;
            anyhow::bail!("failure {calls}")
        });
        assert_eq!(calls, 2, "one retry, never more");
        assert_eq!(
            result.unwrap_err().to_string(),
            "failure 2",
            "a reason git gave on the retry is reported"
        );
    }

    /// Less than the minimum left after the pause means no retry at all: the
    /// first reason is reported.
    #[test]
    fn a_fetch_with_too_little_budget_left_is_not_retried() {
        let mut calls = 0;
        let result = fetch_with_one_retry(
            Duration::from_secs(4),
            Duration::from_millis(10),
            Duration::from_secs(5),
            |_| {
                calls += 1;
                anyhow::bail!("Authentication failed")
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(result.unwrap_err().to_string(), "Authentication failed");
    }

    /// A retry that fails only because it ran out of its own time does not
    /// replace the first attempt's reason.
    #[test]
    fn a_retry_that_only_timed_out_reports_the_first_reason() {
        let mut calls = 0;
        let result = fetch_with_one_retry(
            Duration::from_secs(10),
            Duration::from_millis(10),
            Duration::from_secs(5),
            |bound| {
                calls += 1;
                if calls == 1 {
                    anyhow::bail!("Authentication failed")
                }
                Err(git::FetchTimedOut { timeout: bound }.into())
            },
        );
        assert_eq!(calls, 2);
        assert_eq!(result.unwrap_err().to_string(), "Authentication failed");
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

        let switched = switch_to_base_branch(
            repo.path(),
            "develop",
            &CheckoutMoveGuard::default(),
            &HolderContext::default(),
        )
        .unwrap();

        assert!(switched.folder_was_on_it, "{switched:?}");
    }

    /// A branch held by an agent's worktree names the agent, matched on
    /// canonical paths so another spelling of the directory still matches; a
    /// worktree no agent has names nobody.
    #[test]
    fn a_held_branch_names_whose_worktree_holds_it() {
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
        let spelled_differently = worktrees
            .path()
            .join(".")
            .join("agent")
            .to_string_lossy()
            .into_owned();
        let other = AgentDirectory {
            directory: "/nowhere/else".to_string(),
            label: "other".to_string(),
            standalone: false,
        };
        let holder_by = |agents: Vec<AgentDirectory>, managed_root: &Path| {
            let holders = HolderContext {
                agents,
                managed_root: managed_root.to_path_buf(),
            };
            let refusal = switch_to_base_branch(
                repo.path(),
                "fix-login",
                &CheckoutMoveGuard::default(),
                &holders,
            )
            .unwrap_err();
            let BaseBranchChangeFailure::Held { holder, by } = refusal else {
                panic!("expected a held refusal: {refusal:?}");
            };
            assert_eq!(holder.canonicalize().unwrap(), held.canonicalize().unwrap());
            by
        };
        let elsewhere = tempfile::tempdir().unwrap();

        assert_eq!(
            holder_by(
                vec![
                    other.clone(),
                    AgentDirectory {
                        directory: spelled_differently.clone(),
                        label: "Fix the login".to_string(),
                        standalone: false,
                    },
                ],
                elsewhere.path(),
            ),
            BranchHolder::Agent("Fix the login".to_string())
        );
        assert_eq!(
            holder_by(
                vec![AgentDirectory {
                    directory: spelled_differently,
                    label: "Notes".to_string(),
                    standalone: true,
                }],
                worktrees.path(),
            ),
            BranchHolder::StandaloneAgent("Notes".to_string()),
            "an agent wins over the managed area it may sit in"
        );
        assert_eq!(
            holder_by(vec![other.clone()], worktrees.path()),
            BranchHolder::UnheldManagedWorktree
        );
        assert_eq!(
            holder_by(vec![other], elsewhere.path()),
            BranchHolder::OtherWorktree
        );
    }

    /// A fetch that fails late for a real reason (here a refusing upload-pack
    /// that answers after a delay) leaves only a sliver of the budget. No
    /// retry is squeezed into it, so its timeout cannot replace the real
    /// reason in what the user reads.
    #[test]
    fn a_late_real_failure_is_reported_as_itself_not_as_a_short_timeout() {
        let (_origin, repo) = clone_of_local_origin();
        let refuse_slowly =
            "sleep 2.5; echo 'fatal: Authentication failed for origin' >&2; exit 1".to_string();
        run_git(
            repo.path(),
            &["config", "remote.origin.uploadpack", &refuse_slowly],
        );

        let listing = load_branch_listing(repo.path(), Duration::from_secs(4)).unwrap();

        let OriginFetch::Failed(reason) = &listing.fetch else {
            panic!("expected a failed fetch: {listing:?}");
        };
        assert!(
            !reason.contains("timed out"),
            "the real reason was replaced by the squeezed retry's timeout: {reason}"
        );
        assert!(reason.contains("Authentication failed"), "{reason}");
    }

    /// With a tag named like the base, the switch says the folder is already
    /// on `develop`, and the project's branch-status poller must agree rather
    /// than read `heads/develop` and call the project off its base.
    #[test]
    fn a_tag_named_like_the_base_leaves_the_status_poller_agreeing_with_the_switch() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["switch", "-q", "-c", "develop"]);
        run_git(repo.path(), &["tag", "develop"]);
        let switched = switch_to_base_branch(
            repo.path(),
            "develop",
            &CheckoutMoveGuard::default(),
            &HolderContext::default(),
        )
        .unwrap();
        assert!(switched.folder_was_on_it);

        let mut project =
            crate::engine::test_support::sample_project("p1", repo.path().to_str().unwrap());
        project.leading_branch = Some("develop".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        crate::project_browser::run_project_branch_status_job(project, tx);
        let crate::worker::WorkerEvent::ProjectBranchStatusReady { result, .. } =
            rx.recv().unwrap()
        else {
            panic!("unexpected event");
        };
        let (branch, status) = result.unwrap();
        assert_eq!(
            (branch.as_str(), status),
            ("develop", crate::model::ProjectBranchStatus::Leading),
            "the poller must agree that the folder is on its base"
        );
    }

    /// A worktree in dux's managed area whose directory is gone still holds its
    /// branch in git. It is not "a worktree dux does not manage" (whose way
    /// out, switching it, cannot be done to a folder that is not there): it
    /// gets its own verdict, and following that refusal's advice exactly,
    /// `git worktree remove` with the path, frees the branch and keeps it.
    #[test]
    fn a_missing_managed_worktree_gets_its_own_refusal_and_its_advice_works() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let root = tempfile::tempdir().unwrap();
        let managed_root = root.path().join("app");
        std::fs::create_dir_all(&managed_root).unwrap();
        let held = managed_root.join("fix-login");
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix-login",
                held.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&held).unwrap();
        let refusal = switch_to_base_branch(
            repo.path(),
            "fix-login",
            &CheckoutMoveGuard::default(),
            &HolderContext {
                agents: Vec::new(),
                managed_root: managed_root.clone(),
            },
        )
        .unwrap_err();
        let BaseBranchChangeFailure::Held { by, holder } = refusal else {
            panic!("expected a held refusal: {refusal:?}");
        };
        assert_eq!(by, BranchHolder::MissingWorktree);

        // The refusal's advice, exactly.
        run_git(
            repo.path(),
            &["worktree", "remove", holder.to_str().unwrap()],
        );
        let switched = switch_to_base_branch(
            repo.path(),
            "fix-login",
            &CheckoutMoveGuard::default(),
            &HolderContext::default(),
        )
        .unwrap();
        assert!(!switched.folder_was_on_it);
        assert_eq!(
            run_git(repo.path(), &["symbolic-ref", "HEAD"]),
            "refs/heads/fix-login"
        );
    }

    /// An agent whose worktree folder is gone, under a worktrees root reached
    /// through a symlink: git reports the holder resolved, the agent's record
    /// keeps the symlinked spelling, and the folder cannot be canonicalized.
    /// The agent is still the one named, since deleting it with its worktree
    /// forgets the registration whether or not the folder is there.
    #[test]
    fn an_agent_whose_worktree_folder_is_gone_behind_a_symlinked_root_is_still_named() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let real = tempfile::tempdir().unwrap();
        let links = tempfile::tempdir().unwrap();
        let linked_root = links.path().join("worktrees");
        std::os::unix::fs::symlink(real.path(), &linked_root).unwrap();
        let through_link = linked_root.join("app").join("fix-login");
        std::fs::create_dir_all(linked_root.join("app")).unwrap();
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix-login",
                through_link.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(real.path().join("app").join("fix-login")).unwrap();
        let holders = HolderContext {
            agents: vec![AgentDirectory {
                directory: through_link.to_string_lossy().into_owned(),
                label: "Fix the login".to_string(),
                standalone: false,
            }],
            managed_root: linked_root.join("app"),
        };

        let refusal = switch_to_base_branch(
            repo.path(),
            "fix-login",
            &CheckoutMoveGuard::default(),
            &holders,
        )
        .unwrap_err();

        let BaseBranchChangeFailure::Held { by, .. } = refusal else {
            panic!("expected a held refusal: {refusal:?}");
        };
        assert_eq!(by, BranchHolder::Agent("Fix the login".to_string()));
    }

    /// Several agents in one worktree are all named, because only the last one
    /// deleted is offered the worktree box; a standalone agent in the folder
    /// wins, because deleting it never removes the folder.
    #[test]
    fn a_shared_worktree_names_every_agent_and_a_standalone_one_wins() {
        let folder = tempfile::tempdir().unwrap();
        let directory = folder.path().to_string_lossy().into_owned();
        let agent = |label: &str, standalone| AgentDirectory {
            directory: directory.clone(),
            label: label.to_string(),
            standalone,
        };
        let classify = |agents| {
            HolderContext {
                agents,
                managed_root: PathBuf::from("/nowhere"),
            }
            .classify(folder.path())
        };

        assert_eq!(
            classify(vec![agent("one", false), agent("two", false)]),
            BranchHolder::SharedAgentWorktree(vec!["one".to_string(), "two".to_string()])
        );
        assert_eq!(
            classify(vec![agent("one", false), agent("notes", true)]),
            BranchHolder::StandaloneAgent("notes".to_string())
        );
    }

    /// A worktree whose folder EXISTS but cannot be looked at (its parent is
    /// unreadable) is never called gone, whose advice would remove a real
    /// folder: it gets the unreadable verdict, which offers no removal.
    #[test]
    fn an_unreadable_existing_worktree_is_not_called_gone() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let outside = tempfile::tempdir().unwrap();
        let parent = outside.path().join("locked");
        std::fs::create_dir_all(&parent).unwrap();
        let held = parent.join("wt");
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix-login",
                held.to_str().unwrap(),
            ],
        );
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refusal = switch_to_base_branch(
            repo.path(),
            "fix-login",
            &CheckoutMoveGuard::default(),
            &HolderContext {
                agents: Vec::new(),
                managed_root: PathBuf::from("/nowhere"),
            },
        );
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            held.join("f").exists(),
            "the worktree folder is really there"
        );
        let Err(BaseBranchChangeFailure::Held { by, .. }) = refusal else {
            panic!("expected a held refusal: {refusal:?}");
        };
        assert_eq!(
            by,
            BranchHolder::UnreadableWorktree,
            "an existing folder dux merely could not stat must not be reported as gone"
        );
    }
}
