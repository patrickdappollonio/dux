use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use content_inspector::{ContentType, inspect};
use percent_encoding::percent_decode_str;
use url::Url;

use crate::logger;
use crate::model::{ChangedFile, ProjectBranchStatus};
use crate::worker::BranchWarningKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch_name: Option<String>,
    pub detached: bool,
}

impl GitWorktree {
    pub fn label(&self) -> String {
        if let Some(branch_name) = &self.branch_name {
            return branch_name.clone();
        }
        if let Some(head) = &self.head {
            let short = head.chars().take(7).collect::<String>();
            return format!("detached {short}");
        }
        "detached HEAD".to_string()
    }
}

/// Where a branch was found when checking for its existence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchLocation {
    /// The branch exists as a local `refs/heads/` ref.
    Local,
    /// The branch exists only as a remote tracking ref (`refs/remotes/origin/`).
    Remote,
}

/// The create-agent branch preflight decision: whether creating an agent named
/// `name` in a project would start a genuinely FRESH branch or ATTACH to an
/// EXISTING branch's history. The single-source decision both surfaces consume
/// so neither silently attaches without consent: the TUI resolves an
/// `ExistingBranch` through its confirm dialog, and the web refuses an
/// unconfirmed attach and surfaces the same confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateAgentBranchPlan {
    /// No branch of that name exists (local or remote): a new branch is created.
    Fresh,
    /// A branch of that name already exists; attaching would adopt its history.
    ExistingBranch { location: BranchLocation },
}

/// Inspect whether creating an agent named `name` in the repo at `repo_path`
/// would attach to an existing branch. Wraps [`branch_exists`] into the typed
/// [`CreateAgentBranchPlan`] both surfaces branch on. Blocking (a git
/// subprocess), so callers should run it off the UI thread / in the actor.
pub fn create_agent_branch_preflight(repo_path: &Path, name: &str) -> CreateAgentBranchPlan {
    match branch_exists(repo_path, name) {
        Some(location) => CreateAgentBranchPlan::ExistingBranch { location },
        None => CreateAgentBranchPlan::Fresh,
    }
}

enum DiffStat {
    Text(usize, usize),
    Binary,
}

struct StatusEntry {
    index_status: char,
    worktree_status: char,
    path: String,
}

const NULL_DEVICE: &str = "/dev/null";

pub fn current_branch(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", repo_path.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git symbolic-ref failed for {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Like [`current_branch`], but tolerates a detached HEAD: returns `Ok(None)`
/// when HEAD is not a symbolic ref (git `symbolic-ref` exit code 1, with
/// `--quiet` suppressing the message), and `Err` for any real failure
/// (exit 128 = not a repo, git missing, etc.). Used by inspection/preview
/// call sites that must not treat a detached HEAD as a hard error.
pub fn current_branch_opt(repo_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", repo_path.display()))?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    // Exit code 1 = "ref is not a symbolic ref" (detached HEAD). Anything else
    // (128 = not a repo / fatal) is a real error. `--quiet` silenced stderr for
    // the detached case only.
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(anyhow!(
        "git symbolic-ref failed for {}: {}",
        repo_path.display(),
        String::from_utf8_lossy(&output.stderr)
    ))
}

/// Returns the default branch name for the `origin` remote by reading
/// `refs/remotes/origin/HEAD`.  This ref is set automatically by `git clone`;
/// repos created with `git init` + manual remote typically lack it.
///
/// Returns `None` when the ref doesn't exist or the command fails — callers
/// should fall back to a heuristic (e.g. checking if the current branch is
/// `main` or `master`).
pub fn remote_default_branch(repo_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let full_ref = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // e.g. "refs/remotes/origin/main" → "main"
    full_ref
        .strip_prefix("refs/remotes/origin/")
        .map(|s| s.to_string())
}

/// Classifies a checked-out branch against the repo's known default branch.
///
/// - `Some(Known { default_branch })` when `origin/HEAD` resolves to a branch
///   that differs from `branch`.
/// - `None` when `origin/HEAD` resolves to `branch` (already on default), or
///   when it's unavailable and `branch` is one of the common defaults
///   (`main` or `master`).
/// - `Some(Heuristic)` when `origin/HEAD` is unavailable and `branch` is
///   neither `main` nor `master`.
pub fn branch_warning_kind(path: &Path, branch: &str) -> Option<BranchWarningKind> {
    match remote_default_branch(path) {
        Some(default) if default != branch => Some(BranchWarningKind::Known {
            default_branch: default,
        }),
        Some(_) => None,
        None if branch != "main" && branch != "master" => Some(BranchWarningKind::Heuristic),
        None => None,
    }
}

/// Translates a `BranchWarningKind` from [`branch_warning_kind`] into the
/// corresponding `ProjectBranchStatus`. `Some(_) -> NotLeading`,
/// `None -> Leading`.
pub fn branch_status_from_warning(warning_kind: Option<&BranchWarningKind>) -> ProjectBranchStatus {
    match warning_kind {
        Some(_) => ProjectBranchStatus::NotLeading,
        None => ProjectBranchStatus::Leading,
    }
}

pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "rev-parse",
            "--git-dir",
        ])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Where a path sits relative to git repositories, for the add-project and
/// init-repository gates. Unlike [`is_git_repo`] (which answers "is git happy
/// anywhere at or above this path?" and must stay loose because
/// `load_projects` uses it for the `path_missing` flag), this classifies the
/// path precisely so gates can distinguish a repository root from a folder
/// buried inside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoPathKind {
    /// The root of a normal (non-bare) work tree.
    WorkTreeRoot,
    /// The root of a bare repository.
    BareRoot,
    /// A directory inside a work tree but not its root.
    InsideWorkTree { root: PathBuf },
    /// A directory inside git's internal directory (`.git/` of a normal repo,
    /// or the internals of a bare repo such as `objects/`).
    InsideGitDir { git_dir: PathBuf },
    /// Not inside any git repository.
    NotARepo,
    /// Git could not be consulted (spawn failure, unparseable output). Gates
    /// fail open on this; mutations fail closed (the [`CommitState`] doctrine).
    Indeterminate,
}

/// Classify `path` per [`RepoPathKind`] using plumbing only.
///
/// The `--is-inside-git-dir` rung exists because inside a normal repo's
/// `.git` directory `--git-dir` succeeds, `--is-bare-repository` prints
/// `false`, and `--show-toplevel` exits 128 (measured); without the rung that
/// combination would fall through to `Indeterminate` and the fail-open add
/// gate would accept `~/repo/.git` as a project.
pub fn repo_path_kind(path: &Path) -> RepoPathKind {
    let run = |args: &[&str]| -> Option<std::process::Output> {
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
    };
    // Rung 1: is git willing to talk about this path at all?
    match run(&["rev-parse", "--git-dir"]) {
        Some(out) if out.status.success() => {}
        Some(_) => return RepoPathKind::NotARepo,
        None => return RepoPathKind::Indeterminate,
    }
    let capture = |args: &[&str]| -> Option<String> {
        run(args).filter(|out| out.status.success()).map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(['\n', '\r'])
                .to_string()
        })
    };
    // Path outputs must be decoded from the RAW bytes, never via
    // `from_utf8_lossy`: git prints path bytes verbatim, and a repo under a
    // non-UTF8 path (legal on Linux) would have its bytes rewritten to U+FFFD,
    // fail canonicalization, and fall to Indeterminate, which the fail-open
    // add gate accepts, i.e. exactly the paths this ladder exists to stop
    // would slip through.
    let capture_path = |args: &[&str]| -> Option<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        run(args).filter(|out| out.status.success()).map(|out| {
            let mut bytes = out.stdout.as_slice();
            while let [rest @ .., b'\n' | b'\r'] = bytes {
                bytes = rest;
            }
            PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
        })
    };
    // Rung 2: bare repositories. The bare root is addable; a folder inside a
    // bare repo (objects/, refs/, ...) is git internals and must not be.
    match capture(&["rev-parse", "--is-bare-repository"]).as_deref() {
        Some("true") => {
            let Some(git_dir) = capture_path(&["rev-parse", "--absolute-git-dir"]) else {
                return RepoPathKind::Indeterminate;
            };
            let (Ok(canon_git_dir), Ok(canon_path)) = (git_dir.canonicalize(), path.canonicalize())
            else {
                return RepoPathKind::Indeterminate;
            };
            if canon_git_dir == canon_path {
                return RepoPathKind::BareRoot;
            }
            return RepoPathKind::InsideGitDir { git_dir };
        }
        Some(_) => {}
        None => return RepoPathKind::Indeterminate,
    }
    // Rung 3: inside a normal repo's .git directory (see the doc above).
    match capture(&["rev-parse", "--is-inside-git-dir"]).as_deref() {
        Some("true") => {
            let Some(git_dir) = capture_path(&["rev-parse", "--absolute-git-dir"]) else {
                return RepoPathKind::Indeterminate;
            };
            return RepoPathKind::InsideGitDir { git_dir };
        }
        Some(_) => {}
        None => return RepoPathKind::Indeterminate,
    }
    // Rung 4: work tree root vs a folder inside the work tree.
    let Some(toplevel) = capture_path(&["rev-parse", "--show-toplevel"]) else {
        return RepoPathKind::Indeterminate;
    };
    let (Ok(canon_top), Ok(canon_path)) = (toplevel.canonicalize(), path.canonicalize()) else {
        return RepoPathKind::Indeterminate;
    };
    if canon_top == canon_path {
        RepoPathKind::WorkTreeRoot
    } else {
        RepoPathKind::InsideWorkTree { root: toplevel }
    }
}

/// Initialize a new git repository in `path` (`git init`). Imperative: exit
/// status only, stdout unparsed, stderr surfaced in the error. Deliberately
/// honors the user's `init.defaultBranch` (no `-b` override). `git init` runs
/// no hooks; the initial-commit step already pins `core.hooksPath=/dev/null`,
/// which also neutralizes hook scripts an `init.templateDir` might copy in.
pub fn init_repo(path: &Path) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("init")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run git init in {}", path.display()))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git init failed in {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Whether a repository's HEAD resolves to a commit, distinguishing a real
/// git failure from a genuinely unborn HEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitState {
    /// HEAD resolves to a commit — the repo has history.
    Born,
    /// A valid repo whose HEAD is *unborn* (fresh `git init`, no commits yet):
    /// the branch named by HEAD does not exist under `refs/heads/` until the
    /// first commit.
    Unborn,
    /// Could not determine (git failed to run, not a repo, permission/I-O
    /// error). Never conflate this with `Unborn`. The safe handling depends on
    /// the decision: a *reject/gate* site (should this add be blocked? is this
    /// repo unborn for messaging?) treats it as "not unborn" and proceeds
    /// (don't block on a transient hiccup); a *mutation* site (about to create a
    /// commit) refuses (don't mutate a repo whose state we can't confirm).
    Indeterminate,
}

/// Precise, fail-closed probe of a repo's commit state. `git rev-parse --verify
/// --quiet HEAD` exits 0 when HEAD resolves (`Born`), 1 when it doesn't
/// (`Unborn`), and anything else (or a spawn error) is a real failure
/// (`Indeterminate`). Prefer this over [`repo_has_commits`] at any site that
/// makes a hard decision — rejecting an add, failing agent creation, creating a
/// commit — so a transient git hiccup can't be mistaken for "no commits".
pub fn repo_commit_state(path: &Path) -> CommitState {
    let out = Command::new("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "rev-parse",
            "--verify",
            "--quiet",
            "HEAD",
        ])
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) => match o.status.code() {
            Some(0) => CommitState::Born,
            Some(1) => CommitState::Unborn,
            _ => CommitState::Indeterminate,
        },
        Err(_) => CommitState::Indeterminate,
    }
}

/// Returns `true` when the repository at `path` has at least one commit. This is
/// the **fail-open** convenience form (any inability to tell → `false`), fine
/// for UI hints; use [`repo_commit_state`] where the Born/Unborn/Indeterminate
/// distinction matters (any hard reject/gate/mutation).
pub fn repo_has_commits(path: &Path) -> bool {
    matches!(repo_commit_state(path), CommitState::Born)
}

/// Creates an empty initial commit so an otherwise-unborn repo gains a root
/// commit and can back worktrees.
///
/// It is built entirely with **plumbing** (`hash-object` for the empty tree,
/// `commit-tree` for the commit object, `update-ref` to land it) rather than
/// `git commit`, which buys three properties `git commit --allow-empty` cannot:
/// - **No hooks run.** Plumbing never invokes `pre-commit`/`post-commit`/
///   `reference-transaction`/… — a repo's hook scripts must not execute just
///   because dux is adding the project. (`--no-verify` only skips the first two.)
/// - **Empty tree, always.** The commit is built from an explicit empty tree, so
///   it can never bake in whatever happens to be staged in the index at commit
///   time (a race `git commit --allow-empty` is subject to). The user's files —
///   staged or untracked — are left exactly as they were.
/// - **Atomic and bare-safe.** `update-ref <branch> <sha> ""` is a compare-and-
///   swap that creates the branch only if it does not yet exist, so a real
///   commit landing concurrently makes this fail (never a second commit on top);
///   and none of the steps need a work tree, so a **bare** repo works too.
///
/// Returns the short name of the branch the commit landed on, so callers persist
/// the branch that was actually committed rather than one resolved separately
/// (which a concurrent HEAD change could make stale).
///
/// It is *idempotent*: the goal is "the repo has a commit", so if a commit
/// already exists — whether at entry or because the update-ref CAS lost a race
/// to another writer — it returns `Ok(branch)` (no second commit is made)
/// rather than a scary error, letting the caller register the project. It only
/// errors when the index has staged changes (a deliberate courtesy stop so dux
/// doesn't add a project while the user has staged work they may want in the
/// first commit), when git's state can't be determined, on a detached HEAD, or
/// on a genuine git failure (committer identity unset, etc. — surfaced verbatim).
/// Callers still serialize concurrent initial commits on the same repo via the
/// engine's in-flight gate; the CAS is the cross-process backstop.
pub fn create_initial_commit(path: &Path) -> Result<String> {
    let repo = path.to_string_lossy();
    // Fail closed on commit state — only bootstrap a confirmed-unborn repo. A
    // Born repo is idempotent success (a commit already exists, e.g. one raced in
    // between the caller's dispatch-time check and this worker running): the goal
    // is met, so return the current branch (empty if detached — the caller
    // handles that like the normal born path) and let it register.
    match repo_commit_state(path) {
        CommitState::Born => return Ok(current_branch_opt(path)?.unwrap_or_default()),
        CommitState::Unborn => {}
        CommitState::Indeterminate => {
            return Err(anyhow!(
                "couldn't determine the commit state of {}, so refusing to create an initial commit",
                path.display()
            ));
        }
    }
    // Courtesy refuse-if-staged. The commit uses an empty tree and never reads
    // the index, so this can't leak staged content into history — it just stops
    // us from quietly adding a project while the user has staged work pending.
    let staged = Command::new("git")
        .args(["-C", repo.as_ref(), "diff", "--cached", "--quiet"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to inspect the index of {}", path.display()))?;
    match staged.status.code() {
        Some(0) => {}
        Some(1) => {
            return Err(anyhow!(
                "refusing to create an initial commit in {}: you have staged changes. Commit or unstage them first, then add the project.",
                path.display()
            ));
        }
        _ => {
            return Err(anyhow!(
                "failed to inspect the index of {}: {}",
                path.display(),
                String::from_utf8_lossy(&staged.stderr).trim()
            ));
        }
    }
    // Now confirmed unborn, so the symbolic HEAD is guaranteed to exist. Its
    // fully-qualified ref (e.g. `refs/heads/main`) is the branch the commit lands
    // on. (A detached unborn HEAD has no symbolic ref and can't be bootstrapped —
    // reported rather than guessed.)
    let head_ref = run_git_capture(
        path,
        &["symbolic-ref", "HEAD"],
        "resolve the current branch",
    )?;
    let short = head_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&head_ref)
        .to_string();
    // Build the empty-tree commit object with plumbing (no index, no hooks).
    let empty_tree = run_git_capture(
        path,
        &["hash-object", "-t", "tree", NULL_DEVICE],
        "compute the empty tree",
    )?;
    let commit = run_git_capture(
        path,
        &[
            // `commit.gpgsign=false` so a signing prompt can't block; hooksPath at
            // /dev/null so no hook runs at any step of this bootstrap.
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit-tree",
            &empty_tree,
            "-m",
            "Initial commit",
        ],
        "create the initial commit object",
    )?;
    // Land it atomically: CAS with an empty old-value requires the branch to not
    // yet exist, closing the "a real commit landed concurrently" race. hooksPath
    // at /dev/null so the ref update runs no `reference-transaction` hook.
    let update = Command::new("git")
        .args([
            "-C",
            repo.as_ref(),
            "-c",
            "core.hooksPath=/dev/null",
            "update-ref",
            &head_ref,
            &commit,
            "",
        ])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to create initial commit in {}", path.display()))?;
    if !update.status.success() {
        // The CAS lost the race — most likely another writer (a second dux
        // instance, or a concurrent `git commit`) just created the first commit.
        // The goal is "the repo has a commit"; if it's now Born, that goal is
        // met, so treat it as success (idempotent). Re-resolve the current branch
        // fresh (not the pre-race `short`) so we persist the branch the repo is
        // actually on, consistent with the entry-Born path. Only a
        // still-unborn/indeterminate repo is a real error.
        if repo_commit_state(path) == CommitState::Born {
            return Ok(current_branch_opt(path)?.unwrap_or_default());
        }
        return Err(anyhow!(
            "couldn't create the initial commit in {}: {}",
            path.display(),
            String::from_utf8_lossy(&update.stderr).trim()
        ));
    }
    // Return the branch the commit actually landed on (short name) so callers
    // persist THAT, not a name resolved separately before the commit (which a
    // concurrent HEAD change could make stale).
    Ok(short)
}

/// Run a git command that produces a single trimmed line of stdout (a SHA, a
/// ref, …). Returns `Err` with the stderr on non-zero exit. Stdin is detached.
fn run_git_capture(path: &Path, args: &[&str], what: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to {what} for {}", path.display()))?;
    if !out.status.success() {
        return Err(anyhow!(
            "failed to {what} for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn list_worktrees(repo_path: &Path) -> Result<Vec<GitWorktree>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ])
        .output()
        .with_context(|| format!("failed to list worktrees for {}", repo_path.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git worktree list failed for {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    parse_worktree_list_porcelain_z(&output.stdout)
}

pub fn parse_worktree_list_porcelain_z(bytes: &[u8]) -> Result<Vec<GitWorktree>> {
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktree> = None;

    for raw in bytes.split(|byte| *byte == 0) {
        if raw.is_empty() {
            if let Some(entry) = current.take() {
                worktrees.push(entry);
            }
            continue;
        }
        let token = String::from_utf8_lossy(raw);
        if let Some(path) = token.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                worktrees.push(entry);
            }
            current = Some(GitWorktree {
                path: PathBuf::from(path),
                head: None,
                branch_name: None,
                detached: false,
            });
        } else if let Some(head) = token.strip_prefix("HEAD ") {
            if let Some(entry) = &mut current {
                entry.head = Some(head.to_string());
            }
        } else if let Some(branch) = token.strip_prefix("branch ") {
            if let Some(entry) = &mut current {
                entry.branch_name = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            }
        } else if token == "detached"
            && let Some(entry) = &mut current
        {
            entry.detached = true;
        }
    }

    if let Some(entry) = current {
        worktrees.push(entry);
    }

    Ok(worktrees)
}

pub fn pull_current_branch(repo_path: &Path) -> Result<()> {
    let branch = match current_branch_opt(repo_path)? {
        Some(b) => b,
        None => {
            return Err(anyhow!(
                "HEAD is detached; check out a branch before pulling"
            ));
        }
    };
    pull_origin_branch(repo_path, &branch)
}

pub fn pull_branch(repo_path: &Path, branch: &str) -> Result<()> {
    switch_branch_if_needed(repo_path, branch)?;
    pull_origin_branch(repo_path, branch)
}

pub fn switch_branch_if_needed(repo_path: &Path, branch: &str) -> Result<()> {
    // On a detached HEAD there is no current branch to compare against, so we
    // simply switch. Only skip the switch when already on the target branch.
    let current = current_branch_opt(repo_path)?;
    if current.as_deref() != Some(branch) {
        switch_branch(repo_path, branch)?;
    }
    Ok(())
}

/// True when the repo has an `origin` remote. Exit-status only, per the
/// git-safety rules for imperative commands.
pub fn has_origin_remote(repo_path: &Path) -> Result<bool> {
    let status = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "remote",
            "get-url",
            "origin",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| {
            format!(
                "failed to run git remote get-url in {}",
                repo_path.display()
            )
        })?;
    Ok(status.success())
}

fn pull_origin_branch(repo_path: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "pull",
            "--ff-only",
            "origin",
            branch,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git pull failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Switches `repo_path` to `branch_name`. Uses `git switch` rather than
/// `git checkout` because `switch` is single-purpose (branch switching only)
/// and rejects the detached-HEAD and file-restore surprises that `checkout`
/// silently allows. Returns the raw git stderr on failure so callers can
/// surface the concrete reason (e.g. conflicting unstaged changes) to the
/// user. Requires git >= 2.23 (August 2019).
pub fn switch_branch(repo_path: &Path, branch_name: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "switch",
            // `--` so the branch is read as a REF and never as an option.
            // Without it `git switch --detach` detaches HEAD instead of
            // failing. Measured on git 2.55.
            "--",
            branch_name,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git switch {branch_name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Checks whether a branch exists locally or on the `origin` remote.
///
/// Uses the plumbing command `git rev-parse --verify --quiet` and inspects
/// only the exit code — no stdout is parsed.
pub fn branch_exists(repo_path: &Path, name: &str) -> Option<BranchLocation> {
    let repo = repo_path.to_string_lossy();
    let local_ref = format!("refs/heads/{name}");
    if ref_exists(repo_path, &local_ref) {
        return Some(BranchLocation::Local);
    }
    let remote_ref = format!("refs/remotes/origin/{name}");
    let remote = Command::new("git")
        .args([
            "-C",
            repo.as_ref(),
            "rev-parse",
            "--verify",
            "--quiet",
            &remote_ref,
        ])
        .output()
        .ok()
        .is_some_and(|o| o.status.success());
    if remote {
        return Some(BranchLocation::Remote);
    }
    None
}

pub fn local_branch_exists(repo_path: &Path, name: &str) -> bool {
    ref_exists(repo_path, &format!("refs/heads/{name}"))
}

fn ref_exists(repo_path: &Path, ref_name: &str) -> bool {
    let repo = repo_path.to_string_lossy();
    Command::new("git")
        .args([
            "-C",
            repo.as_ref(),
            "rev-parse",
            "--verify",
            "--quiet",
            ref_name,
        ])
        .output()
        .ok()
        .is_some_and(|o| o.status.success())
}

/// Creates a worktree that checks out an **existing** branch (no `-b`).
///
/// When the branch exists only as a remote tracking ref, git automatically
/// creates a local branch that tracks the remote.
pub fn create_worktree_existing_branch(
    repo_path: &Path,
    worktrees_root: &Path,
    project_name: &str,
    branch_name: &str,
) -> Result<(String, PathBuf)> {
    let project_root = worktrees_root.join(project_name);
    fs::create_dir_all(&project_root)?;
    let worktree_path = project_root.join(branch_name);
    let repo = repo_path.to_string_lossy();
    let worktree = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args([
            "-C",
            repo.as_ref(),
            "worktree",
            "add",
            worktree.as_ref(),
            // `--` so the commit-ish is read as a REF and never as an option.
            // Without it `git worktree add <path> --force` obeys the flag and
            // checks out HEAD instead. Measured on git 2.55.
            "--",
            branch_name,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let canonical = worktree_path.canonicalize().unwrap_or(worktree_path);
    Ok((branch_name.to_string(), canonical))
}

pub fn fetch_pull_request_head(repo_path: &Path, pr_number: u64, branch_name: &str) -> Result<()> {
    let repo = repo_path.to_string_lossy();
    let refspec = format!("pull/{pr_number}/head:refs/heads/{branch_name}");
    let output = Command::new("git")
        .args(["-C", repo.as_ref(), "fetch", "origin", &refspec])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git fetch failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub fn create_worktree_from_start_point(
    repo_path: &Path,
    worktrees_root: &Path,
    project_name: &str,
    start_point: Option<&str>,
    custom_name: Option<&str>,
) -> Result<(String, PathBuf)> {
    let branch_name = custom_name
        .map(|s| s.to_string())
        .unwrap_or_else(docker_style_name);
    let project_root = worktrees_root.join(project_name);
    fs::create_dir_all(&project_root)?;
    let worktree_path = project_root.join(&branch_name);
    let repo = repo_path.to_string_lossy();
    let worktree = worktree_path.to_string_lossy();
    let mut command = Command::new("git");
    command.args([
        "-C",
        repo.as_ref(),
        "worktree",
        "add",
        "-b",
        &branch_name,
        worktree.as_ref(),
    ]);
    if let Some(start_point) = start_point {
        command.arg(start_point);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let canonical = worktree_path.canonicalize().unwrap_or(worktree_path);
    Ok((branch_name, canonical))
}

pub fn head_commit(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to inspect HEAD for {}", repo_path.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git rev-parse HEAD failed for {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug, Default)]
pub struct UncommittedCopySummary {
    pub copied: usize,
    pub deleted: usize,
    /// Records skipped because the source is not a regular file or symlink:
    /// dirty submodules (" M dir"), untracked embedded repos ("?? dir/"), and
    /// non-regular files such as FIFOs, sockets, and devices (which would
    /// block or fail a byte copy). Relative paths, for the user-facing note.
    pub skipped_paths: Vec<String>,
}

/// Copies exactly what `git status --porcelain=v1 -z --untracked-files=all`
/// reports in `source` into `destination`. Nothing gitignored travels.
/// PRECONDITION: both worktrees are at the SAME HEAD commit; callers enforce
/// this with the HEAD-equality guard (agent_job.rs), because status deltas
/// are relative to the HEAD commit's tree.
pub fn copy_uncommitted_changes(
    source: &Path,
    destination: &Path,
) -> Result<UncommittedCopySummary> {
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", source.display()))?;
    let destination = destination
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", destination.display()))?;
    if source == destination {
        return Err(anyhow!(
            "source and destination worktrees must be different"
        ));
    }

    let output = Command::new("git")
        .args([
            "-C",
            source.to_string_lossy().as_ref(),
            // Pin rename/copy detection off so every record carries exactly
            // one path (rename records are two-path and config-dependent).
            "-c",
            "status.renames=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Classify every record before touching the filesystem, then run all
    // deletions before any copies (one path can appear twice, e.g. a staged
    // delete plus an untracked recreate, and a tracked file can be replaced
    // by a directory).
    let mut deletions: Vec<PathBuf> = Vec::new();
    let mut copies: Vec<PathBuf> = Vec::new();
    let mut skipped_paths: Vec<String> = Vec::new();

    for record in output.stdout.split(|byte| *byte == 0) {
        if record.len() < 4 {
            continue;
        }
        let index_status = record[0] as char;
        let worktree_status = record[1] as char;
        if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
            return Err(anyhow!(
                "rename/copy detection produced a two-path record despite `-c status.renames=false`; this needs git >= 2.18"
            ));
        }
        let path_bytes = &record[3..];
        let rel: &Path =
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path_bytes).as_ref();
        // Defensive guard: status never emits absolute paths, `..`, or paths
        // under `.git`, but corrupt output must not escape the destination.
        let mut components = rel.components();
        let first_is_git_dir =
            matches!(components.next(), Some(Component::Normal(name)) if name == ".git");
        if rel.is_absolute()
            || first_is_git_dir
            || rel
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            continue;
        }

        if path_bytes.ends_with(b"/") {
            // An untracked directory git will not descend into (an embedded
            // repo): never a recursive copy, skip with a note.
            skipped_paths.push(rel.to_string_lossy().trim_end_matches('/').to_string());
            continue;
        }
        let unmerged = index_status == 'U'
            || worktree_status == 'U'
            || (index_status == 'A' && worktree_status == 'A')
            || (index_status == 'D' && worktree_status == 'D');
        if unmerged {
            // Unmerged records cannot be classified by status code: both `UD`
            // and `DU` leave a file on disk (with different contents). The
            // copy phase decides by source disk state, the only honest source
            // for a mid-merge tree.
            copies.push(rel.to_path_buf());
        } else if worktree_status == 'D' || (index_status == 'D' && worktree_status == ' ') {
            // ` D`/`MD`/`AD` (the worktree delete wins) and a staged delete;
            // an untracked recreate arrives as its own `??` record.
            deletions.push(rel.to_path_buf());
        } else {
            copies.push(rel.to_path_buf());
        }
    }

    let mut summary = UncommittedCopySummary {
        skipped_paths,
        ..Default::default()
    };

    // Phase 1: deletions. Classified purely by status code, never by probing
    // the source filesystem (a `D` means the tracked thing is gone from the
    // working tree regardless of what occupies the path now).
    for rel in &deletions {
        remove_path_if_exists(&destination.join(rel))?;
        summary.deleted += 1;
        prune_empty_ancestors(&destination, rel);
    }

    // Phase 2: copies. The source filesystem is probed on this side only.
    for rel in &copies {
        let source_path = source.join(rel);
        let destination_path = destination.join(rel);
        let metadata = match fs::symlink_metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // The unmerged-record-absent-on-disk branch (e.g. `DD`), and
                // race tolerance for plain copies.
                remove_path_if_exists(&destination_path)?;
                summary.deleted += 1;
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            // A dirty submodule (` M smdir`), an unmerged record that turned
            // out to be a directory, or a non-regular file (FIFO, socket,
            // device): skip with a note, touch nothing. `sync_entry` must
            // only ever see regular files and symlinks, because opening a
            // FIFO with no writer blocks forever.
            summary
                .skipped_paths
                .push(rel.to_string_lossy().to_string());
            continue;
        }
        ensure_destination_parents(&source, &destination, rel)?;
        sync_entry(&source_path, &destination_path)?;
        summary.copied += 1;
    }

    Ok(summary)
}

/// Best-effort removal of now-empty ancestor directories of `rel` inside
/// `destination`, walking up toward (never past) `destination` and stopping
/// at the first non-empty directory. Errors are ignored.
fn prune_empty_ancestors(destination: &Path, rel: &Path) {
    let mut ancestor = rel.parent();
    while let Some(dir) = ancestor {
        if dir.as_os_str().is_empty() {
            break;
        }
        // `remove_dir` refuses to remove a non-empty directory.
        if fs::remove_dir(destination.join(dir)).is_err() {
            break;
        }
        ancestor = dir.parent();
    }
}

/// Create the missing parent directories of `rel` under `destination`,
/// copying the corresponding source directory's mode when it exists (see the
/// umask-safety rationale in `sync_entry`), and tolerating races.
fn ensure_destination_parents(source: &Path, destination: &Path, rel: &Path) -> Result<()> {
    let Some(parent) = rel.parent() else {
        return Ok(());
    };
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        prefix.push(component);
        let destination_dir = destination.join(&prefix);
        if fs::symlink_metadata(&destination_dir).is_ok() {
            continue;
        }
        let mut builder = fs::DirBuilder::new();
        if let Ok(source_meta) = fs::symlink_metadata(source.join(&prefix))
            && source_meta.file_type().is_dir()
        {
            builder.mode(source_meta.permissions().mode());
        }
        match builder.create(&destination_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// Like `remove_path`, but an already-absent path is success.
fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => remove_path(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub struct RemoveResult {
    pub branch_already_deleted: bool,
}

pub fn remove_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<RemoveResult> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "worktree",
            "remove",
            "--force",
            worktree_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !output.status.success() {
        if worktree_path.exists() {
            return Err(anyhow!(
                "git worktree remove failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        // Worktree already gone from disk — prune stale git refs.
        let _ = Command::new("git")
            .args([
                "-C",
                repo_path.to_string_lossy().as_ref(),
                "worktree",
                "prune",
            ])
            .output();
    }
    // Best-effort branch deletion.
    let branch_output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "branch",
            "-D",
            // `--` so the name is read as a REF and never as an option. Without
            // it a ref plumbing created as `--delete` is parsed as the flag and
            // survives the cleanup. Measured on git 2.55.
            "--",
            branch_name,
        ])
        .output()?;
    Ok(RemoveResult {
        branch_already_deleted: !branch_output.status.success(),
    })
}

/// The file listing returned by [`worktree_files`].
#[derive(Debug, Clone)]
pub struct WorktreeFileList {
    pub files: Vec<String>,
    /// `true` when the walk hit the caller's `max_files` cap and some entries
    /// were omitted. The client may surface a subtle hint.
    pub truncated: bool,
}

/// Walk the worktree's filesystem and return every file path (worktree-relative)
/// except the contents of `.git/objects/` and `.git/logs/` (excluded for
/// performance — tens of thousands of loose/pack entries that nobody edits).
/// The rest of `.git/` is included so the editor can open `.git/config`,
/// `.git/HEAD`, hooks, etc. as read-only. Symlinked directories are NOT
/// recursed (`follow_links(false)`); a symlinked dir appears as a leaf entry.
///
/// This feeds the web editor's file-SEARCH index, not its tree (the tree uses
/// [`list_dir`]). Returns at most `max_files` entries and sets `truncated` if
/// more exist; `max_files == 0` disables the cap entirely.
pub fn worktree_files(worktree_path: &Path, max_files: usize) -> Result<WorktreeFileList> {
    use walkdir::WalkDir;

    let wt = worktree_path.to_path_buf();
    let mut files: Vec<String> = Vec::new();
    let mut truncated = false;

    let walker = WalkDir::new(worktree_path)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            // Prune .git/objects and .git/logs subtrees entirely.
            if e.file_type().is_dir() {
                let pruned = e.path().strip_prefix(&wt).is_ok_and(|rel| {
                    let r = rel.to_string_lossy();
                    r == ".git/objects"
                        || r.starts_with(".git/objects/")
                        || r == ".git/logs"
                        || r.starts_with(".git/logs/")
                });
                if pruned {
                    return false; // don't descend into this directory
                }
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // Permission error, broken symlink, etc. — skip and continue;
                // a broken entry should not blank the entire listing.
                logger::warn(&format!("worktree_files: skipping entry: {e}"));
                continue;
            }
        };
        // Only emit leaf paths — directories are structural, not files.
        // Symlinked dirs appear as Symlink (follow_links=false), so they ARE
        // emitted here (the symlink target is not recursed).
        if entry.file_type().is_dir() {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(worktree_path) {
            if max_files > 0 && files.len() >= max_files {
                truncated = true;
                break;
            }
            files.push(rel.to_string_lossy().into_owned());
        }
    }

    files.sort();
    Ok(WorktreeFileList { files, truncated })
}

/// One entry in a single-directory listing for the web editor's lazy file tree.
/// Produced by [`list_dir`]; unlike [`worktree_files`] this never recurses and
/// never caps — it reflects exactly one directory's children as they are on disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirEntryInfo {
    /// The child's own name (final path segment), never a full path.
    pub name: String,
    /// The child's worktree-relative path (`parent_rel/name`, or `name` at root).
    pub path: String,
    /// True when the entry is a directory (a real dir, OR a symlink that
    /// resolves to a directory that is still inside the worktree — those are
    /// expandable).
    pub is_dir: bool,
    /// True when the entry is a symlink (of any kind). The UI may badge it; a
    /// symlinked dir that escapes the worktree is reported with `is_dir = false`
    /// and `expandable = false` so it can never be walked out of the tree.
    pub is_symlink: bool,
    /// True when the UI may request this entry's children via [`list_dir`].
    /// False for files and for symlinked dirs whose target is outside the
    /// worktree.
    pub expandable: bool,
}

/// List exactly one directory of the worktree for the web editor's lazy file
/// tree: a single `read_dir`, no recursion, no cap. `rel_dir` is worktree
/// relative; `""` lists the worktree root. Containment reuses the
/// read-permissive resolver (`.git/` is listable; traversal, absolute paths,
/// and symlinks that escape the worktree are refused).
pub fn list_dir(worktree: &Path, rel_dir: &str) -> Result<Vec<DirEntryInfo>> {
    let abs_dir = if rel_dir.is_empty() {
        worktree.to_path_buf()
    } else {
        let (abs, _is_git_dir, is_outside) =
            crate::worktree_file::resolve_worktree_path_for_read(worktree, rel_dir)?;
        if is_outside {
            return Err(anyhow!(
                "directory resolves outside the worktree: {rel_dir}"
            ));
        }
        abs
    };

    let mut entries: Vec<DirEntryInfo> = Vec::new();
    for entry in std::fs::read_dir(&abs_dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                logger::warn(&format!("list_dir: skipping entry: {e}"));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                logger::warn(&format!("list_dir: skipping entry {name}: {e}"));
                continue;
            }
        };
        let is_symlink = ft.is_symlink();
        let (is_dir, expandable) = if is_symlink {
            // Follow the link to learn whether the target is a directory; a
            // dangling symlink is a plain non-expandable leaf.
            let target_is_dir = std::fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if target_is_dir {
                // Expandable only when the resolved target stays inside the
                // worktree — an escaping symlinked dir is shown but can never
                // be walked out of the tree.
                let in_tree = crate::worktree_file::resolve_worktree_path_for_read(worktree, &path)
                    .map(|(_, _, is_outside)| !is_outside)
                    .unwrap_or(false);
                (in_tree, in_tree)
            } else {
                (false, false)
            }
        } else {
            let d = ft.is_dir();
            (d, d)
        };
        entries.push(DirEntryInfo {
            name,
            path,
            is_dir,
            is_symlink,
            expandable,
        });
    }

    // `file_name().to_string_lossy()` replaces invalid UTF-8 bytes with U+FFFD,
    // so two distinct non-UTF-8 names can collide onto the same lossy `path`
    // (the client keys tree rows by path, so a collision would make one of
    // them unreachable). Drop later duplicates and warn rather than build an
    // escaping scheme — graceful degradation, not full fidelity for names that
    // aren't valid UTF-8 to begin with.
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    entries.retain(|e| {
        if seen_paths.insert(e.path.clone()) {
            true
        } else {
            logger::warn(&format!(
                "list_dir: dropping duplicate entry after lossy UTF-8 name conversion: {}",
                e.path
            ));
            false
        }
    });

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

pub fn changed_files(worktree_path: &Path) -> Result<(Vec<ChangedFile>, Vec<ChangedFile>)> {
    let wt = worktree_path.to_string_lossy();

    let output = Command::new("git")
        .args([
            "-C",
            wt.as_ref(),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();

    for entry in parse_status_porcelain_z(&output.stdout) {
        let index_status = entry.index_status;
        let worktree_status = entry.worktree_status;
        let path = entry.path;

        if index_status == '?' && worktree_status == '?' {
            unstaged.push(ChangedFile {
                status: "?".to_string(),
                path,
                additions: 0,
                deletions: 0,
                binary: false,
            });
            continue;
        }

        if index_status != ' ' {
            staged.push(ChangedFile {
                status: index_status.to_string(),
                path: path.clone(),
                additions: 0,
                deletions: 0,
                binary: false,
            });
        }

        if worktree_status != ' ' {
            unstaged.push(ChangedFile {
                status: worktree_status.to_string(),
                path: path.clone(),
                additions: 0,
                deletions: 0,
                binary: false,
            });
        }
    }

    if let Ok(ns) = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--numstat", "-z"])
        .output()
        && ns.status.success()
    {
        let stats = parse_numstat(&ns.stdout);
        for file in &mut unstaged {
            if let Some(stat) = stats.get(&file.path) {
                match stat {
                    DiffStat::Text(a, d) => {
                        file.additions = *a;
                        file.deletions = *d;
                    }
                    DiffStat::Binary => {
                        file.binary = true;
                    }
                }
            } else if file.status == "?" {
                match untracked_file_diff_stat(worktree_path, &file.path) {
                    Some(DiffStat::Text(a, d)) => {
                        file.additions = a;
                        file.deletions = d;
                    }
                    Some(DiffStat::Binary) => {
                        file.binary = true;
                    }
                    None => {
                        let (additions, binary) =
                            classify_untracked_file_fallback(&worktree_path.join(&file.path));
                        file.additions = additions;
                        file.binary = binary;
                    }
                }
            }
        }
    }

    if let Ok(ns) = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--cached", "--numstat", "-z"])
        .output()
        && ns.status.success()
    {
        let stats = parse_numstat(&ns.stdout);
        for file in &mut staged {
            if let Some(stat) = stats.get(&file.path) {
                match stat {
                    DiffStat::Text(a, d) => {
                        file.additions = *a;
                        file.deletions = *d;
                    }
                    DiffStat::Binary => {
                        file.binary = true;
                    }
                }
            }
        }
    }

    Ok((staged, unstaged))
}

fn untracked_file_diff_stat(worktree_path: &Path, rel_path: &str) -> Option<DiffStat> {
    let output = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "diff",
            "--no-index",
            "--numstat",
            "-z",
            "--",
            NULL_DEVICE,
            rel_path,
        ])
        .output()
        .ok()?;

    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }

    parse_numstat(&output.stdout).into_values().next()
}

fn classify_untracked_file_fallback(path: &Path) -> (usize, bool) {
    let Ok(bytes) = fs::read(path) else {
        return (0, false);
    };
    match inspect(&bytes) {
        ContentType::UTF_8 => match std::str::from_utf8(&bytes) {
            Ok(text) => (text.lines().count(), false),
            Err(_) => (0, true),
        },
        _ => (0, true),
    }
}

fn parse_numstat(raw: &[u8]) -> HashMap<String, DiffStat> {
    let mut stats = HashMap::new();
    let mut records = raw.split(|byte| *byte == 0).peekable();

    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let Some((path, stat)) = parse_numstat_record(record, &mut records) else {
            continue;
        };
        stats.insert(path, stat);
    }

    stats
}

fn parse_numstat_line(line: &str) -> Option<DiffStat> {
    let mut parts = line.split('\t');
    let add = parts.next()?;
    let del = parts.next()?;
    if add == "-" || del == "-" {
        Some(DiffStat::Binary)
    } else {
        Some(DiffStat::Text(add.parse().ok()?, del.parse().ok()?))
    }
}

fn parse_status_porcelain_z(raw: &[u8]) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut records = raw.split(|byte| *byte == 0).peekable();

    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }

        let index_status = record[0] as char;
        let worktree_status = record[1] as char;
        // Renames consume an extra NUL-delimited "old path" record. Advance
        // past it unconditionally so the next record is not misparsed as a
        // top-level status, even when we end up dropping this entry below.
        let is_rename = matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C');
        if is_rename {
            records.next();
        }

        // Strict UTF-8: lossy conversion silently substitutes U+FFFD for any
        // non-UTF-8 bytes in a path. The resulting "string" is then used as
        // an identifier for staging/discarding, which would no longer match
        // the real on-disk path. Skip the entry instead so the user sees one
        // less file rather than a mislabeled one that fails to act on.
        let path = match std::str::from_utf8(&record[3..]) {
            Ok(s) if !s.is_empty() => s.to_string(),
            Ok(_) => continue,
            Err(_) => {
                logger::debug("git status: skipping entry with non-UTF-8 path");
                continue;
            }
        };

        entries.push(StatusEntry {
            index_status,
            worktree_status,
            path,
        });
    }

    entries
}

fn parse_numstat_record<'a, I>(
    record: &[u8],
    records: &mut std::iter::Peekable<I>,
) -> Option<(String, DiffStat)>
where
    I: Iterator<Item = &'a [u8]>,
{
    let first_tab = record.iter().position(|byte| *byte == b'\t')?;
    let second_tab = record[first_tab + 1..]
        .iter()
        .position(|byte| *byte == b'\t')?
        + first_tab
        + 1;
    let stat = parse_numstat_line(std::str::from_utf8(record).ok()?)?;
    let path_bytes = &record[second_tab + 1..];

    // Strict UTF-8 for the same reason as parse_status_porcelain_z: the path
    // string is the lookup key into the status map, so a U+FFFD-substituted
    // string would silently fail to associate stats with the right entry.
    if !path_bytes.is_empty() {
        let path = std::str::from_utf8(path_bytes).ok()?.to_string();
        return Some((path, stat));
    }

    // Rename record: two trailing NUL-delimited paths follow. Consume both
    // even on UTF-8 failure so the iterator stays aligned for the next record.
    let _old_path = records.next()?;
    let new_path = records.next()?;
    let path = std::str::from_utf8(new_path).ok()?.to_string();
    Some((path, stat))
}

pub fn stage_file(worktree_path: &Path, file_path: &str) -> Result<()> {
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "add", "--", file_path])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn unstage_file(worktree_path: &Path, file_path: &str) -> Result<()> {
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "reset", "HEAD", "--", file_path])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git reset failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn discard_file(worktree_path: &Path, file_path: &str, is_untracked: bool) -> Result<()> {
    if is_untracked {
        let full = worktree_path.join(file_path);
        // Defense-in-depth before a destructive remove: callers classify the
        // path against live `git status` output (which never yields paths
        // outside the worktree), but a filesystem delete should not rest on
        // that invariant alone. `is_under` rejects any resolved path that
        // escapes the worktree (e.g. via a symlinked parent component).
        if !is_under(worktree_path, &full) {
            return Err(anyhow!(
                "refusing to delete \"{file_path}\": it resolves outside the worktree"
            ));
        }
        if full.is_dir() {
            fs::remove_dir_all(&full)?;
        } else {
            fs::remove_file(&full)?;
        }
        return Ok(());
    }
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "checkout", "--", file_path])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git checkout failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Classify a discard request against the worktree's LIVE git status and return
/// whether the target file is untracked. Discard is destructive (it deletes
/// untracked files and restores tracked ones from HEAD via [`discard_file`]), so
/// the tracked vs untracked distinction must be derived from `git status` at the
/// moment of the action, never trusted from a client flag or a snapshot captured
/// earlier: a file's tracked/untracked state can change between when a UI decides
/// to offer the discard and when the user confirms it. A file that is currently
/// STAGED cannot be discarded (unstage it first), and a file with no working-tree
/// change has nothing to discard; both are reported as an error.
pub fn discard_classify(worktree_path: &Path, path: &str) -> Result<bool> {
    let (staged, unstaged) = changed_files(worktree_path)?;
    // Reject when the file is staged (and has no separate unstaged change). The
    // TUI and web both surface "Unstage the file first to discard changes." for
    // this case.
    if staged.iter().any(|f| f.path == path) && !unstaged.iter().any(|f| f.path == path) {
        anyhow::bail!("Unstage the file first to discard changes.");
    }
    match unstaged.iter().find(|f| f.path == path) {
        Some(file) => Ok(file.status == "?"),
        None => anyhow::bail!("No unstaged changes to discard for \"{path}\"."),
    }
}

/// Return the text of `git diff --cached` for the given worktree.
/// Uses `-c color.diff=false` to strip ANSI escapes regardless of user config.
pub fn staged_diff_text(worktree_path: &Path) -> Result<String> {
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args([
            "-C",
            wt.as_ref(),
            "-c",
            "color.diff=false",
            "diff",
            "--cached",
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The typed outcome of [`commit_preflight`]: the single decision both surfaces
/// share for whether a commit may proceed. The refusal reasons are stable CODES,
/// not user-facing strings, so each surface renders its own copy (the TUI status
/// line vs the web 400 body) without the wording being pinned in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitPreflight {
    /// The message is empty or whitespace-only.
    EmptyMessage,
    /// The message is fine but nothing is staged (live `git status` has no staged
    /// entry), so `git commit` would fail. Checked against LIVE status rather than
    /// a cached changed-files list so the decision matches the worktree as it is
    /// at commit time, not when the commit UI was last refreshed.
    NothingStaged,
    /// A real message and at least one staged change: safe to commit.
    Ready,
}

/// Decide whether a commit may proceed, from the worktree's LIVE git status.
/// Both the TUI's commit action and the web commit route call this so they agree
/// on the empty-message and nothing-staged refusals (the web previously lacked
/// the nothing-staged gate and let `git commit` fail with raw stderr as a 500).
/// Surface-specific concerns such as a message length cap are NOT decided here.
pub fn commit_preflight(worktree_path: &Path, message: &str) -> CommitPreflight {
    if message.trim().is_empty() {
        return CommitPreflight::EmptyMessage;
    }
    match changed_files(worktree_path) {
        Ok((staged, _unstaged)) if staged.is_empty() => CommitPreflight::NothingStaged,
        // A git-status error is not a preflight refusal: fall through to Ready and
        // let the actual `git commit` surface the underlying error. Treating a
        // transient status failure as "nothing staged" would wrongly block a valid
        // commit.
        _ => CommitPreflight::Ready,
    }
}

pub fn commit(worktree_path: &Path, message: &str) -> Result<String> {
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "commit", "-m", message])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn push(worktree_path: &Path) -> Result<String> {
    let wt = worktree_path.to_string_lossy();
    let branch = match current_branch_opt(worktree_path)? {
        Some(b) => b,
        None => {
            return Err(anyhow!(
                "HEAD is detached; check out a branch before pushing"
            ));
        }
    };
    let output = Command::new("git")
        // `--` so the branch is read as a REFSPEC and never as an option.
        // Without it, a checkout whose HEAD points at a ref named `--all`
        // pushes EVERY branch to the remote. Measured on git 2.55.
        .args(["-C", wt.as_ref(), "push", "-u", "origin", "--", &branch])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Return the contents of a file as raw bytes as it exists at HEAD, or `None`
/// for new (untracked) files. Uses the plumbing command `cat-file` which is
/// immune to user configuration.
pub fn file_bytes_at_head(worktree_path: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    let output = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "cat-file",
            "-p",
            &format!("HEAD:{path}"),
        ])
        .output()?;
    if !output.status.success() {
        // File doesn't exist at HEAD (new/untracked file).
        return Ok(None);
    }
    Ok(Some(output.stdout))
}

/// Return the size in bytes of a file's blob at HEAD via the plumbing command
/// `cat-file -s` — which reads the object header WITHOUT inflating the whole
/// blob — or `None` for new (untracked) files. Lets a caller cap a diff/read by
/// size before buffering the full HEAD content into memory.
pub fn blob_size_at_head(worktree_path: &Path, path: &str) -> Result<Option<u64>> {
    let output = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "cat-file",
            "-s",
            &format!("HEAD:{path}"),
        ])
        .output()?;
    if !output.status.success() {
        // Not present at HEAD (new/untracked file).
        return Ok(None);
    }
    // A successful `cat-file -s` always prints just the decimal byte size. A parse
    // failure here means genuinely unexpected output (corrupt store, a wrapper
    // injecting text) — propagate it rather than collapsing it into the `None`
    // ("absent at HEAD") sentinel, which would silently skip the caller's size cap.
    let raw = String::from_utf8_lossy(&output.stdout);
    let size = raw.trim().parse::<u64>().map_err(|e| {
        anyhow!(
            "git cat-file -s returned non-numeric output {:?}: {e}",
            raw.trim()
        )
    })?;
    Ok(Some(size))
}

pub fn is_under(base: &Path, candidate: &Path) -> bool {
    match (base.canonicalize(), candidate.canonicalize()) {
        (Ok(b), Ok(c)) => c.starts_with(b),
        _ => false,
    }
}

/// The single security boundary for client-supplied worktree-relative paths.
/// Resolves `rel_path` to its on-disk location under `worktree`, rejecting empty
/// or absolute paths, any `..`/`.`/root/prefix component, the `.git` directory,
/// and (for paths that exist) symlinks whose realpath escapes the worktree. A
/// literal `.` (`Component::CurDir`) component is rejected even though it is
/// lexically harmless on its own: after a symlink component, POSIX resolves `.`
/// against the symlink's TARGET directory, so `symlink_metadata` on a path
/// ending in `.` dereferences the preceding symlink and `Path::parent()` on that
/// path strips the symlink component entirely, letting a parent-containment
/// check run against the always-safe worktree root instead of the symlink's
/// real (possibly escaping) location. UI-supplied paths never legitimately need
/// `.`, so it is refused outright rather than specially handled. Returns
/// the joined path, which may not yet exist — existence/file-kind is the caller's
/// concern. (Callers that read/write should additionally refuse symlinks via a
/// no-follow stat to close the dangling-symlink window this existence check can
/// miss — see `worktree_file`.)
///
/// Used by every surface that reads or writes a file from a client path (the
/// diff engine, the web editor endpoints) so the escape check lives in one
/// tested place and cannot drift between call sites.
pub fn resolve_worktree_path(worktree: &Path, rel_path: &str) -> anyhow::Result<PathBuf> {
    let rp = Path::new(rel_path);
    if rp.as_os_str().is_empty()
        || rp.is_absolute()
        || rp.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
        // `Path::components()` normalizes away a non-leading `.` segment (per its
        // own docs: "Occurrences of `.` are normalized away, except if they are
        // at the beginning of the path"), so `a/./b` and `a/.` do NOT surface a
        // `Component::CurDir` above even though the raw string contains one.
        // Check the raw, unnormalized string instead so every `.` segment,
        // anywhere in the path, is caught.
        || rel_path.split('/').any(|seg| seg == ".")
    {
        anyhow::bail!("invalid worktree path: {rel_path}");
    }
    // Never touch a git metadata directory. Reject `.git` as ANY path component,
    // not just the first — editing is gated by containment alone, so a NESTED
    // repo's `.git` (a vendored dep, a submodule) must be unreachable too.
    // Case-insensitive for case-folding filesystems (e.g. default macOS).
    if rp
        .iter()
        .any(|c| c.to_str().is_some_and(|c| c.eq_ignore_ascii_case(".git")))
    {
        anyhow::bail!("refusing to access the git directory: {rel_path}");
    }
    let joined = worktree.join(rel_path);
    // The literal checks above block `..` and `.git` names, but a symlink inside
    // the worktree could still point outside it, OR a symlinked directory could
    // resolve INTO a `.git` dir (sidestepping the literal name check). For paths
    // that exist, resolve and refuse anything whose realpath escapes the worktree
    // or lands inside a `.git` directory.
    if joined.exists() {
        if !is_under(worktree, &joined) {
            anyhow::bail!("path escapes worktree: {rel_path}");
        }
        if resolves_into_git_dir(worktree, &joined) {
            anyhow::bail!("refusing to access the git directory: {rel_path}");
        }
    }
    Ok(joined)
}

/// True when `candidate`'s realpath lies inside a `.git` directory under the
/// worktree — a literal nested `.git`, or one reached through a symlinked
/// directory (which the literal component check can't see). Used to close the
/// symlink-into-`.git` gap on both read/write and on file creation (where the
/// parent dir is checked). Returns false if either path can't be canonicalized.
pub(crate) fn resolves_into_git_dir(worktree: &Path, candidate: &Path) -> bool {
    match (worktree.canonicalize(), candidate.canonicalize()) {
        (Ok(wt), Ok(c)) => c
            .strip_prefix(&wt)
            .map(|rel| {
                rel.components().any(|comp| {
                    comp.as_os_str()
                        .to_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(".git"))
                })
            })
            .unwrap_or(false),
        _ => false,
    }
}

pub fn ellipsize_middle(input: &str, max_width: usize) -> String {
    if input.chars().count() <= max_width {
        return input.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let left = (max_width - 3) / 2;
    let right = max_width - 3 - left;
    let start: String = input.chars().take(left).collect();
    let end: String = input
        .chars()
        .rev()
        .take(right)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{start}...{end}")
}

/// Truncate `input` to at most `max_width` chars, keeping the END and prefixing
/// a single `…` when characters are dropped from the front. Used for paths,
/// where the leaf (the tail) is the informative part and the leading directories
/// can be elided. Measured in chars, matching [`ellipsize_middle`].
pub fn ellipsize_start(input: &str, max_width: usize) -> String {
    let len = input.chars().count();
    if len <= max_width {
        return input.to_string();
    }
    match max_width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let tail: String = input
                .chars()
                .rev()
                .take(max_width - 1)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("…{tail}")
        }
    }
}

/// Shorten `path` for display by stripping a leading `base` directory when
/// `path` sits under it, returning the remainder relative to `base` (no leading
/// slash). Matching is boundary-safe: `base = /home/pat` does NOT strip
/// `/home/patrick/x`. Returns `path` unchanged when `base` is `None`/empty, when
/// `path` is not under `base`, or when `path` IS exactly `base`.
pub fn display_path_relative_to(path: &str, base: Option<&str>) -> String {
    let Some(base) = base else {
        return path.to_string();
    };
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(base)
        && let Some(sub) = rest.strip_prefix('/')
    {
        let sub = sub.trim_start_matches('/');
        if !sub.is_empty() {
            return sub.to_string();
        }
    }
    path.to_string()
}

/// Upper bound on how long a single `git branch -m` may run before the
/// rename worker gives up and kills the child. `git branch -m` is normally
/// instantaneous; a multi-second wait means the process is wedged (a stale
/// `.git/index.lock`, an NFS stall, etc.). Without this bound a hung child
/// never posts `BranchRenameCompleted`, so the session's in-flight marker and
/// `rename_expected` stay set for the process's lifetime — permanently
/// blocking further renames and deferring drift detection.
const RENAME_BRANCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Wait for `child` to exit, killing it and returning an error if `timeout`
/// elapses first. Polls `try_wait` on a short interval rather than blocking on
/// `wait`/`output`, so a wedged process cannot hang the caller forever. On
/// timeout the child is killed and reaped before the error is returned so no
/// zombie is left behind. `what` names the operation for the error message.
fn wait_child_or_kill(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
    what: &str,
) -> Result<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{what} timed out after {}s and was terminated",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Rename a git branch inside a worktree. Runs `git branch -m <old> <new>`
/// from within the worktree directory, bounded by [`RENAME_BRANCH_TIMEOUT`]
/// so a wedged git invocation can't strand the rename worker forever.
pub fn rename_branch(worktree_path: &Path, old_name: &str, new_name: &str) -> Result<()> {
    let mut child = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "branch",
            "-m",
            old_name,
            new_name,
        ])
        .stdout(Stdio::null())
        // `git branch -m` writes only a short line to stderr on failure, well
        // under the pipe buffer, so leaving it undrained until the child exits
        // cannot deadlock. Read it after the wait for the error message.
        .stderr(Stdio::piped())
        .spawn()?;
    let status = wait_child_or_kill(&mut child, RENAME_BRANCH_TIMEOUT, "git branch rename")?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        return Err(anyhow!("git branch rename failed: {}", stderr.trim()));
    }
    Ok(())
}

pub fn docker_style_name() -> String {
    petname::petname(2, "-").expect("petname generation should not fail")
}

/// Returns `true` if `name` contains only characters safe for git branch names:
/// ASCII alphanumeric, dash (`-`), underscore (`_`), and slash (`/`).
/// Also rejects names that start or end with `/`, contain consecutive slashes,
/// or start with `-`, since git forbids these patterns in ref names.
pub fn is_valid_agent_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') {
        return false;
    }
    if name.contains("//") {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
}

/// Input mapper for agent name text fields. Maps characters for insertion,
/// rejecting those that would make the name invalid per [`is_valid_agent_name`]
/// rules. Spaces are transparently converted to dashes. Designed for use with
/// [`TextInput::with_char_map`].
pub fn agent_name_char_map(text: &str, cursor: usize, ch: char) -> Option<char> {
    // Transparently convert spaces to dashes.
    let ch = if ch == ' ' { '-' } else { ch };
    // Only allow ASCII alphanumeric, '-', '_', '/'
    if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '/') {
        return None;
    }
    // First position must be alphanumeric (reject '-', '_', '/')
    if cursor == 0 && !ch.is_ascii_alphanumeric() {
        return None;
    }
    // Prevent '//' by checking the character before and after the cursor
    if ch == '/' {
        if cursor > 0 && text.as_bytes().get(cursor - 1) == Some(&b'/') {
            return None;
        }
        if text.as_bytes().get(cursor) == Some(&b'/') {
            return None;
        }
    }
    Some(ch)
}

#[cfg(test)]
pub(crate) mod test_support {
    /// A `git` command that cannot see the developer's own configuration.
    ///
    /// EVERY git-shelling test fixture in this crate builds its command here.
    /// A fixture that shells out to git directly reads whatever the developer
    /// has configured, and then it passes or fails for reasons that belong to
    /// nobody's code. The concrete hazard is `url.*.insteadOf`, which
    /// `git remote get-url` APPLIES: a developer with one configured (a common
    /// setup, and this repository's own author has exactly that) sees a fixture
    /// remote resolve to a host nobody wrote down. `gh.rs` demonstrates the
    /// rewrite happening rather than asserting it from memory.
    ///
    /// The isolation is PER COMMAND, set through the child's environment. It
    /// used to be process-wide, via `std::env::set_var`, which is unsound in a
    /// threaded test binary (which is why Rust marks it unsafe) and was also
    /// incomplete: several fixtures never called the helper at all, and any of
    /// them could run first or run alongside it.
    ///
    /// Per-command isolation cannot reach a git command spawned by PRODUCTION
    /// code, which inherits the test process's environment. That is deliberate
    /// and is not worked around here: a test whose behaviour depends on git's
    /// configuration composes the two halves itself, running the git command
    /// through this helper and handing the output to the pure parser, which is
    /// the same composition production performs. Production is left alone on
    /// purpose. dux WANTS `insteadOf` rewrites applied when it runs for real,
    /// because the rewritten URL is the one git would actually contact; do not
    /// "fix" production by isolating it from the user's configuration.
    pub(crate) fn git_command() -> std::process::Command {
        let mut command = std::process::Command::new("git");
        isolate_git_config(&mut command);
        command
    }

    /// Applies the isolation to an already-built command, which is what makes
    /// it testable: the removals below have to come AFTER anything that sets
    /// the variables, exactly as they do for a variable the test process
    /// inherited.
    ///
    /// Git reads configuration from FILES and, separately, from the
    /// ENVIRONMENT. Pointing the file lookups at `/dev/null` says nothing about
    /// the environment channel, and the environment channel carries the same
    /// hazard: `GIT_CONFIG_COUNT=1` with `GIT_CONFIG_KEY_0`/`GIT_CONFIG_VALUE_0`
    /// installs an `url.*.insteadOf` rewrite just as a global config file
    /// would, and `GIT_CONFIG_PARAMETERS` (git's own transport for `git -c`) is
    /// a second, independent channel that a zero count does NOT neutralise.
    /// Both were measured, not assumed. Both are removed: with no
    /// `GIT_CONFIG_COUNT` git reads no numbered pair at all, so a stray
    /// `GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` needs no enumerating.
    pub(crate) fn isolate_git_config(command: &mut std::process::Command) {
        command
            // Refuses `/etc/gitconfig` on every git version.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // The explicit paths (git >= 2.32) also cover `$HOME` and
            // `$XDG_CONFIG_HOME` lookups, which is what makes the global file
            // unreachable rather than merely relocated.
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRemote {
    pub host: String,
    pub owner_repo: String,
}

/// What reading a worktree's `origin` concluded. THREE outcomes, not two,
/// because a caller that can only see "resolved" and "nothing" cannot tell an
/// address it may not ask about from an address it could not read, and those
/// two want opposite handling.
///
/// The distinction is load-bearing in the PR poller: an unresolved address may
/// fall back to the host remembered with the agent's last known pull request,
/// because nothing is known about where this agent pushes. A DENIED one may
/// not. Collapsing them sent dux to the remembered host, which is a host this
/// agent's address does not name, and the eligibility gate after the choice
/// could not recover because the live address was already gone by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteResolution {
    /// git could not read an `origin`, or what it read is not an address dux
    /// can parse into a host and an `owner/repo`.
    Unresolved,
    /// A readable address whose host the policy does not allow. dux knows where
    /// this agent pushes and knows it may not ask about it, so the answer is to
    /// ask nothing rather than to ask elsewhere.
    Denied,
    /// A readable address on a host dux may name when it calls `gh`.
    Allowed(GitHubRemote),
}

impl RemoteResolution {
    /// The allowed remote, for callers whose only question is whether they have
    /// one they may use.
    pub fn allowed(self) -> Option<GitHubRemote> {
        match self {
            Self::Allowed(remote) => Some(remote),
            Self::Unresolved | Self::Denied => None,
        }
    }
}

/// Returns the GitHub host and `"owner/repo"` parsed from the `origin` remote
/// URL, or `None` if the remote doesn't point to GitHub or the command fails.
pub fn remote_github_repo(
    worktree_path: &Path,
    policy: &crate::gh::GithubHostPolicy,
) -> Option<GitHubRemote> {
    resolve_remote_github_repo(worktree_path, policy).allowed()
}

/// [`remote_github_repo`] keeping the reason it has no answer. See
/// [`RemoteResolution`].
pub fn resolve_remote_github_repo(
    worktree_path: &Path,
    policy: &crate::gh::GithubHostPolicy,
) -> RemoteResolution {
    let Ok(output) = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
    else {
        return RemoteResolution::Unresolved;
    };
    if !output.status.success() {
        return RemoteResolution::Unresolved;
    }
    resolve_remote_from_git_output(&output.stdout, policy)
}

/// The parse half of [`resolve_remote_github_repo`], keeping the reason.
pub(crate) fn resolve_remote_from_git_output(
    stdout: &[u8],
    policy: &crate::gh::GithubHostPolicy,
) -> RemoteResolution {
    let Ok(text) = std::str::from_utf8(stdout) else {
        return RemoteResolution::Unresolved;
    };
    classify_github_remote(strip_git_record_terminator(text), policy)
}

/// Split the two questions the parser used to answer at once: WHAT ADDRESS is
/// this (the grammar, which does not consult the policy), and MAY DUX ASK about
/// it (the policy, and nothing else).
pub(crate) fn classify_github_remote(
    url: &str,
    policy: &crate::gh::GithubHostPolicy,
) -> RemoteResolution {
    match parse_remote_address(url) {
        None => RemoteResolution::Unresolved,
        Some(remote) if policy.allows(&remote.host) => RemoteResolution::Allowed(remote),
        Some(_) => RemoteResolution::Denied,
    }
}

/// The parse half of [`remote_github_repo`]: git's raw stdout for
/// `git remote get-url` in, a GitHub remote out.
///
/// This is the ONLY place a byte is allowed off the end of a remote, and the
/// only byte it takes off is the one git put there: a single output record
/// terminator. Everything else git printed is part of the remote and reaches
/// the parser intact, because a remote really can hold edge whitespace and a
/// remote with a space in its host is not a GitHub remote.
///
/// Invalid UTF-8 is refused rather than lossily substituted. `U+FFFD` is
/// neither a control character nor whitespace, so a replacement character used
/// to survive every later check and travel into a `gh --repo` argument as part
/// of a name nobody wrote.
#[cfg(test)]
pub(crate) fn github_remote_from_git_output_with(
    stdout: &[u8],
    policy: &crate::gh::GithubHostPolicy,
) -> Option<GitHubRemote> {
    resolve_remote_from_git_output(stdout, policy).allowed()
}

/// [`github_remote_from_git_output_with`] under the legacy name rule, for the
/// tests that are about GIT OUTPUT HANDLING rather than about which hosts
/// qualify. See the shim on [`parse_github_remote`].
#[cfg(test)]
pub(crate) fn github_remote_from_git_output(stdout: &[u8]) -> Option<GitHubRemote> {
    github_remote_from_git_output_with(stdout, &crate::gh::GithubHostPolicy::LegacyNameRule)
}

/// Removes exactly one trailing `\n`. Nothing else, and in particular not a
/// carriage return in front of it.
///
/// This project targets macOS and Linux only and assumes Unix throughout, so
/// git never terminates an output record with CRLF here and there is no CRLF
/// case to preserve. What a trailing `\r\n` really means on these platforms is
/// a remote whose own value ends in a carriage return, with git having appended
/// only the `\n`: an `url.*.insteadOf` replacement ending in one produces
/// exactly that, and the bytes were measured rather than assumed. Taking the
/// `\r` off as well would DELETE a byte of the remote and answer confidently
/// for an address nobody wrote. Left in place it is simply another control
/// character, which the parser already refuses.
fn strip_git_record_terminator(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

/// Extracts `"owner/repo"` from a GitHub remote URL.
///
/// Handles the two spellings that can name a GitHub repository git could reach:
/// the scp-like SSH shorthand (`[user@]github.com:owner/repo.git`, whose colon
/// must precede any slash) and scheme-qualified URLs (`ssh://`, `git://`,
/// `git+ssh://`, `ssh+git://`, `http://`, `https://`, with optional credentials
/// and port). A value with neither is a relative local path to git, not an
/// address, so `github.com/owner/repo` is refused.
///
/// Those are the two spellings dux handles, not the whole of git's grammar. Git
/// also reads the deprecated `ftp`/`ftps` schemes, `<helper>::<address>` for an
/// explicit remote helper, and the tilde-expanded ssh path forms. None of them
/// is a way dux can read reliably, so each is refused deliberately rather than
/// overlooked.
#[cfg(test)]
fn parse_github_owner_repo(url: &str) -> Option<String> {
    parse_github_remote(url).map(|remote| remote.owner_repo)
}

/// The transport schemes a git remote URL may use, spelled exactly as git
/// spells them. Anything else is refused, and the comparison is CASE SENSITIVE
/// because git's is: see the check site in `parse_github_remote`.
///
/// This is the set dux handles, not the full set git understands: the
/// deprecated `ftp`/`ftps` and any `<helper>::` remote helper are refused on
/// purpose, since neither names GitHub. Beyond that, the table is the second
/// line of defence against a scp-like remote: `github.com:owner/repo.git` parses "successfully" as a URL
/// whose *scheme* is `github.com`, and it would be rejected here even if the
/// scp-like branch below were ever bypassed.
///
/// `git+ssh` and `ssh+git` are spellings of `ssh` and invoke the same transport;
/// they are accepted and handled identically. `ftp`/`ftps` stay refused.
const GIT_URL_SCHEMES: [&str; 6] = ["ssh", "git", "git+ssh", "ssh+git", "http", "https"];

/// The schemes for which git decodes the whole address BEFORE it separates host
/// from path, which is what lets a percent escape in the authority move that
/// boundary. Everything git runs over ssh, plus its native protocol; `http` and
/// `https` are deliberately absent, because curl splits first and decodes
/// afterwards. See the decision site in `parse_github_remote` for the
/// measurements behind the split.
const PERCENT_MOVES_THE_BOUNDARY: [&str; 4] = ["ssh", "git+ssh", "ssh+git", "git"];

/// The scheme spellings git actually runs over ssh, matched case sensitively
/// like the table above. Git's native protocol is deliberately absent: it is a
/// different transport on a different port, not an ssh spelling.
///
/// This exists for one host. GitHub documents `ssh.github.com` for SSH and only
/// for SSH, so the acceptance of that name is scoped to these three schemes;
/// see `github_host`.
const SSH_TRANSPORT_SCHEMES: [&str; 3] = ["ssh", "git+ssh", "ssh+git"];

/// [`parse_github_remote_with`] under the rule this function used to hardcode
/// (`github.com` or `github.*`).
///
/// It exists for the grammar tests below. WHICH SPELLINGS PARSE and WHICH HOSTS
/// QUALIFY are two separate questions, and only the second one moved: pinning
/// the tests to the legacy policy keeps every one of them meaning exactly what
/// it meant when it was written, so this change cannot quietly widen or narrow
/// the address grammar under them.
#[cfg(test)]
fn parse_github_remote(url: &str) -> Option<GitHubRemote> {
    parse_github_remote_with(url, &crate::gh::GithubHostPolicy::LegacyNameRule)
}

/// [`parse_remote_address`] with the policy applied, discarding the reason.
#[cfg(test)]
fn parse_github_remote_with(
    url: &str,
    policy: &crate::gh::GithubHostPolicy,
) -> Option<GitHubRemote> {
    classify_github_remote(url, policy).allowed()
}

/// Parse a git remote address into the host and `owner/repo` it names.
///
/// This is the GRAMMAR and only the grammar: what is an address at all, and
/// what repository it names. Whether dux may hand the host to `gh` is a
/// separate question, asked once by [`classify_github_remote`], because a
/// caller has to be able to tell "this is not an address" from "this is an
/// address on a host that is not allowed". Folding the policy in here made the
/// two indistinguishable.
fn parse_remote_address(url: &str) -> Option<GitHubRemote> {
    // The input is consumed EXACTLY, with no trimming of any kind. Trimming
    // used to happen here and then again inside the literal check, which
    // MANUFACTURED matches out of values that are not GitHub remotes:
    // `" ssh://github.com/o/r "` and a value with a trailing tab both became
    // `github.com` `o/r`. A git remote can hold edge whitespace, and what git
    // would then contact is a host with a space in it. Git's own record
    // terminator is removed at the process boundary, in
    // `github_remote_from_git_output`, and nowhere else.

    // 0. Refuse anything the URL parser would silently rewrite rather than
    //    read. This has to come first, because the rewriting happens inside
    //    `Url::parse` and is invisible afterwards.
    if !remote_input_is_literal(url) {
        return None;
    }

    // 1. The scp-like SSH shorthand, `[user@]host:path`. Git's documented rule
    //    is that this is SSH whenever the colon appears before any slash, and
    //    the user part is optional. It is not a URL, so it is parsed by hand,
    //    and it is tried FIRST because `github.com:owner/repo` would otherwise
    //    be read as a URL whose scheme is the hostname.
    if let Some((authority, path)) = split_scp_like(url) {
        // The scp-like spelling is ssh by definition, so GitHub's documented
        // port-443 ssh host is legitimate here.
        let written_host = strip_remote_userinfo(authority).to_ascii_lowercase();
        let host = remote_api_host(&written_host, true);
        // Git hands an scp-like path to ssh verbatim, so it is NOT
        // percent-decoded.
        return owner_repo_from_path(strip_boundary_slashes(path))
            .map(|owner_repo| GitHubRemote { host, owner_repo });
    }

    // 2. Everything with a real scheme goes through the URL parser, which
    //    already separates credentials, ports and IPv6 literals correctly and
    //    rejects a malformed authority (an out-of-range or non-numeric port is
    //    a parse error) instead of guessing at one.
    if let Ok(parsed) = Url::parse(url) {
        // The scheme is matched AS WRITTEN, case sensitively, because that is
        // how git matches it: it compares the literal text before the `://`
        // against its own lowercase table, and anything else it takes as the
        // name of a remote helper to run. MEASURED, git 2.55.0, with a stub
        // `GIT_SSH_COMMAND` printing its argv and a `.invalid` host:
        // `ssh://git@nonexistent-host.invalid/o/r` reaches ssh as
        // `git@nonexistent-host.invalid git-upload-pack '/o/r'`, while
        // `SSH://git@nonexistent-host.invalid/o/r` fails with "git:
        // 'remote-SSH' is not a git command" and "remote helper 'SSH' aborted
        // session". `Ssh://`, `HTTPS://`, `GIT://` and `Git+SSH://` fail the
        // same way, each naming its own missing helper.
        //
        // `parsed.scheme()` cannot be used for this: the `url` crate lowercases
        // the scheme, so it says `ssh` for `SSH://` too, and checking it there
        // is what made dux answer host `github.com`, repository `o/r` for an
        // address git cannot connect with. The HOST is a different matter and
        // stays case insensitive below, because git really does ignore host
        // case (`ssh://NONEXISTENT-HOST.INVALID/o/r` reaches ssh unchanged).
        //
        // Reading the raw scheme also keeps the scp-like second line of
        // defence: `github.com:owner/repo.git` parses as a URL whose scheme is
        // the hostname, and it has no `://` at all, so it stops here.
        let raw_scheme = url.split_once("://")?.0;
        if !GIT_URL_SCHEMES.contains(&raw_scheme) {
            return None;
        }
        // The authority is read RAW as well, because two things about it can
        // only be seen before the parser has split it up.
        let raw_authority = raw_url_authority(url)?;
        // A percent in the authority can move the boundary git splits on, but
        // only for some of the transports, and the split the parser has
        // already performed cannot show it. The asymmetry below is MEASURED,
        // and it is not an oversight.
        //
        // For the ssh-style transports and the native protocol, git decodes a
        // scheme-qualified address and separates host from path AFTERWARDS, in
        // that order, so the encoding is gone by the time the cut is made.
        // `ssh://user%2Fx@host/o/r` reaches ssh as host `user` with the path
        // `/x@host/o/r` (measured with a stub GIT_SSH_COMMAND that prints what
        // git hands ssh), and `git://us%2Fer@host/o/r` makes git look up the
        // host `us` on port 9418 (measured with GIT_TRACE). The crate applies
        // the generic URL grammar, which splits first, and so reports the
        // written host for both. Reproducing git's ordering from output that
        // has already been split would mean reimplementing git's parser, and
        // no legitimate GitHub remote percent-encodes its authority, so these
        // are refused rather than guessed at.
        //
        // For http and https the same shape does NOT move the boundary. Git
        // hands those to curl, which separates the authority from the path
        // first and decodes each piece afterwards, the opposite order:
        // `https://user%2Fx@host/o/r` and `https://u:p%40ss@host/o/r` both
        // reach `https://host/o/r/`, the host and path the address names,
        // which is what the crate reports too. The userinfo there is
        // credentials and is already dropped as credentials. So a percent is
        // allowed under the web schemes, and refusing it would refuse an
        // ordinary remote whose password contains an escaped character.
        if PERCENT_MOVES_THE_BOUNDARY.contains(&parsed.scheme()) && raw_authority.contains('%') {
            return None;
        }
        // Git's native protocol has no user component, unlike its ssh URL
        // syntax, so a `user@` here is part of the HOST:
        // `git://user@github.com/o/r.git` sends git looking up
        // `user@github.com` on port 9418 (measured with GIT_TRACE), not
        // github.com. The crate applies the generic URL grammar and discards
        // the user, which would answer for a repository on a host the remote
        // never names. `ssh`, `git+ssh` and `ssh+git` are ssh, where a user is
        // legitimate; under http(s) userinfo is legitimate credentials and is
        // correctly dropped as such. Only the native protocol lacks it.
        if parsed.scheme() == "git" && raw_authority.contains('@') {
            return None;
        }
        // The ssh port belongs to the ssh service rather than to the host's
        // API, and credentials must never reach a log line or a `gh` argument;
        // taking only the host drops both. For a non-special scheme the parser
        // leaves the host's case alone, so lowercase it explicitly: hostnames
        // are case-insensitive and this value is handed to `gh`.
        let written_host = parsed.host_str()?.to_ascii_lowercase();
        let host = remote_api_host(&written_host, SSH_TRANSPORT_SCHEMES.contains(&raw_scheme));
        // An ssh or git port is the transport service's port and has nothing to
        // do with the host's API, so dropping it is right. An http(s) port is part
        // of the server endpoint, and `gh` cannot express one: it refuses a
        // colon in a hostname and builds fixed API URLs. Keeping the host and
        // discarding the port would send the query to a different server than
        // the remote names, so it is refused instead. A port written out that is
        // the scheme's own default names no other server; the `url` crate has
        // already normalised those away, so `port()` is `None` for them.
        if matches!(parsed.scheme(), "http" | "https") && parsed.port().is_some() {
            return None;
        }
        // The path is taken from the RAW input, not from `Url::path()`. The
        // parser canonicalises `.` and `..` segments away, which git does not
        // do, so `Url::path()` can name a repository the remote never mentioned
        // (`/o/../r/z` becomes `/r/z`). The parser is used for the authority,
        // which it gets right, and for nothing else.
        let raw_path = raw_url_path(url)?;
        // The SYNTACTIC slashes at the boundaries come off BEFORE decoding:
        // exactly the one leading slash the URL grammar puts there, and one
        // trailing slash if it is written. Trimming slash characters after
        // decoding instead erased DECODED separators at those boundaries, so
        // `/%2Fo/r` (three components, the first empty) answered `o/r`.
        let raw_path = strip_boundary_slashes(raw_path);
        // Git percent-decodes the path of a URL, so `%2E` really is a dot. A
        // decoded SLASH is refused outright rather than counted as a separator.
        let path = decode_remote_path(raw_path)?;
        return owner_repo_from_path(&path).map(|owner_repo| GitHubRemote { host, owner_repo });
    }

    // 3. There is no third form. Git's grammar for a remote that leaves the
    //    machine is a scheme-qualified URL or the scp-like `[user@]host:path`,
    //    and the scp-like one REQUIRES its colon before any slash. A value with
    //    neither a scheme nor such a colon is a RELATIVE LOCAL PATH.
    //
    //    There used to be a branch here accepting the bare `github.com/owner/
    //    repo` spelling, and it was the oldest line in this function. It was
    //    wrong: git reads that as a directory. MEASURED, git 2.55.0, isolated
    //    `HOME`, `GIT_CONFIG_NOSYSTEM=1`, a stub `GIT_SSH_COMMAND` printing its
    //    argv, and a `.invalid` host so nothing could leave the machine. With
    //    the remote set to `nonexistent-host.invalid/o/r`, `GIT_TRACE=1 git
    //    ls-remote` runs `git-upload-pack 'nonexistent-host.invalid/o/r'`
    //    LOCALLY and fails with "does not appear to be a git repository"; the
    //    stub ssh is never called and no name is ever resolved. Create the
    //    directory `nonexistent-host.invalid/o/r` as a bare repo and the same
    //    command succeeds against it. The scp-like spelling of the same words,
    //    `nonexistent-host.invalid:o/r`, instead reaches ssh as
    //    `nonexistent-host.invalid git-upload-pack 'o/r'`.
    //
    //    So accepting the bare form meant reporting a folder on disk as a
    //    GitHub repository and then asking GitHub about it. Removing the branch
    //    also removes the last asymmetry in this function: it was the one place
    //    that applied the web family's rules to something git does not run over
    //    a web transport.
    None
}

/// Whether a remote URL can be read literally, which is the only way it can be
/// read truthfully.
///
/// The `url` crate implements the WHATWG URL spec, which DELETES every embedded
/// tab, newline and carriage return before it parses anything, and strips
/// leading and trailing C0 controls. Git does neither: to git those bytes are
/// part of the host or of the path. So `ssh://git<LF>hub.com/o/r` parses to the
/// host `github.com`, which would MANUFACTURE a GitHub match out of a remote
/// that is not GitHub at all and send `gh` after somebody else's repository.
/// There is no way to detect that after the fact, so any input carrying an
/// ASCII control character (C0 or DEL) is refused up front.
///
/// `?` and `#` are refused for the same class of reason: `Url::path()` excludes
/// the query and the fragment, but to git they are ordinary characters in the
/// repository path, so keeping the parser's answer would name a different
/// repository. Neither character can appear in a GitHub owner or repository
/// name, so nothing legitimate is lost by declining to guess.
///
/// A raw backslash is refused for the third variation of the same theme. Under
/// http and https the `url` crate treats `\` as a path separator, so for
/// `https://github.com\ignored/o/r` the crate reported the host `github.com`
/// while the raw-path scan below skipped `\ignored` and answered `o/r`: two
/// parsers disagreeing about where a component begins, which is exactly the
/// hazard this parser exists to remove. No GitHub owner or repository name can
/// contain one, so it is refused wherever it appears rather than reconciled.
///
/// Edge whitespace is refused too. The value used to be trimmed before it got
/// here, so `" ssh://github.com/o/r "` became a GitHub remote. It is not one:
/// git would look up a host with a space in it.
fn remote_input_is_literal(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    if url.starts_with(char::is_whitespace) || url.ends_with(char::is_whitespace) {
        return false;
    }
    !url.bytes()
        .any(|b| b < 0x20 || b == 0x7f || b == b'?' || b == b'#' || b == b'\\')
}

/// The authority of a scheme-qualified remote, sliced out of the ORIGINAL input:
/// everything between the `://` and the `/` that ends it, or the whole remainder
/// when no slash follows. Read raw because the `url` crate has already decoded
/// and split it, and both of those steps can hide where git would have cut.
fn raw_url_authority(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://")?.1;
    Some(match after_scheme.find('/') {
        Some(slash) => &after_scheme[..slash],
        None => after_scheme,
    })
}

/// The path of a scheme-qualified remote, sliced out of the ORIGINAL input so
/// no normalisation can reach it. Userinfo and an IPv6 literal cannot contain an
/// unescaped `/`, so the first `/` after the `://` starts the path. `None` when
/// there is no path, which is not a repository either way.
fn raw_url_path(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://")?.1;
    let slash = after_scheme.find('/')?;
    Some(&after_scheme[slash..])
}

/// Whether a decoded owner or repository name can be handed to `gh` as part of
/// a `--repo` argument.
///
/// Deliberately a rejection list rather than an allow-list. GitHub names use
/// letters, digits, `-`, `_` and `.`, and an enterprise host is free to differ,
/// so a strict identifier allow-list would refuse names that really exist. What
/// is refused is what cannot be a name and can do harm: an empty component,
/// `.` and `..` (path navigation, not a repository), and any control character
/// (C0, DEL and the C1 block, all of Unicode's `Cc`) or whitespace, since these
/// survive percent-decoding and travel straight into a command argument and
/// into log lines.
fn remote_component_is_usable(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
}

/// Splits `[user@]host:path` into its authority and path when the string is
/// git's scp-like SSH shorthand: a colon that appears before any slash, and is
/// neither the `://` of a scheme nor the `::` of an explicit remote helper.
fn split_scp_like(url: &str) -> Option<(&str, &str)> {
    let colon = url.find(':')?;
    let (authority, rest) = url.split_at(colon);
    let rest = &rest[1..];
    if authority.is_empty() || authority.contains('/') || rest.starts_with("//") {
        return None;
    }
    // A SECOND colon right after the first is git's explicit remote-helper
    // syntax, `<transport>::<address>`, which takes precedence over everything
    // else and is not the scp-like shorthand at all. MEASURED, git 2.55.0, same
    // stub ssh and `.invalid` host: `nonexistent-host.invalid::o/r` and
    // `nonexistent-host.invalid::` both fail with "git:
    // 'remote-nonexistent-host.invalid' is not a git command" and "remote helper
    // 'nonexistent-host.invalid' aborted session", never touching ssh, while the
    // single-colon `nonexistent-host.invalid:o/r` reaches ssh as
    // `nonexistent-host.invalid git-upload-pack 'o/r'`. dux used to read the
    // first of those as an scp-like remote and answer host `github.com`,
    // repository `:o/r`: a host git never contacts, and an owner carrying a
    // stray colon.
    //
    // The `user@` spelling is refused here too, for a measurably different
    // reason worth writing down: git requires a helper name to be made of
    // URL-scheme characters and `@` is not one, so
    // `git@nonexistent-host.invalid::o/r` is NOT a helper invocation, it reaches
    // ssh as `git@nonexistent-host.invalid git-upload-pack ':o/r'`. That path is
    // still not `o/r` and `:o` is not an owner, so it is wrong either way.
    //
    // The test is on the byte after the FIRST colon, deliberately, rather than a
    // blunt "the input contains `::`": a scheme-qualified IPv6 literal such as
    // `ssh://[::1]/o/r` contains `::` and is an ordinary ssh remote (measured:
    // it reaches ssh as `::1 git-upload-pack '/o/r'`). Such an address is
    // already ended by the `//` check above and must stay refused only by the
    // host check, which is the one reason that applies to it.
    if rest.starts_with(':') {
        return None;
    }
    Some((authority, rest))
}

/// Drops any `user[:password]@` prefix from a remote's authority. Credentials
/// embedded in a remote URL must never reach the parsed host, which is logged
/// and handed to `gh` as a command argument.
fn strip_remote_userinfo(authority: &str) -> &str {
    match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    }
}

/// Removes the SYNTACTIC slashes at the two ends of a remote's path: at most
/// one at each end, never more.
///
/// It has to be at most one, and it has to run before any percent-decoding.
/// Trimming every slash character erased a DECODED separator sitting at a
/// boundary along with the syntactic one, so `/%2Fo/r` lost the empty first
/// component the decode had just produced and answered `o/r`, and a trailing
/// `%2F` was erased the same way. The leading slash is the URL grammar's; the
/// trailing one is a spelling git tolerates and so does dux.
fn strip_boundary_slashes(path: &str) -> &str {
    let path = path.strip_prefix('/').unwrap_or(path);
    path.strip_suffix('/').unwrap_or(path)
}

/// Percent-decodes a remote's path one raw segment at a time, refusing the
/// whole path if any segment decodes to something containing a slash.
///
/// Git percent-decodes a URL's path, so the decoding has to happen; what must
/// not happen is a decoded slash being read as a separator. A GitHub owner name
/// and a repository name can never contain one, so a decoded slash always means
/// the address names something other than what it appears to name.
/// `https://github.com/octocat%2FHello-World.git` is a single path segment, and
/// the real service was asked: `git ls-remote` for that address answers "Not
/// Found", while `octocat/Hello-World` exists. Answering `octocat/Hello-World`
/// for it sent `gh` after a repository the remote does not address. The
/// position of the encoded slash makes no difference and neither does the
/// scheme, so it is refused everywhere.
///
/// A slash is the only decoded character that can restructure the path this
/// way. A decoded dot is legitimate inside a repository name and stays working
/// (`%2Egit` is still a `.git` suffix); `.` and `..` as whole components are
/// refused by [`remote_component_is_usable`], as are decoded control characters
/// and whitespace. A raw backslash is refused by [`remote_input_is_literal`],
/// but for a reason a percent-encoded one does not share: the `url` crate reads
/// a RAW backslash as a path separator under http(s) and so disagrees with the
/// raw-path scan about where a component begins. An encoded one is invisible to
/// that crate and is not a separator to git either, so it changes no structure
/// and is left to the component checks.
fn decode_remote_path(raw_path: &str) -> Option<String> {
    let mut decoded: Vec<String> = Vec::new();
    for segment in raw_path.split('/') {
        let segment = percent_decode_str(segment).decode_utf8().ok()?;
        if segment.contains('/') {
            return None;
        }
        decoded.push(segment.into_owned());
    }
    Some(decoded.join("/"))
}

/// Takes `owner/repo` from a remote's path, which its caller has already
/// stripped of its boundary slashes, tolerating a `.git` suffix on the
/// repository name.
///
/// EXACTLY two components, for every family of remote. To git the whole path is
/// the repository path, so a third segment means git addresses a repository
/// that is not `owner/repo`, and answering `owner/repo` would send `gh`
/// somewhere git never goes. The http(s) and bare forms used to tolerate extra
/// segments because they double as the URL a user copies out of a browser,
/// where `/tree/main` is a web route layered on the repository path. That
/// leniency is gone: this function's input comes only from
/// `git remote get-url`, never from an address bar, so tolerating a browser
/// route bought nothing and produced wrong answers.
fn owner_repo_from_path(path: &str) -> Option<String> {
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    // GitHub does not allow a repository literally named `.git`, so a bare
    // `.git` segment is the suffix and leaves no repository name behind.
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !remote_component_is_usable(owner) || !remote_component_is_usable(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// GitHub's documented second SSH endpoint, offered for networks that block
/// port 22 and normally written with an explicit `:443`. It is the same GitHub,
/// reached over a different port.
const GITHUB_SSH_ALT_HOST: &str = "ssh.github.com";

/// The host to ASK ABOUT for a remote, which is not always the host written in
/// the remote.
///
/// `ssh_transport` says whether git would run this address over ssh, which
/// matters for exactly one hostname and is passed rather than inferred so the
/// two call sites have to state it.
///
/// `ssh.github.com` is normalised to `github.com`, because this value is handed
/// to the `gh` command line as the host to query and `gh` knows `github.com` as
/// an API host; it has no idea what `ssh.github.com` is. Returning the name as
/// written would turn one silent refusal into a failing lookup, which is not an
/// improvement.
///
/// This function answers "which host is this really" and nothing else. Whether
/// dux may hand that host to `gh` is the policy's answer, asked once, by
/// [`classify_github_remote`]; it used to be asked here too, which is what made
/// a denied host indistinguishable from an unparseable address.
///
/// That normalisation is deliberately the ONLY widening here. It is not a
/// general "any `ssh.` prefix" rule: it is one hostname GitHub documents, and
/// it is matched exactly, so `sshgithub.com`, `x.ssh.github.com` and
/// `ssh.github.com.attacker.example` are all refused. There is no enterprise
/// counterpart either, because GitHub Enterprise Server publishes no equivalent
/// endpoint and inventing one would be guessing at an address on somebody
/// else's network. And it applies only over ssh, the transport it is documented
/// for: `https://ssh.github.com/...` is not an endpoint GitHub offers.
fn remote_api_host(host: &str, ssh_transport: bool) -> String {
    // `gh` has never heard of `ssh.github.com`, so the name that has to be
    // checked against the policy, and handed onwards, is `github.com`.
    if ssh_transport && host == GITHUB_SSH_ALT_HOST {
        "github.com".to_string()
    } else {
        host.to_string()
    }
}

fn sync_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        sync_symlink(source, destination)?;
        return Ok(());
    }

    let source_mode = metadata.permissions().mode();

    if file_type.is_dir() {
        // The status-driven copy expands directories into per-file records
        // before syncing, so a directory reaching this point is a caller bug.
        return Err(anyhow!(
            "sync_entry called on a directory: {}",
            source.display()
        ));
    }

    // Regular file: copy contents through an explicitly-moded handle so the
    // destination never exists with default umask permissions, and so a
    // symlink swapped in between checks can't redirect the write.
    if let Ok(destination_meta) = fs::symlink_metadata(destination) {
        let dest_type = destination_meta.file_type();
        if dest_type.is_dir() || dest_type.is_symlink() {
            remove_path(destination)?;
        }
    }

    let mut input = fs::File::open(source)?;
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(source_mode)
        .open(destination)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            // Destination is a regular file that survived the cleanup pass
            // above. Truncate it in place and realign permissions.
            let file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(destination)?;
            fs::set_permissions(destination, metadata.permissions())?;
            file
        }
        Err(err) => return Err(err.into()),
    };
    io::copy(&mut input, &mut output)?;
    Ok(())
}

fn sync_symlink(source: &Path, destination: &Path) -> Result<()> {
    let target = fs::read_link(source)?;
    if let Ok(existing_target) = fs::read_link(destination)
        && existing_target == target
    {
        return Ok(());
    }
    if destination.exists() || fs::symlink_metadata(destination).is_ok() {
        remove_path(destination)?;
    }
    symlink(&target, destination)?;
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsizes_in_the_middle() {
        assert_eq!(
            ellipsize_middle("src/components/app.rs", 12),
            "src/...pp.rs"
        );
    }

    #[test]
    fn ellipsize_start_keeps_the_tail() {
        // Fits: unchanged.
        assert_eq!(ellipsize_start("proj/app", 12), "proj/app");
        // Too long: keep the tail, prefix a single ellipsis (result width == max).
        let out = ellipsize_start("/home/patrick/code/proj", 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("proj"));
        // Degenerate widths.
        assert_eq!(ellipsize_start("abc", 0), "");
        assert_eq!(ellipsize_start("abc", 1), "…");
    }

    #[test]
    fn display_path_relative_strips_the_base_dir() {
        let base = Some("/home/patrick");
        assert_eq!(
            display_path_relative_to("/home/patrick/proj/a", base),
            "proj/a"
        );
        // A trailing slash on the base is tolerated.
        assert_eq!(
            display_path_relative_to("/home/patrick/proj", Some("/home/patrick/")),
            "proj"
        );
        // Boundary-safe: a shared prefix that is not a path boundary is not stripped.
        assert_eq!(
            display_path_relative_to("/home/patrickson/x", base),
            "/home/patrickson/x"
        );
        // Not under base, exactly the base, or no base: unchanged.
        assert_eq!(display_path_relative_to("/etc/hosts", base), "/etc/hosts");
        assert_eq!(
            display_path_relative_to("/home/patrick", base),
            "/home/patrick"
        );
        assert_eq!(display_path_relative_to("/a/b", None), "/a/b");
    }

    #[test]
    fn is_under_checks_real_paths() {
        let tmp = std::env::temp_dir();
        let child = tmp.join("is_under_test_child");
        std::fs::create_dir_all(&child).unwrap();
        assert!(is_under(&tmp, &child));
        std::fs::remove_dir(&child).unwrap();
    }

    #[test]
    fn is_under_rejects_nonexistent_candidate() {
        let tmp = std::env::temp_dir();
        assert!(!is_under(&tmp, Path::new("/nonexistent/path/xyz")));
    }

    // --- resolve_worktree_path: CurDir (`.`) rejection ---
    //
    // A literal `.` component is never legitimate in a UI-supplied path: it has
    // no meaning a client should be sending, and (per the reviewer's finding
    // reproduced in `worktree_file::tests::delete_refuses_curdir_component`) it
    // can make `symlink_metadata` dereference a preceding symlink and make
    // `Path::parent()` strip the symlink component from containment checks.
    // Reject it lexically at this shared boundary, same as ParentDir.

    #[test]
    fn resolve_worktree_path_rejects_bare_dot() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_worktree_path(dir.path(), ".").is_err());
    }

    #[test]
    fn resolve_worktree_path_rejects_dot_as_middle_component() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        assert!(resolve_worktree_path(dir.path(), "a/./b").is_err());
    }

    #[test]
    fn resolve_worktree_path_rejects_trailing_dot_component() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        assert!(resolve_worktree_path(dir.path(), "a/.").is_err());
    }

    #[test]
    fn resolve_worktree_path_still_accepts_a_plain_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        assert!(resolve_worktree_path(dir.path(), "a/b").is_ok());
    }

    #[test]
    fn docker_name_uses_dash() {
        assert!(docker_style_name().contains('-'));
    }

    // ── Helpers for git-backed tests ─────────────────────────────

    /// Create a temporary bare-ish git repo with an initial commit so
    /// worktrees and branches can be created from it.
    fn init_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let out = test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        run(&["commit", "--allow-empty", "-m", "init"]);
        dir
    }

    /// Create a worktree + branch from the test repo. Returns the worktree path.
    fn add_worktree(repo: &Path, branch: &str) -> PathBuf {
        let wt = repo.join(format!("wt-{branch}"));
        let out = test_support::git_command()
            .args([
                "-C",
                repo.to_string_lossy().as_ref(),
                "worktree",
                "add",
                "-b",
                branch,
                wt.to_string_lossy().as_ref(),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        wt
    }

    #[test]
    fn worktree_files_lists_tracked_untracked_and_loose_ignored_but_collapses_ignored_dirs() {
        let repo = init_test_repo();
        let p = repo.path();
        let g = |args: &[&str]| {
            test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
        };
        std::fs::write(p.join("tracked.txt"), "a").unwrap();
        std::fs::create_dir(p.join("src")).unwrap();
        std::fs::write(p.join("src/main.rs"), "fn main() {}").unwrap();
        g(&["add", "tracked.txt", "src/main.rs"]);
        g(&["commit", "-m", "add tracked"]);
        std::fs::write(p.join("untracked.txt"), "b").unwrap();
        // Ignore a loose file, an entire directory, AND a glob that matches a file
        // sitting in an otherwise-tracked directory (src/).
        std::fs::write(p.join(".gitignore"), "ignored.txt\nnode_modules/\n*.log\n").unwrap();
        std::fs::write(p.join("ignored.txt"), "c").unwrap();
        std::fs::create_dir(p.join("node_modules")).unwrap();
        std::fs::write(p.join("node_modules/dep.js"), "d").unwrap();
        std::fs::write(p.join("src/debug.log"), "e").unwrap();

        let result = worktree_files(p, crate::config::DEFAULT_SEARCH_INDEX_MAX_FILES).unwrap();
        let files = result.files;
        assert!(files.contains(&"tracked.txt".to_string()));
        assert!(files.contains(&"untracked.txt".to_string()));
        assert!(files.contains(&".gitignore".to_string()));
        // A loose gitignored FILE is surfaced so the editor can open it.
        assert!(
            files.contains(&"ignored.txt".to_string()),
            "loose ignored file should be listed: {files:?}"
        );
        // An ignored file inside a directory that ALSO has tracked content is
        // listed too (the walk is filesystem-based, not git-based).
        assert!(
            files.contains(&"src/debug.log".to_string()),
            "ignored file in a partially-tracked dir should be listed: {files:?}"
        );
        // With the walkdir-based implementation, fully-ignored dir contents ARE
        // listed (node_modules/dep.js appears). That is intentional — the new
        // walk surfaces everything. The old ls-files collapse is gone.
        assert!(
            files.iter().any(|f| f.starts_with("node_modules")),
            "walkdir lists ignored directory contents: {files:?}"
        );
        // .git/ contents are listed (except objects/ and logs/).
        assert!(
            files.iter().any(|f| f.starts_with(".git/")),
            "walkdir lists git internals: {files:?}"
        );
    }

    #[test]
    fn worktree_files_walk_lists_git_internals_and_ignored_dir_contents() {
        let repo = init_test_repo();
        let p = repo.path();
        let g = |args: &[&str]| {
            test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
        };
        // A tracked file and a fully-ignored directory with content.
        std::fs::write(p.join("src.rs"), "fn main() {}").unwrap();
        g(&["add", "src.rs"]);
        g(&["commit", "-m", "add file"]);
        std::fs::write(p.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir(p.join("node_modules")).unwrap();
        std::fs::write(p.join("node_modules/dep.js"), "x").unwrap();
        // .git/config always exists in an initialized repo.

        let result = worktree_files(p, crate::config::DEFAULT_SEARCH_INDEX_MAX_FILES).unwrap();

        // Tracked file is included.
        assert!(
            result.files.contains(&"src.rs".to_string()),
            "files: {:?}",
            result.files
        );
        // node_modules contents are included (full walk, not ls-files).
        assert!(
            result.files.contains(&"node_modules/dep.js".to_string()),
            "ignored dir contents must appear: {:?}",
            result.files
        );
        // .git/config is readable in the listing.
        assert!(
            result.files.iter().any(|f| f.starts_with(".git/config")),
            ".git/config must be listed: {:?}",
            result.files
        );
        // .git/objects and .git/logs are excluded for performance.
        assert!(
            !result.files.iter().any(|f| f.starts_with(".git/objects")),
            ".git/objects must be excluded: {:?}",
            result.files
        );
        assert!(
            !result.files.iter().any(|f| f.starts_with(".git/logs")),
            ".git/logs must be excluded: {:?}",
            result.files
        );
        assert!(!result.truncated, "small repo must not be truncated");
    }

    #[test]
    fn list_dir_root_lists_every_child_including_dotfiles_and_git() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(wt.join("Cargo.toml"), "x").unwrap();
        std::fs::write(wt.join("CLAUDE.md"), "x").unwrap();
        std::fs::create_dir(wt.join(".git")).unwrap();
        std::fs::write(wt.join(".git/config"), "[core]\n").unwrap();
        std::fs::create_dir(wt.join(".superpowers")).unwrap();
        std::fs::create_dir(wt.join("crates")).unwrap();

        let names: Vec<String> = list_dir(wt, "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        for expected in ["Cargo.toml", "CLAUDE.md", ".git", ".superpowers", "crates"] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected} in {names:?}"
            );
        }
    }

    /// list_dir is structurally immune to a huge sibling subtree: it does a
    /// single `read_dir` on exactly the requested directory, never recurses,
    /// and never caps. A 1000-file sibling can't discriminate that property
    /// on its own (any number would pass); it just exercises the shape.
    #[test]
    fn list_dir_root_is_unaffected_by_a_huge_sibling_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(wt.join("Cargo.toml"), "x").unwrap();
        let big = wt.join("target");
        std::fs::create_dir(&big).unwrap();
        for i in 0..1000 {
            std::fs::write(big.join(format!("f{i}")), "x").unwrap();
        }
        // list_dir("") reads ONLY the root dir, so `target`'s size is
        // irrelevant and Cargo.toml is always present, whatever readdir order.
        let names: Vec<String> = list_dir(wt, "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"Cargo.toml".to_string()));
        assert!(names.contains(&"target".to_string()));
        assert_eq!(
            names.len(),
            2,
            "root listing must not include descendants: {names:?}"
        );
    }

    #[test]
    fn list_dir_sorts_dirs_first_then_files_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(wt.join("Zoo.txt"), "x").unwrap();
        std::fs::write(wt.join("apple.txt"), "x").unwrap();
        std::fs::create_dir(wt.join("Beta")).unwrap();
        std::fs::create_dir(wt.join("alpha")).unwrap();
        let names: Vec<String> = list_dir(wt, "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["alpha", "Beta", "apple.txt", "Zoo.txt"]);
    }

    #[test]
    fn list_dir_reports_child_paths_relative_to_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::create_dir_all(wt.join("a/b")).unwrap();
        std::fs::write(wt.join("a/b/c.rs"), "x").unwrap();
        let entries = list_dir(wt, "a/b").unwrap();
        let c = entries.iter().find(|e| e.name == "c.rs").unwrap();
        assert_eq!(c.path, "a/b/c.rs");
        assert!(!c.is_dir && !c.expandable);
    }

    #[test]
    fn list_dir_lists_under_dot_git() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::create_dir_all(wt.join(".git/objects")).unwrap();
        std::fs::write(wt.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let names: Vec<String> = list_dir(wt, ".git")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"HEAD".to_string()));
        assert!(names.contains(&"objects".to_string()));
    }

    #[test]
    fn list_dir_rejects_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_dir(dir.path(), "..").is_err());
        assert!(list_dir(dir.path(), "../etc").is_err());
        assert!(list_dir(dir.path(), "/etc").is_err());
    }

    #[test]
    fn list_dir_dedupes_names_that_collide_after_lossy_utf8_conversion() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::tempdir().unwrap();
        // Two distinct single-byte names, both invalid UTF-8 on their own, that
        // `to_string_lossy()` both collapse to the same single replacement
        // character (U+FFFD) — a real filesystem collision the tree UI (which
        // keys rows by path) must never see duplicated.
        let name_a = OsString::from_vec(vec![0xFF]);
        let name_b = OsString::from_vec(vec![0xFE]);
        std::fs::write(dir.path().join(&name_a), "a").unwrap();
        std::fs::write(dir.path().join(&name_b), "b").unwrap();

        let entries = list_dir(dir.path(), "").unwrap();
        let mut paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        paths.sort_unstable();
        let unique_count = {
            let mut deduped = paths.clone();
            deduped.dedup();
            deduped.len()
        };
        assert_eq!(
            entries.len(),
            unique_count,
            "list_dir must never return two entries with the same path: {entries:?}"
        );
    }

    #[test]
    fn list_dir_symlinked_dir_escaping_worktree_is_not_expandable() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("secret")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("escape"))
            .unwrap();
        let e = list_dir(dir.path(), "")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "escape")
            .unwrap();
        assert!(e.is_symlink);
        assert!(
            !e.expandable,
            "an escaping symlinked dir must not be expandable"
        );
        // And listing THROUGH it must be refused.
        assert!(list_dir(dir.path(), "escape").is_err());
    }

    #[test]
    fn list_dir_in_worktree_symlinked_dir_is_expandable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/x.txt"), "x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let e = list_dir(dir.path(), "")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "link")
            .unwrap();
        assert!(e.is_symlink && e.is_dir && e.expandable);
        // Listing through the in-tree symlink works.
        let names: Vec<String> = list_dir(dir.path(), "link")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"x.txt".to_string()));
    }

    #[test]
    fn worktree_files_walk_sets_truncated_when_cap_exceeded() {
        let repo = init_test_repo();
        let p = repo.path();
        // Create enough files to exceed a small explicit cap.
        std::fs::create_dir(p.join("bulk")).unwrap();
        for i in 0..25 {
            std::fs::write(p.join(format!("bulk/f{i}.txt")), "x").unwrap();
        }
        let result = worktree_files(p, 20).unwrap();
        assert!(result.truncated, "must set truncated when walk exceeds cap");
        assert_eq!(result.files.len(), 20, "must return exactly cap entries");
    }

    #[test]
    fn worktree_files_zero_cap_never_truncates() {
        let repo = init_test_repo();
        let p = repo.path();
        std::fs::create_dir(p.join("bulk")).unwrap();
        for i in 0..50 {
            std::fs::write(p.join(format!("bulk/f{i}.txt")), "x").unwrap();
        }
        let result = worktree_files(p, 0).unwrap();
        assert!(!result.truncated, "max_files == 0 must disable the cap");
        assert!(
            result
                .files
                .iter()
                .filter(|f| f.starts_with("bulk/"))
                .count()
                == 50,
            "all bulk files must be listed: {}",
            result.files.len()
        );
    }

    #[test]
    fn parse_worktree_list_porcelain_z_handles_branches_detached_and_spaces() {
        let input = b"worktree /repo/main checkout\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0worktree /repo/feature\0HEAD 2222222222222222222222222222222222222222\0branch refs/heads/feature/x\0\0worktree /repo/detached\0HEAD abcdef1234567890\0detached\0\0";

        let entries = parse_worktree_list_porcelain_z(input).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, PathBuf::from("/repo/main checkout"));
        assert_eq!(entries[0].branch_name.as_deref(), Some("main"));
        assert_eq!(entries[0].label(), "main");
        assert_eq!(entries[1].branch_name.as_deref(), Some("feature/x"));
        assert_eq!(entries[2].branch_name, None);
        assert!(entries[2].detached);
        assert_eq!(entries[2].label(), "detached abcdef1");
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let out = test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_all(cwd: &Path, message: &str) {
        run_git(cwd, &["add", "-A"]);
        run_git(cwd, &["commit", "-m", message]);
    }

    #[test]
    fn commit_preflight_matrix_empty_message_then_nothing_staged_then_ready() {
        let repo = init_test_repo();
        let wt = repo.path();

        // Empty (or whitespace-only) message is refused first, before any git IO
        // decision about staging.
        assert_eq!(
            commit_preflight(wt, "   "),
            CommitPreflight::EmptyMessage,
            "a whitespace-only message must be refused as empty",
        );

        // A valid message but nothing staged: live git status has no staged entry.
        assert_eq!(
            commit_preflight(wt, "real message"),
            CommitPreflight::NothingStaged,
            "a clean worktree has nothing to commit",
        );

        // Stage a change and the preflight clears.
        fs::write(wt.join("a.txt"), "hello\n").unwrap();
        run_git(wt, &["add", "a.txt"]);
        assert_eq!(
            commit_preflight(wt, "real message"),
            CommitPreflight::Ready,
            "a staged change with a real message is ready to commit",
        );
    }

    #[test]
    fn discard_classify_reflects_live_status_as_the_worktree_changes() {
        // The classification must track the CURRENT git status, transitioning as
        // the same path moves between untracked, staged, tracked-and-modified, and
        // clean. This is the property the discard confirm relies on: it re-reads
        // this at action time rather than trusting an earlier snapshot.
        let repo = init_test_repo();
        let wt = repo.path();

        // Untracked file: classified as untracked (the delete branch).
        fs::write(wt.join("ghost.txt"), "ghost\n").unwrap();
        assert!(
            discard_classify(wt, "ghost.txt").unwrap(),
            "a brand-new untracked file must classify as untracked",
        );

        // Once staged, discard is refused (unstage first) rather than classified.
        run_git(wt, &["add", "ghost.txt"]);
        let staged_err = discard_classify(wt, "ghost.txt").unwrap_err().to_string();
        assert!(
            staged_err.contains("Unstage the file first"),
            "a staged file must be refused, got: {staged_err}",
        );

        // A tracked file with an unstaged modification classifies as tracked (the
        // restore-from-HEAD branch), NOT untracked.
        fs::write(wt.join("tracked.txt"), "one\n").unwrap();
        commit_all(wt, "add tracked");
        fs::write(wt.join("tracked.txt"), "two\n").unwrap();
        assert!(
            !discard_classify(wt, "tracked.txt").unwrap(),
            "a modified tracked file must classify as tracked, not untracked",
        );

        // A clean/committed path has nothing to discard.
        commit_all(wt, "commit tracked change");
        let clean_err = discard_classify(wt, "tracked.txt").unwrap_err().to_string();
        assert!(
            clean_err.contains("No unstaged changes to discard"),
            "a clean tracked file must report nothing to discard, got: {clean_err}",
        );
    }

    #[test]
    fn create_worktree_from_start_point_uses_explicit_head_commit() {
        let repo = init_test_repo();
        let source = add_worktree(repo.path(), "source-head");
        fs::write(source.join("fork.txt"), "from source branch\n").unwrap();
        commit_all(&source, "source commit");
        let source_head = head_commit(&source).unwrap();

        let worktrees_root = repo.path().join("forks");
        let (_branch_name, forked) = create_worktree_from_start_point(
            repo.path(),
            &worktrees_root,
            "demo",
            Some(&source_head),
            None,
        )
        .unwrap();

        assert_eq!(head_commit(&forked).unwrap(), source_head);
        assert_eq!(
            fs::read_to_string(forked.join("fork.txt")).unwrap(),
            "from source branch\n"
        );
    }

    // ── copy_uncommitted_changes tests ───────────────────────────
    //
    // The copy is driven by `git status --porcelain=v1 -z --untracked-files=all`
    // in the source; each test names the bug it catches.

    /// Two sibling worktrees of the same test repo, at the same HEAD commit.
    fn copy_test_worktrees(repo: &Path) -> (PathBuf, PathBuf) {
        let source = add_worktree(repo, "copy-source");
        let destination = add_worktree(repo, "copy-destination");
        (source, destination)
    }

    /// The core rule: files matched by .gitignore never travel.
    #[test]
    fn copy_excludes_gitignored_files() {
        let repo = init_test_repo();
        fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(repo.path().join("tracked.txt"), "original\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::write(source.join("tracked.txt"), "modified\n").unwrap();
        fs::write(source.join("note.txt"), "untracked\n").unwrap();
        fs::write(source.join("junk.log"), "ignored\n").unwrap();

        let summary = copy_uncommitted_changes(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("tracked.txt")).unwrap(),
            "modified\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("note.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!destination.join("junk.log").exists());
        assert_eq!(summary.copied, 2);
        assert!(summary.skipped_paths.is_empty());
    }

    /// Catches routing `?? dir/` through a bulk directory copy (which would
    /// drag ignored files along) and the missing-destination-parent case.
    #[test]
    fn copy_expands_untracked_dirs_and_still_excludes_ignored_files_inside_them() {
        let repo = init_test_repo();
        fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::create_dir_all(source.join("newdir").join("nested")).unwrap();
        fs::write(source.join("newdir").join("keep.txt"), "keep\n").unwrap();
        fs::write(
            source.join("newdir").join("nested").join("deep.txt"),
            "deep\n",
        )
        .unwrap();
        fs::write(source.join("newdir").join("junk.log"), "ignored\n").unwrap();

        copy_uncommitted_changes(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("newdir").join("keep.txt")).unwrap(),
            "keep\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("newdir").join("nested").join("deep.txt")).unwrap(),
            "deep\n"
        );
        assert!(!destination.join("newdir").join("junk.log").exists());
    }

    /// Catches the proven defect: ` D foo` + `?? foo/bar.txt` must delete the
    /// stale destination file before copying into the new directory, or the
    /// copy fails with `File exists` / leaves a stale file behind.
    #[test]
    fn copy_handles_tracked_file_replaced_by_directory() {
        let repo = init_test_repo();
        fs::write(repo.path().join("foo"), "a file\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::remove_file(source.join("foo")).unwrap();
        fs::create_dir(source.join("foo")).unwrap();
        fs::write(source.join("foo").join("bar.txt"), "inner\n").unwrap();

        copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(destination.join("foo").is_dir());
        assert_eq!(
            fs::read_to_string(destination.join("foo").join("bar.txt")).unwrap(),
            "inner\n"
        );
    }

    /// `D  f` + `?? f` is one path reported twice; delete-then-copy phase
    /// ordering must land the recreated contents, not the deletion.
    #[test]
    fn copy_handles_staged_delete_with_untracked_recreate() {
        let repo = init_test_repo();
        fs::write(repo.path().join("f"), "old\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        run_git(&source, &["rm", "f"]);
        fs::write(source.join("f"), "new\n").unwrap();

        copy_uncommitted_changes(&source, &destination).unwrap();

        assert_eq!(fs::read_to_string(destination.join("f")).unwrap(), "new\n");
    }

    /// `MD`: the worktree delete (Y) wins over the staged modify (X).
    #[test]
    fn copy_applies_worktree_delete_over_staged_modify() {
        let repo = init_test_repo();
        fs::write(repo.path().join("a.txt"), "base\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::write(source.join("a.txt"), "staged edit\n").unwrap();
        run_git(&source, &["add", "a.txt"]);
        fs::remove_file(source.join("a.txt")).unwrap();

        let summary = copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(!destination.join("a.txt").exists());
        assert_eq!(summary.deleted, 1);
    }

    /// Build a repo stuck mid-merge with `UD`, `DU`, and `UU` records plus a
    /// rename/rename conflict yielding a `DD` record. Returns (repo, contents
    /// currently on disk for ud.txt/du.txt/c.txt).
    fn conflicted_repo() -> (tempfile::TempDir, String, String, String) {
        let repo = init_test_repo();
        let p = repo.path();
        fs::write(p.join("ud.txt"), "base ud\n").unwrap();
        fs::write(p.join("du.txt"), "base du\n").unwrap();
        fs::write(p.join("c.txt"), "base c\n").unwrap();
        fs::write(p.join("orig.txt"), "base orig\n").unwrap();
        commit_all(p, "base");

        run_git(p, &["switch", "-c", "theirs"]);
        run_git(p, &["rm", "ud.txt"]);
        fs::write(p.join("du.txt"), "theirs du\n").unwrap();
        fs::write(p.join("c.txt"), "theirs c\n").unwrap();
        run_git(p, &["mv", "orig.txt", "theirs.txt"]);
        commit_all(p, "theirs side");

        run_git(p, &["switch", "main"]);
        fs::write(p.join("ud.txt"), "my local edit\n").unwrap();
        run_git(p, &["rm", "du.txt"]);
        fs::write(p.join("c.txt"), "ours c\n").unwrap();
        run_git(p, &["mv", "orig.txt", "ours.txt"]);
        commit_all(p, "ours side");

        let merge = test_support::git_command()
            .args(["merge", "theirs"])
            .current_dir(p)
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge must conflict");

        let ud = fs::read_to_string(p.join("ud.txt")).unwrap();
        let du = fs::read_to_string(p.join("du.txt")).unwrap();
        let c = fs::read_to_string(p.join("c.txt")).unwrap();
        (repo, ud, du, c)
    }

    /// Catches any code-based classification of unmerged records: both `UD`
    /// and `DU` leave a file on disk (with different contents), so the copy
    /// must be decided by source disk state, never by the status code.
    #[test]
    fn copy_keeps_on_disk_files_from_modify_delete_conflicts() {
        let (repo, ud, du, c) = conflicted_repo();
        assert_eq!(ud, "my local edit\n");
        assert_eq!(du, "theirs du\n");

        let destination = tempfile::tempdir().unwrap();
        fs::write(destination.path().join("ud.txt"), "stale\n").unwrap();
        fs::write(destination.path().join("du.txt"), "stale\n").unwrap();
        fs::write(destination.path().join("c.txt"), "stale\n").unwrap();

        copy_uncommitted_changes(repo.path(), destination.path()).unwrap();

        assert_eq!(
            fs::read_to_string(destination.path().join("ud.txt")).unwrap(),
            ud
        );
        assert_eq!(
            fs::read_to_string(destination.path().join("du.txt")).unwrap(),
            du
        );
        assert_eq!(
            fs::read_to_string(destination.path().join("c.txt")).unwrap(),
            c
        );
    }

    /// The disk-state rule's delete branch: a `DD` record (rename/rename
    /// conflict on the original path) is absent on disk, so it is deleted
    /// at the destination.
    #[test]
    fn copy_deletes_both_deleted_conflict_paths() {
        let (repo, _, _, _) = conflicted_repo();
        assert!(!repo.path().join("orig.txt").exists());

        let destination = tempfile::tempdir().unwrap();
        fs::write(destination.path().join("orig.txt"), "stale\n").unwrap();

        copy_uncommitted_changes(repo.path(), destination.path()).unwrap();

        assert!(!destination.path().join("orig.txt").exists());
        // The rename/rename sides are on disk and travel.
        assert_eq!(
            fs::read_to_string(destination.path().join("ours.txt")).unwrap(),
            "base orig\n"
        );
        assert_eq!(
            fs::read_to_string(destination.path().join("theirs.txt")).unwrap(),
            "base orig\n"
        );
    }

    /// Kills two-path `R`/`C` parse corruption: hostile rename/copy detection
    /// config in the source repo must not corrupt the record stream.
    #[test]
    fn copy_is_immune_to_rename_and_copy_detection_config() {
        let repo = init_test_repo();
        fs::write(repo.path().join("a.txt"), "contents a\n").unwrap();
        fs::write(repo.path().join("b.txt"), "contents b\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        run_git(&source, &["config", "status.renames", "copies"]);
        run_git(&source, &["mv", "a.txt", "renamed.txt"]);
        // A staged copy: identical contents under a new name.
        fs::write(source.join("copied.txt"), "contents b\n").unwrap();
        run_git(&source, &["add", "copied.txt"]);

        copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(!destination.join("a.txt").exists());
        assert_eq!(
            fs::read_to_string(destination.join("renamed.txt")).unwrap(),
            "contents a\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("copied.txt")).unwrap(),
            "contents b\n"
        );
    }

    #[test]
    fn copy_deletes_removed_tracked_files_and_prunes_empty_dirs() {
        let repo = init_test_repo();
        fs::create_dir_all(repo.path().join("dir").join("sub")).unwrap();
        fs::write(repo.path().join("dir").join("sub").join("file.txt"), "x\n").unwrap();
        fs::write(repo.path().join("dir").join("keeper.txt"), "keep\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::remove_file(source.join("dir").join("sub").join("file.txt")).unwrap();

        let summary = copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(
            !destination
                .join("dir")
                .join("sub")
                .join("file.txt")
                .exists()
        );
        // `sub` became empty and is pruned; `dir` still holds keeper.txt.
        assert!(!destination.join("dir").join("sub").exists());
        assert_eq!(
            fs::read_to_string(destination.join("dir").join("keeper.txt")).unwrap(),
            "keep\n"
        );
        assert_eq!(summary.deleted, 1);
    }

    /// Replaces the mode/symlink coverage the mirror tests used to provide.
    #[test]
    fn copy_preserves_file_modes_and_symlinks() {
        let repo = init_test_repo();
        let (source, destination) = copy_test_worktrees(repo.path());

        let secret = source.join("secret.txt");
        fs::write(&secret, "shh\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(Path::new("secret.txt"), source.join("link")).unwrap();

        copy_uncommitted_changes(&source, &destination).unwrap();

        let dest_secret = fs::metadata(destination.join("secret.txt")).unwrap();
        assert_eq!(dest_secret.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::read_link(destination.join("link")).unwrap(),
            PathBuf::from("secret.txt")
        );
    }

    /// A dirty submodule surfaces as ` M smdir` (a directory on disk); the
    /// copy must skip it with a note rather than fail or bulk-copy it.
    #[test]
    fn copy_skips_dirty_submodule_without_failing() {
        let sub_origin = init_test_repo();
        fs::write(sub_origin.path().join("subfile.txt"), "sub\n").unwrap();
        commit_all(sub_origin.path(), "sub base");

        let repo = init_test_repo();
        let out = test_support::git_command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub_origin.path().to_str().unwrap(),
                "smdir",
            ])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "submodule add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        commit_all(repo.path(), "add submodule");
        // Dirty the submodule with an untracked file.
        fs::write(repo.path().join("smdir").join("dirt.txt"), "dirt\n").unwrap();

        let destination = tempfile::tempdir().unwrap();
        let summary = copy_uncommitted_changes(repo.path(), destination.path()).unwrap();

        assert!(summary.skipped_paths.iter().any(|p| p.contains("smdir")));
        assert!(!destination.path().join("smdir").exists());
    }

    /// An untracked embedded git repo collapses to `?? embedded/` even under
    /// `--untracked-files=all`; it is skipped with a note. This test also
    /// guards the `--untracked-files=all` flag itself: without it ordinary
    /// untracked dirs collapse the same way and the expansion tests fail.
    #[test]
    fn copy_skips_untracked_embedded_repo_dir() {
        let repo = init_test_repo();
        let (source, destination) = copy_test_worktrees(repo.path());

        let embedded = source.join("embedded");
        fs::create_dir(&embedded).unwrap();
        run_git(&embedded, &["init"]);
        fs::write(embedded.join("inner.txt"), "inner\n").unwrap();

        let summary = copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(summary.skipped_paths.iter().any(|p| p.contains("embedded")));
        assert!(!destination.join("embedded").exists());
    }

    /// A tracked file replaced by a FIFO is reported as an ordinary ` M`
    /// copy record, but opening a FIFO with no writer blocks forever. The
    /// copy must skip it with a note and still complete.
    #[test]
    fn copy_skips_tracked_file_replaced_by_fifo_without_hanging() {
        let repo = init_test_repo();
        fs::write(repo.path().join("f"), "regular\n").unwrap();
        commit_all(repo.path(), "base");
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::remove_file(source.join("f")).unwrap();
        let out = Command::new("mkfifo")
            .arg(source.join("f"))
            .output()
            .unwrap();
        assert!(out.status.success(), "mkfifo failed");

        let summary = copy_uncommitted_changes(&source, &destination).unwrap();

        assert!(summary.skipped_paths.iter().any(|p| p == "f"));
        // The destination is untouched: it keeps the tracked contents.
        assert_eq!(
            fs::read_to_string(destination.join("f")).unwrap(),
            "regular\n"
        );
    }

    /// Recursively snapshot every path under `root` (skipping `.git`) to its
    /// on-disk representation: file bytes, symlink target, or directory marker.
    fn snapshot_tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut snapshot = std::collections::BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() == ".git" {
                    continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.file_type().is_symlink() {
                    snapshot.insert(
                        rel,
                        fs::read_link(&path)
                            .unwrap()
                            .into_os_string()
                            .into_encoded_bytes(),
                    );
                } else if meta.is_dir() {
                    snapshot.insert(rel, b"<dir>".to_vec());
                    stack.push(path);
                } else {
                    snapshot.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        snapshot
    }

    /// Data-loss tripwire: the copy must NEVER mutate the source checkout,
    /// which holds the user's uncommitted work. Uses the richest fixture (a
    /// mid-merge tree with UD/DU/UU/DD records) and asserts the source is
    /// byte-identical afterwards.
    #[test]
    fn copy_never_mutates_the_source_checkout() {
        let (repo, _, _, _) = conflicted_repo();
        let before = snapshot_tree(repo.path());
        assert!(!before.is_empty());

        let destination = tempfile::tempdir().unwrap();
        copy_uncommitted_changes(repo.path(), destination.path()).unwrap();

        let after = snapshot_tree(repo.path());
        assert_eq!(before, after, "the source checkout must be untouched");
    }

    /// Kills line-based or quote-unaware status parsing.
    #[test]
    fn copy_handles_paths_with_spaces_quotes_and_unicode() {
        let repo = init_test_repo();
        let (source, destination) = copy_test_worktrees(repo.path());

        fs::write(source.join("my file \"quoted\".txt"), "spaces\n").unwrap();
        fs::write(source.join("日本語 ファイル.txt"), "unicode\n").unwrap();
        fs::create_dir(source.join("dir with space")).unwrap();
        fs::write(
            source.join("dir with space").join("inner file.txt"),
            "nested\n",
        )
        .unwrap();

        copy_uncommitted_changes(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("my file \"quoted\".txt")).unwrap(),
            "spaces\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("日本語 ファイル.txt")).unwrap(),
            "unicode\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("dir with space").join("inner file.txt")).unwrap(),
            "nested\n"
        );
    }

    #[test]
    fn copy_rejects_same_source_and_destination() {
        let repo = init_test_repo();
        assert!(copy_uncommitted_changes(repo.path(), repo.path()).is_err());
    }

    #[test]
    fn has_origin_remote_true_with_bare_remote_false_without() {
        let repo = init_test_repo();
        assert!(!has_origin_remote(repo.path()).unwrap());

        let bare = tempfile::tempdir().unwrap();
        run_git(bare.path(), &["init", "--bare", "-b", "main"]);
        run_git(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        assert!(has_origin_remote(repo.path()).unwrap());
    }

    // ── rename_branch tests ──────────────────────────────────────

    #[test]
    fn rename_branch_succeeds() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "old-name");

        rename_branch(&wt, "old-name", "new-name").unwrap();

        let branch = current_branch(&wt).unwrap();
        assert_eq!(branch, "new-name");
    }

    #[test]
    fn rename_branch_fails_on_conflict() {
        let repo = init_test_repo();
        // Create two worktrees with different branches.
        let wt1 = add_worktree(repo.path(), "branch-a");
        let _wt2 = add_worktree(repo.path(), "branch-b");

        // Trying to rename branch-a to branch-b should fail because
        // branch-b already exists.
        let result = rename_branch(&wt1, "branch-a", "branch-b");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("rename failed"),
            "error should mention rename failure"
        );

        // The original branch should be unchanged.
        let branch = current_branch(&wt1).unwrap();
        assert_eq!(branch, "branch-a");
    }

    #[test]
    fn rename_branch_fails_on_invalid_name() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "valid-name");

        // Git rejects branch names with spaces and other invalid characters.
        let result = rename_branch(&wt, "valid-name", "has spaces");
        assert!(result.is_err());

        // Original branch should still be intact.
        let branch = current_branch(&wt).unwrap();
        assert_eq!(branch, "valid-name");
    }

    #[test]
    fn rename_branch_fails_when_old_name_wrong() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "real-branch");

        // Renaming a nonexistent branch should fail.
        let result = rename_branch(&wt, "nonexistent", "new-name");
        assert!(result.is_err());

        // The real branch should be unaffected.
        let branch = current_branch(&wt).unwrap();
        assert_eq!(branch, "real-branch");
    }

    #[test]
    fn rename_branch_noop_same_name() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "same-name");

        // Renaming to the same name should succeed (git allows this).
        rename_branch(&wt, "same-name", "same-name").unwrap();

        let branch = current_branch(&wt).unwrap();
        assert_eq!(branch, "same-name");
    }

    #[test]
    fn wait_child_or_kill_times_out_and_kills_a_wedged_child() {
        // A hung child must be killed once the deadline passes, and the error
        // must say so — this is what keeps a wedged `git branch -m` from
        // stranding the rename worker forever.
        use std::time::{Duration, Instant};
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let start = Instant::now();
        let err = wait_child_or_kill(&mut child, Duration::from_millis(100), "sleep").unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must return promptly after the timeout, not wait for the child"
        );
        assert!(
            err.to_string().contains("timed out"),
            "error should report the timeout, got: {err}"
        );
        // The child was killed and reaped: a follow-up try_wait sees it gone.
        assert!(
            child.try_wait().unwrap().is_some(),
            "the timed-out child must have been reaped, not left a zombie"
        );
    }

    #[test]
    fn wait_child_or_kill_returns_status_for_a_fast_child() {
        use std::time::Duration;
        let mut child = Command::new("true").spawn().unwrap();
        let status = wait_child_or_kill(&mut child, Duration::from_secs(5), "true").unwrap();
        assert!(status.success());
    }

    // ── branch_exists tests ────────────────────────────────────

    #[test]
    fn branch_exists_returns_none_for_nonexistent() {
        let repo = init_test_repo();
        assert_eq!(branch_exists(repo.path(), "no-such-branch"), None);
    }

    #[test]
    fn branch_exists_returns_local_for_existing_branch() {
        let repo = init_test_repo();
        let _wt = add_worktree(repo.path(), "feature-x");
        // "feature-x" now exists as a local branch.
        assert_eq!(
            branch_exists(repo.path(), "feature-x"),
            Some(BranchLocation::Local)
        );
    }

    /// #10: the create-agent branch preflight (the single-source decision both
    /// surfaces consume): a name matching no existing branch is Fresh, and a
    /// name matching a local branch is ExistingBranch (so the surface can ask
    /// for consent before attaching to that branch's history).
    #[test]
    fn create_agent_branch_preflight_reports_fresh_for_a_new_name() {
        let repo = init_test_repo();
        assert_eq!(
            create_agent_branch_preflight(repo.path(), "brand-new"),
            CreateAgentBranchPlan::Fresh
        );
    }

    #[test]
    fn create_agent_branch_preflight_reports_existing_branch_with_location() {
        let repo = init_test_repo();
        let _wt = add_worktree(repo.path(), "feature-x");
        assert_eq!(
            create_agent_branch_preflight(repo.path(), "feature-x"),
            CreateAgentBranchPlan::ExistingBranch {
                location: BranchLocation::Local
            }
        );
    }

    // ── create_worktree_existing_branch tests ────────────────

    #[test]
    fn create_worktree_existing_branch_succeeds_for_local_branch() {
        let repo = init_test_repo();
        // Create a branch without a worktree that points to it.
        run_git(repo.path(), &["branch", "reuse-me"]);
        let worktrees_root = repo.path().join("wt-root");
        let (name, path) =
            create_worktree_existing_branch(repo.path(), &worktrees_root, "proj", "reuse-me")
                .unwrap();
        assert_eq!(name, "reuse-me");
        assert!(path.exists());
        assert_eq!(current_branch(&path).unwrap(), "reuse-me");
    }

    // ── refnames are positionals, not options ────────────────
    //
    // Every git subcommand below takes a refname in a positional slot. Without a
    // `--` separator git reads a leading-dash argument as an option, so a
    // refname that looks like one is silently obeyed as a flag instead of being
    // rejected as the ref it is. Git's own `check-ref-format` blocks a
    // dash-LEADING branch through `git branch`, but plumbing (`update-ref`)
    // creates such a ref outright, and a non-leading component may begin with a
    // dash (`foo/-bar`) through the porcelain, so these names do reach dux.
    // Each test below pins the corrected reading: the argument is a REF.

    /// Unlike the three below, this one passed before the `--` was added, and
    /// the reason is worth writing down rather than leaving for someone to
    /// rediscover. In isolation `git worktree add <path> --force` really does
    /// obey the flag and check out HEAD. At THIS call shape it cannot, because
    /// the worktree path is derived from the same string, so the branch git
    /// then infers from the path's last component is itself `--force`, which
    /// `check-ref-format` refuses. Every option-looking name was measured
    /// against this shape and all of them fail one way or another. The `--` is
    /// therefore defence in depth here, and this test pins the reading so a
    /// later change that decouples the path from the branch cannot quietly
    /// re-open the door.
    #[test]
    fn create_worktree_existing_branch_reads_an_option_looking_branch_as_a_ref() {
        let repo = init_test_repo();
        let worktrees_root = repo.path().join("wt-root");
        let result =
            create_worktree_existing_branch(repo.path(), &worktrees_root, "proj", "--force");
        assert!(
            result.is_err(),
            "an option-looking branch must be refused, not obeyed as a flag: {result:?}"
        );
        assert!(
            !worktrees_root.join("proj").join("--force").exists(),
            "no worktree should have been created"
        );
    }

    #[test]
    fn switch_branch_reads_an_option_looking_branch_as_a_ref() {
        let repo = init_test_repo();
        // Without `--`, `git switch --detach` detaches HEAD instead of failing.
        let result = switch_branch(repo.path(), "--detach");
        assert!(result.is_err(), "expected a refused switch: {result:?}");
        assert_eq!(
            current_branch(repo.path()).unwrap(),
            "main",
            "HEAD must not have been detached"
        );
    }

    #[test]
    fn remove_worktree_deletes_a_branch_whose_name_looks_like_an_option() {
        let repo = init_test_repo();
        // `git branch` refuses to create a dash-leading name, but plumbing does
        // not, and such a ref is what dux would then be asked to clean up.
        run_git(repo.path(), &["update-ref", "refs/heads/--delete", "HEAD"]);
        let result = remove_worktree(repo.path(), &repo.path().join("gone"), "--delete").unwrap();
        assert!(
            !result.branch_already_deleted,
            "the branch should have been deleted, not read as the --delete flag"
        );
        let listed = std::process::Command::new("git")
            .args(["-C", repo.path().to_string_lossy().as_ref(), "branch"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&listed.stdout).contains("--delete"),
            "the ref should be gone"
        );
    }

    #[test]
    fn push_reads_an_option_looking_branch_as_a_ref_and_pushes_nothing_else() {
        let repo = init_test_repo();
        let remote = repo.path().join("remote.git");
        run_git(
            repo.path(),
            &["init", "--bare", remote.to_string_lossy().as_ref()],
        );
        run_git(
            repo.path(),
            &["remote", "add", "origin", remote.to_string_lossy().as_ref()],
        );
        run_git(repo.path(), &["branch", "unrelated"]);
        // A branch named `--all` reaches `push` through current_branch_opt, and
        // without `--` git reads it as the flag that pushes EVERY branch.
        run_git(repo.path(), &["update-ref", "refs/heads/--all", "HEAD"]);
        run_git(repo.path(), &["symbolic-ref", "HEAD", "refs/heads/--all"]);

        let _ = push(repo.path());

        let heads = std::process::Command::new("git")
            .args([
                "-C",
                remote.to_string_lossy().as_ref(),
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads",
            ])
            .output()
            .unwrap();
        let heads = String::from_utf8_lossy(&heads.stdout);
        assert!(
            !heads.contains("unrelated"),
            "push must not have fanned out to every branch: {heads}"
        );
    }

    #[test]
    fn create_worktree_existing_branch_fails_when_already_checked_out() {
        let repo = init_test_repo();
        let _wt = add_worktree(repo.path(), "occupied");
        // "occupied" is checked out in _wt — git forbids a second worktree.
        let worktrees_root = repo.path().join("wt-root");
        let result =
            create_worktree_existing_branch(repo.path(), &worktrees_root, "proj", "occupied");
        assert!(result.is_err());
    }

    #[test]
    fn changed_files_expands_untracked_directories_into_files() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "changes-pane-folder");

        let nested = wt.join("new-folder").join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            wt.join("new-folder").join("one.txt"),
            "first line\nsecond line\n",
        )
        .unwrap();
        fs::write(nested.join("two.txt"), "nested line\n").unwrap();

        let (_staged, unstaged) = changed_files(&wt).unwrap();
        let mut actual: Vec<_> = unstaged
            .into_iter()
            .map(|file| {
                (
                    file.path,
                    file.status,
                    file.additions,
                    file.deletions,
                    file.binary,
                )
            })
            .collect();
        actual.sort();

        assert_eq!(
            actual,
            vec![
                (
                    "new-folder/nested/two.txt".to_string(),
                    "?".to_string(),
                    1,
                    0,
                    false,
                ),
                (
                    "new-folder/one.txt".to_string(),
                    "?".to_string(),
                    2,
                    0,
                    false
                ),
            ]
        );
    }

    #[test]
    fn staged_diff_text_returns_diff_for_staged_changes() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "staged-diff");
        fs::write(wt.join("hello.txt"), "hello world\n").unwrap();
        let run = |args: &[&str]| {
            let out = test_support::git_command()
                .args(args)
                .current_dir(&wt)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["add", "hello.txt"]);
        let diff = staged_diff_text(&wt).unwrap();
        assert!(diff.contains("hello.txt"), "diff should mention the file");
        assert!(
            diff.contains("+hello world"),
            "diff should contain the added line"
        );
    }

    #[test]
    fn staged_diff_text_empty_when_nothing_staged() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "no-staged");
        let diff = staged_diff_text(&wt).unwrap();
        assert!(
            diff.is_empty(),
            "diff should be empty when nothing is staged"
        );
    }

    #[test]
    fn changed_files_marks_untracked_binary_files() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "changes-pane-binary");

        fs::write(wt.join("image.bin"), [0_u8, 159, 146, 150]).unwrap();

        let (_staged, unstaged) = changed_files(&wt).unwrap();
        assert_eq!(unstaged.len(), 1);
        let file = &unstaged[0];
        assert_eq!(file.path, "image.bin");
        assert_eq!(file.status, "?");
        assert_eq!(file.additions, 0);
        assert_eq!(file.deletions, 0);
        assert!(file.binary);
    }

    #[test]
    fn parse_status_porcelain_z_handles_untracked_and_spaces() {
        let raw = b"?? spaced name.txt\0";
        let entries = parse_status_porcelain_z(raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index_status, '?');
        assert_eq!(entries[0].worktree_status, '?');
        assert_eq!(entries[0].path, "spaced name.txt");
    }

    #[test]
    fn parse_status_porcelain_z_uses_destination_path_for_renames() {
        let raw = b"R  new name.txt\0old name.txt\0";
        let entries = parse_status_porcelain_z(raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index_status, 'R');
        assert_eq!(entries[0].worktree_status, ' ');
        assert_eq!(entries[0].path, "new name.txt");
    }

    #[test]
    fn parse_status_porcelain_z_skips_empty_records() {
        let raw = b"\0M  file.txt\0\0";
        let entries = parse_status_porcelain_z(raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "file.txt");
    }

    #[test]
    fn parse_numstat_handles_regular_path_with_spaces() {
        let stats = parse_numstat(b"1\t2\tsp ace.txt\0");
        let stat = stats.get("sp ace.txt").expect("stat present");
        match stat {
            DiffStat::Text(additions, deletions) => {
                assert_eq!((*additions, *deletions), (1, 2));
            }
            DiffStat::Binary => panic!("expected text stat"),
        }
    }

    #[test]
    fn parse_numstat_handles_rename_records() {
        let stats = parse_numstat(b"0\t0\t\0old name.txt\0new name.txt\0");
        let stat = stats.get("new name.txt").expect("stat present");
        match stat {
            DiffStat::Text(additions, deletions) => {
                assert_eq!((*additions, *deletions), (0, 0));
            }
            DiffStat::Binary => panic!("expected text stat"),
        }
    }

    #[test]
    fn parse_numstat_handles_binary_records() {
        let stats = parse_numstat(b"-\t-\tbinary.bin\0");
        assert!(matches!(stats.get("binary.bin"), Some(DiffStat::Binary)));
    }

    #[test]
    fn parse_status_porcelain_z_skips_non_utf8_paths() {
        // 0xFF is invalid as a UTF-8 start byte. Lossy conversion would
        // produce a U+FFFD-substituted string that no longer matches the
        // real on-disk file when used as a stage/discard identifier.
        let raw: &[u8] = b"M  good.txt\0?? \xFFbad.txt\0M  also-good.txt\0";
        let entries = parse_status_porcelain_z(raw);

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["good.txt", "also-good.txt"]);
    }

    #[test]
    fn parse_status_porcelain_z_keeps_iterator_aligned_after_invalid_rename() {
        // A rename whose destination path is not UTF-8 must still consume
        // its trailing source-path record so the next status entry parses
        // at the correct position.
        let raw: &[u8] = b"R  \xFFnew.txt\0old.txt\0M  next.txt\0";
        let entries = parse_status_porcelain_z(raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "next.txt");
        assert_eq!(entries[0].index_status, 'M');
    }

    #[test]
    fn parse_numstat_skips_non_utf8_paths() {
        // Path bytes after the second tab include 0xFF which is invalid UTF-8.
        // Without strict parsing, the lookup key would be a corrupted string
        // that would never match downstream `file.path` comparisons.
        let stats = parse_numstat(b"1\t2\t\xFFbad.txt\0");
        assert!(stats.is_empty());
    }

    #[test]
    fn changed_files_uses_destination_path_for_staged_rename() {
        let repo = init_test_repo();
        let wt = add_worktree(repo.path(), "rename-status");

        fs::write(wt.join("old name.txt"), "hello\n").unwrap();
        run_git(&wt, &["add", "old name.txt"]);
        run_git(&wt, &["commit", "-m", "add file"]);
        run_git(&wt, &["mv", "old name.txt", "new name.txt"]);

        let (staged, unstaged) = changed_files(&wt).unwrap();

        assert!(unstaged.is_empty());
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "new name.txt");
        assert_eq!(staged[0].status, "R");
    }

    #[test]
    fn valid_agent_names() {
        assert!(is_valid_agent_name("foo"));
        assert!(is_valid_agent_name("foo-bar"));
        assert!(is_valid_agent_name("foo_bar"));
        assert!(is_valid_agent_name("foo/bar"));
        assert!(is_valid_agent_name("ABC123"));
        assert!(is_valid_agent_name("feature/my-branch_v2"));
    }

    #[test]
    fn invalid_agent_names() {
        assert!(!is_valid_agent_name(""));
        assert!(!is_valid_agent_name("foo bar"));
        assert!(!is_valid_agent_name("foo@bar"));
        assert!(!is_valid_agent_name("-foo"));
        assert!(!is_valid_agent_name("foo/"));
        assert!(!is_valid_agent_name("/foo"));
        assert!(!is_valid_agent_name("foo//bar"));
        assert!(!is_valid_agent_name("foo.bar"));
        assert!(!is_valid_agent_name("foo..bar"));
        assert!(!is_valid_agent_name("hello world!"));
    }

    #[test]
    fn create_worktree_uses_custom_name() {
        let repo = init_test_repo();
        let worktrees_root = repo.path().join("agents");
        let (branch, path) = create_worktree_from_start_point(
            repo.path(),
            &worktrees_root,
            "proj",
            None,
            Some("my-agent"),
        )
        .unwrap();
        assert_eq!(branch, "my-agent");
        assert!(path.ends_with("proj/my-agent"));
        assert!(path.exists());
    }

    #[test]
    fn create_worktree_generates_name_when_none() {
        let repo = init_test_repo();
        let worktrees_root = repo.path().join("agents");
        let (branch, path) =
            create_worktree_from_start_point(repo.path(), &worktrees_root, "proj", None, None)
                .unwrap();
        // Auto-generated names contain a dash (docker-style petname).
        assert!(branch.contains('-'), "expected dash in '{branch}'");
        assert!(path.exists());
    }

    #[test]
    fn create_worktree_from_start_point_uses_custom_name() {
        let repo = init_test_repo();
        let source = add_worktree(repo.path(), "src-branch");
        fs::write(source.join("marker.txt"), "data\n").unwrap();
        commit_all(&source, "add marker");
        let source_head = head_commit(&source).unwrap();

        let worktrees_root = repo.path().join("forks");
        let (branch, forked) = create_worktree_from_start_point(
            repo.path(),
            &worktrees_root,
            "proj",
            Some(&source_head),
            Some("my-fork"),
        )
        .unwrap();

        assert_eq!(branch, "my-fork");
        assert!(forked.ends_with("proj/my-fork"));
        assert_eq!(head_commit(&forked).unwrap(), source_head);
    }

    #[test]
    fn create_worktree_from_start_point_uses_named_base_branch() {
        let repo = init_test_repo();
        let feature = add_worktree(repo.path(), "feature");
        fs::write(feature.join("feature.txt"), "feature\n").unwrap();
        commit_all(&feature, "add feature marker");

        let main_head = head_commit(repo.path()).unwrap();
        let worktrees_root = repo.path().join("agents");
        let (_branch, agent) = create_worktree_from_start_point(
            repo.path(),
            &worktrees_root,
            "proj",
            Some("main"),
            Some("agent-from-main"),
        )
        .unwrap();

        assert_eq!(head_commit(&agent).unwrap(), main_head);
        assert!(!agent.join("feature.txt").exists());
    }

    // ── agent_name_char_map tests ───────────────────────────────

    #[test]
    fn agent_map_allows_valid_chars() {
        assert_eq!(agent_name_char_map("a", 1, 'b'), Some('b'));
        assert_eq!(agent_name_char_map("a", 1, '0'), Some('0'));
        assert_eq!(agent_name_char_map("a", 1, '-'), Some('-'));
        assert_eq!(agent_name_char_map("a", 1, '_'), Some('_'));
        assert_eq!(agent_name_char_map("a", 1, '/'), Some('/'));
    }

    #[test]
    fn agent_map_rejects_invalid_chars() {
        assert_eq!(agent_name_char_map("a", 1, '@'), None);
        assert_eq!(agent_name_char_map("a", 1, '.'), None);
        assert_eq!(agent_name_char_map("a", 1, '!'), None);
        assert_eq!(agent_name_char_map("a", 1, '#'), None);
    }

    #[test]
    fn agent_map_converts_space_to_dash() {
        assert_eq!(agent_name_char_map("a", 1, ' '), Some('-'));
    }

    #[test]
    fn agent_map_rejects_space_at_position_zero() {
        // Space maps to dash, but dash is rejected at position 0.
        assert_eq!(agent_name_char_map("", 0, ' '), None);
    }

    #[test]
    fn agent_map_first_char_must_be_alphanumeric() {
        // Rejected at position 0
        assert_eq!(agent_name_char_map("", 0, '-'), None);
        assert_eq!(agent_name_char_map("", 0, '_'), None);
        assert_eq!(agent_name_char_map("", 0, '/'), None);
        // Accepted at position 0
        assert_eq!(agent_name_char_map("", 0, 'a'), Some('a'));
        assert_eq!(agent_name_char_map("", 0, '1'), Some('1'));
        // Also rejected when inserting at position 0 in non-empty text
        assert_eq!(agent_name_char_map("abc", 0, '-'), None);
    }

    #[test]
    fn agent_map_prevents_double_slash() {
        // Inserting '/' right after an existing '/'
        assert_eq!(agent_name_char_map("a/", 2, '/'), None);
        // Inserting '/' right before an existing '/'
        assert_eq!(agent_name_char_map("a/b", 1, '/'), None);
        // Inserting '/' where no adjacent slash exists
        assert_eq!(agent_name_char_map("ab", 1, '/'), Some('/'));
    }

    #[test]
    fn parse_github_ssh_url() {
        assert_eq!(
            parse_github_owner_repo("git@github.com:octocat/Hello-World.git"),
            Some("octocat/Hello-World".to_string()),
        );
    }

    #[test]
    fn parse_github_ssh_url_no_git_suffix() {
        assert_eq!(
            parse_github_owner_repo("git@github.com:octocat/Hello-World"),
            Some("octocat/Hello-World".to_string()),
        );
    }

    #[test]
    fn parse_github_https_url() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/octocat/Hello-World.git"),
            Some("octocat/Hello-World".to_string()),
        );
    }

    #[test]
    fn parse_github_https_url_no_git_suffix() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/octocat/Hello-World"),
            Some("octocat/Hello-World".to_string()),
        );
    }

    #[test]
    fn parse_github_url_non_github() {
        assert_eq!(
            parse_github_owner_repo("git@gitlab.com:owner/repo.git"),
            None,
        );
    }

    /// This test used to assert the opposite: that `/tree/main` was stripped and
    /// the remote answered `octocat/Hello-World`. It was changed deliberately.
    /// The input to this parser comes only from `git remote get-url`, never from
    /// a browser address bar, and to git the whole path is the repository path,
    /// so `/octocat/Hello-World/tree/main` addresses a repository that is not
    /// `octocat/Hello-World`. A remote is not a browser URL, and a wrong
    /// repository name (handed to `gh` as `--repo`) is worse than no repository
    /// name.
    #[test]
    fn parse_github_url_rejects_extra_path_segments_on_every_family() {
        for url in [
            "https://github.com/octocat/Hello-World/tree/main",
            "http://github.com/octocat/Hello-World/tree/main",
            "github.com:octocat/Hello-World/tree/main",
            "ssh://github.com/octocat/Hello-World/tree/main",
            "git@github.com:octocat/Hello-World/tree/main",
        ] {
            assert_eq!(parse_github_owner_repo(url), None, "{url}");
        }
    }

    /// The `url` crate follows the WHATWG URL spec, which DELETES every
    /// embedded tab, newline and carriage return before parsing. Git does no
    /// such thing, so `ssh://git<LF>hub.com/o/r` is a host git would never call
    /// GitHub, and normalising it into `github.com` would MANUFACTURE a match
    /// and query GitHub about a repository from a remote that is not GitHub.
    #[test]
    fn parse_github_remote_rejects_embedded_control_characters() {
        for url in [
            "ssh://git\nhub.com/octocat/Hello-World",
            "ssh://git\thub.com/octocat/Hello-World",
            "ssh://git\rhub.com/octocat/Hello-World",
            "https://git\nhub.com/octocat/Hello-World",
            "git@git\nhub.com:octocat/Hello-World",
            "git\nhub.com:octocat/Hello-World",
            "ssh://github.com/octocat/Hello\u{7f}World",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url:?}");
        }
    }

    /// The parser consumes its input EXACTLY. It used to trim leading and
    /// trailing whitespace first, which MANUFACTURED a match: a remote really
    /// can hold edge whitespace, and `" ssh://github.com/o/r "` is not a GitHub
    /// remote, it is a remote whose host git would look up with a space in it.
    #[test]
    fn parse_github_remote_consumes_its_input_exactly() {
        for url in [
            " ssh://github.com/octocat/Hello-World",
            "ssh://github.com/octocat/Hello-World ",
            " ssh://github.com/octocat/Hello-World ",
            "\tssh://github.com/octocat/Hello-World",
            "ssh://github.com/octocat/Hello-World\t",
            " git@github.com:octocat/Hello-World.git",
            "github.com:octocat/Hello-World ",
            "",
            " ",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url:?}");
        }
    }

    /// The one place trimming is legitimate is the process boundary, and only
    /// for what git actually appends: a single output record terminator, which
    /// on this project's platforms (macOS and Linux, Unix throughout) is
    /// exactly one `\n` and nothing else. Anything else in git's output is part
    /// of the remote and must reach the parser intact.
    #[test]
    fn github_remote_from_git_output_removes_only_the_record_terminator() {
        let expected = Some(GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        for stdout in [
            "ssh://github.com/octocat/Hello-World.git\n",
            // git's output is read the same way when the terminator is absent.
            "ssh://github.com/octocat/Hello-World.git",
        ] {
            assert_eq!(
                github_remote_from_git_output(stdout.as_bytes()),
                expected,
                "{stdout:?}",
            );
        }
        for stdout in [
            // A space before the terminator is part of the remote.
            "ssh://github.com/octocat/Hello-World.git \n",
            "ssh://github.com/octocat/Hello-World.git\t\n",
            " ssh://github.com/octocat/Hello-World.git\n",
            // Only ONE terminator comes off; the rest is a control character.
            "ssh://github.com/octocat/Hello-World.git\n\n",
            "ssh://github.com/octocat/Hello-World.git\r\r\n",
            // A carriage return before the terminator is DATA, not part of the
            // terminator. This used to be pinned the other way round, as a
            // remote resolving to `octocat/Hello-World`, and it passed for an
            // ambiguous reason: it read the `\r\n` as a CRLF line ending, which
            // git does not write on macOS or Linux and which this project has
            // no platform to receive. What really produces this output is a
            // remote whose own path ends in a carriage return (an
            // `url.*.insteadOf` replacement ending in one is enough), where git
            // appended only the `\n`. Stripping the `\r` too would delete a
            // byte of the remote and answer for an address nobody wrote, so
            // exactly one `\n` comes off and the surviving control character is
            // refused like any other.
            "ssh://github.com/octocat/Hello-World.git\r\n",
        ] {
            assert_eq!(
                github_remote_from_git_output(stdout.as_bytes()),
                None,
                "{stdout:?}"
            );
        }
        // Bytes that are not UTF-8 are not lossily substituted into a name: a
        // replacement character is neither a control nor whitespace, so it used
        // to survive into a `--repo` argument.
        assert_eq!(
            github_remote_from_git_output(b"ssh://github.com/octocat/Hello\xffWorld\n"),
            None,
        );
    }

    /// The `url` crate treats a backslash as a path separator under http(s), so
    /// the crate and the hand-written raw-path scan disagreed about where the
    /// path starts: for `https://github.com\ignored/o/r` the crate reported the
    /// host `github.com` while the raw scan skipped `\ignored` and answered
    /// `o/r`. Two parsers disagreeing about component boundaries is the whole
    /// hazard, and neither a GitHub owner nor a repository name can hold a
    /// backslash, so one is refused outright wherever it appears.
    #[test]
    fn parse_github_remote_rejects_a_raw_backslash() {
        for url in [
            r"https://github.com\ignored/octocat/Hello-World",
            r"http://github.com\ignored/octocat/Hello-World",
            r"ssh://github.com\ignored/octocat/Hello-World",
            r"ssh://github.com/octocat\Hello-World",
            r"git@github.com:octocat\Hello-World.git",
            r"github.com:octocat\Hello-World",
            r"C:\repo",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// A decoded slash is refused wherever it appears, and the boundaries are
    /// the place that took two goes to get right: the ONE syntactic leading
    /// slash used to be removed by trimming EVERY slash, which erased a decoded
    /// one sitting next to it, so `/%2Fo/r` answered `o/r`. The syntactic
    /// slashes now come off BEFORE decoding, exactly one at each end, and each
    /// remaining raw segment is decoded on its own, so a decoded slash cannot
    /// be trimmed away and cannot pass as a separator either.
    #[test]
    fn parse_github_remote_refuses_decoded_separators_at_the_path_boundaries() {
        for url in [
            // A decoded leading separator is not the syntactic one.
            "ssh://github.com/%2Foctocat/Hello-World",
            "https://github.com/%2Foctocat/Hello-World",
            // A decoded trailing separator is not the trailing raw slash the
            // parser tolerates.
            "ssh://github.com/octocat/Hello-World%2F",
            "https://github.com/octocat/Hello-World.git%2F",
            // And one in the middle, which used to be read as a separator.
            "ssh://github.com/octocat/Hello-World%2Fextra",
            // An empty component is an empty component however it arrives.
            "ssh://github.com/octocat/Hello-World//",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
        // One trailing RAW slash is still tolerated, and still comes off before
        // the `.git` suffix is matched.
        for url in [
            "ssh://github.com/octocat/Hello-World.git/",
            "https://github.com/octocat/Hello-World.git/",
            "github.com:octocat/Hello-World.git/",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                }),
                "{url}",
            );
        }
    }

    /// An ssh or git port is the ssh service's port, not the API's, so dropping
    /// it is right. An http(s) port is part of the server endpoint, and `gh`
    /// cannot express one at all: it refuses a colon in a hostname and builds
    /// fixed API URLs. Keeping the host and discarding the port would send the
    /// query to a DIFFERENT server than the remote names, so it is refused. A
    /// port written out that is the scheme's own default names no other server
    /// and is accepted.
    #[test]
    fn parse_github_remote_rejects_a_non_default_http_port() {
        let github = Some(GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        for url in [
            "https://github.com:443/octocat/Hello-World.git",
            "http://github.com:80/octocat/Hello-World.git",
            // ssh and git transports keep dropping their port.
            "ssh://git@github.com:2222/octocat/Hello-World.git",
            "git://github.com:9418/octocat/Hello-World.git",
            "git+ssh://github.com:2222/octocat/Hello-World.git",
        ] {
            assert_eq!(parse_github_remote(url), github, "{url}");
        }
        for url in [
            "https://github.com:8443/octocat/Hello-World.git",
            "http://github.com:8080/octocat/Hello-World.git",
            "https://github.com:80/octocat/Hello-World.git",
            "http://github.com:443/octocat/Hello-World.git",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// `Url::path()` drops `?query` and `#fragment`, and canonicalises `.` and
    /// `..` segments. For a git remote none of that is true: those characters
    /// and segments are ordinary parts of the repository path, so the answer
    /// would silently name a DIFFERENT repository. dux reads the raw path out of
    /// the input instead of the parser's normalised one.
    #[test]
    fn parse_github_remote_does_not_let_the_url_parser_retarget_the_repository() {
        for url in [
            // `?`/`#` are part of the path to git, so these are not `o/r`.
            "ssh://github.com/octocat/Hello-World?x",
            "ssh://github.com/octocat/Hello-World#x",
            "https://github.com/octocat/Hello-World?x",
            // Dot segments are ordinary path components to git; the parser
            // would collapse these onto a repository the remote never named.
            "ssh://github.com/octocat/../Hello-World/x",
            "ssh://github.com/octocat/Hello-World/../../a/b",
            "ssh://github.com/./octocat/Hello-World/x",
            "ssh://github.com/octocat/%2E%2E/Hello-World/x",
            "https://github.com/octocat/../Hello-World/x",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// A `.` or `..` component is not a repository, in any family, however it is
    /// spelled.
    #[test]
    fn parse_github_remote_rejects_dot_path_components() {
        for url in [
            "ssh://github.com/octocat/..",
            "ssh://github.com/../Hello-World",
            "ssh://github.com/octocat/%2E%2E",
            "ssh://github.com/octocat/.",
            "github.com:octocat/..",
            "github.com:../Hello-World",
            "ssh://github.com/octocat/...git",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// Percent-decoding happens after parsing, so a decoded control character
    /// lands inside the owner or repository name and is handed straight to `gh`
    /// as a `--repo` argument. Emptiness was the only thing checked.
    #[test]
    fn parse_github_remote_rejects_decoded_control_characters_and_whitespace() {
        for url in [
            "ssh://github.com/octocat/Hello%00World",
            "ssh://github.com/octocat/Hello%0AWorld",
            "ssh://github.com/octo%09cat/Hello-World",
            "https://github.com/octocat/Hello%0DWorld",
            // C1 controls, which arrive as two percent-encoded UTF-8 bytes.
            "ssh://github.com/octocat/Hello%C2%85World",
            // Whitespace is not a GitHub name character either, and a space in
            // a command argument is its own hazard.
            "ssh://github.com/octocat/Hello%20World",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// `git+ssh://` and `ssh+git://` are valid git URL schemes that invoke SSH
    /// exactly as `ssh://` does, so they parse the same way.
    #[test]
    fn parse_github_remote_accepts_the_ssh_scheme_aliases() {
        for url in [
            "git+ssh://github.com/octocat/Hello-World.git",
            "ssh+git://git@github.com/octocat/Hello-World.git",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                }),
                "{url}",
            );
        }
        // The ssh aliases are the whole of the addition: ftp/ftps stay refused.
        assert_eq!(
            parse_github_remote("ftps://github.com/octocat/Hello-World"),
            None,
        );
    }

    /// A percent-encoded authority moves the boundary git splits the address
    /// on for the ssh-style transports, and the parsed answer cannot show it.
    ///
    /// Measured with a stub `GIT_SSH_COMMAND` that prints the arguments git
    /// hands ssh. For `ssh://user%2F@github.com/octocat/Hello-World.git` git
    /// runs `ssh user git-upload-pack '/@github.com/octocat/Hello-World.git'`:
    /// it decodes first and splits afterwards, so the host is `user` and the
    /// repository path is the rest. `ssh://git%2Fhub.com/o/r` becomes host
    /// `git`, path `/hub.com/o/r` the same way. The native protocol behaves
    /// identically: `GIT_TRACE=1 git ls-remote git://us%2Fer@host.invalid/o/r`
    /// reports `unable to look up us (port 9418)`. The `url` crate reports
    /// `github.com` for all of them, and the decoded-slash check only ever sees
    /// the path, so dux answered with a repository on a host the remote does
    /// not address.
    ///
    /// This covers the ssh-style transports ONLY. See the companion test for
    /// http(s), where the same shape is harmless and is accepted.
    #[test]
    fn parse_github_remote_refuses_a_percent_encoded_authority_on_ssh_style_transports() {
        for url in [
            // The encoded slash sits in the user part.
            "ssh://user%2F@github.com/octocat/Hello-World.git",
            "ssh://user%2F@github.com/o/r.git",
            "git+ssh://user%2F@github.com/o/r",
            "ssh+git://user%2F@github.com/o/r",
            "git://user%2F@github.com/o/r",
            "git://user%2F@github.com/o/r.git",
            // And in the host itself.
            "ssh://git%2Fhub.com/o/r",
            "ssh://git@git%2Fhub.com/o/r",
            // An encoding that decodes to nothing structural is refused too:
            // dux cannot reproduce git's decode-then-split order from the
            // crate's already-split output, so it declines to guess at all.
            "ssh://git%40github.com/o/r",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
        // The ordinary spelling is untouched.
        assert_eq!(
            parse_github_remote("ssh://git@github.com/o/r.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
            }),
        );
    }

    /// Under http and https a percent in the authority is harmless, so it is
    /// accepted. This is the asymmetry, and it is measured rather than assumed.
    ///
    /// Git hands an http(s) remote to curl, which separates the authority from
    /// the path FIRST and decodes each piece afterwards, the opposite order
    /// from the ssh-style transports. So the escape never moves the boundary:
    ///
    /// ```text
    /// git ls-remote 'https://user%2Fx@nonexistent-host.invalid/o/r'
    ///   -> unable to access 'https://nonexistent-host.invalid/o/r/'
    /// git ls-remote 'https://u:p%40ss@nonexistent-host.invalid/o/r'
    ///   -> unable to access 'https://nonexistent-host.invalid/o/r/'
    /// ```
    ///
    /// Both reach the host the address names, with the path the address names,
    /// which is exactly what the `url` crate reports. Refusing these was the
    /// cost of the ssh rule being applied to every scheme, and it refused an
    /// ordinary web remote carrying credentials with an escaped character in
    /// the password, which is a real thing users have.
    #[test]
    fn parse_github_remote_accepts_a_percent_encoded_authority_on_web_transports() {
        let github = || {
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
            })
        };
        for url in [
            // The regressed case: an escape inside the password.
            "https://user:p%40ss@github.com/o/r.git",
            "http://u:p%40ss@github.com/o/r",
            // An escaped slash in the user part does NOT move the boundary
            // here, per the measurement above, so it is not a hole.
            "https://user%2Fx@github.com/o/r.git",
            "http://user%2F@github.com/o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), github(), "{url}");
        }
        // An escape in the HOST is still handled identically by both, so
        // nothing has to be refused on dux's side to keep them agreeing. curl
        // decodes the host and rejects a decoded `/` outright ("URL rejected:
        // Bad hostname" for `https://git%2Fhub.com/o/r`), which is what the
        // `url` crate does too, so this stays refused for its own reason.
        assert_eq!(parse_github_remote("https://git%2Fhub.com/o/r"), None);
        // And a decoded escape that IS a legal host character resolves the
        // same way in both: `git ls-remote https://nonexistent-host%2Einvalid/o/r`
        // reports `Could not resolve host: nonexistent-host.invalid`.
        assert_eq!(parse_github_remote("https://github%2Ecom/o/r"), github());
    }

    /// Git's native protocol has no user component, so a `user@` in a `git://`
    /// authority is part of the HOST.
    ///
    /// Measured: `GIT_TRACE=1 git ls-remote git://user@github.com/octocat/Hello-World.git`
    /// reports `unable to look up user@github.com (port 9418)`. The `url`
    /// crate reads the same string by the generic URL grammar, reports the host
    /// `github.com` and throws `user@` away as userinfo, so dux answered with a
    /// GitHub repository for a remote that never names github.com.
    ///
    /// This is scheme-specific on purpose. Git's ssh URL syntax DOES have a
    /// user component (`ssh://user@host/path` is the documented spelling), and
    /// `git+ssh`/`ssh+git` are the same transport, so a user is legitimate
    /// there. Under http(s) userinfo is legitimate credentials, already dropped
    /// as such. Only the native protocol lacks the component.
    #[test]
    fn parse_github_remote_refuses_a_user_in_a_native_git_url() {
        assert_eq!(
            parse_github_remote("git://user@github.com/octocat/Hello-World.git"),
            None,
        );
        let github = |owner_repo: &str| {
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: owner_repo.to_string(),
            })
        };
        // The native protocol without a user, and every scheme whose syntax
        // really does carry one, keep working.
        for url in [
            "git://github.com/o/r.git",
            "ssh://user@github.com/o/r.git",
            "git+ssh://user@github.com/o/r.git",
            "ssh+git://user@github.com/o/r.git",
            "https://user:token@github.com/o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), github("o/r"), "{url}");
        }
    }

    /// Git matches a URL scheme CASE SENSITIVELY. It compares the literal text
    /// before the `://` against its own lowercase table, and anything else is
    /// taken as the name of a remote helper, so an uppercase spelling does not
    /// select the transport it looks like.
    ///
    /// MEASURED, git 2.55.0, with a stub `GIT_SSH_COMMAND` that prints its argv
    /// and a `.invalid` host so nothing leaves the machine:
    ///
    /// ```text
    /// $ git ls-remote ssh://git@nonexistent-host.invalid/o/r
    /// SSH-INVOKED argv: git@nonexistent-host.invalid git-upload-pack '/o/r'
    /// $ git ls-remote SSH://git@nonexistent-host.invalid/o/r
    /// git: 'remote-SSH' is not a git command. See 'git --help'.
    /// fatal: remote helper 'SSH' aborted session
    /// ```
    ///
    /// `Ssh://`, `HTTPS://`, `GIT://` and `Git+SSH://` all fail the same way,
    /// each naming its own missing `git-remote-<as-written>` helper. So dux used
    /// to answer host `github.com`, repository `o/r` for an address git cannot
    /// connect with at all.
    ///
    /// The HOST stays case insensitive, because git really does ignore host
    /// case: `ssh://NONEXISTENT-HOST.INVALID/o/r` reaches ssh as
    /// `NONEXISTENT-HOST.INVALID git-upload-pack '/o/r'`, and
    /// `git@NonExistent-Host.Invalid:o/r.git` reaches it too. Only the scheme is
    /// case sensitive; making both insensitive was the regression this pins.
    #[test]
    fn parse_github_remote_refuses_a_scheme_spelled_in_the_wrong_case() {
        for url in [
            "SSH://git@github.com/o/r",
            "Ssh://git@github.com/o/r",
            "HTTPS://github.com/o/r",
            "Https://github.com/o/r",
            "GIT://github.com/o/r",
            "Git+SSH://github.com/o/r",
            "SSH+GIT://github.com/o/r",
            "HTTP://github.com/o/r",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
        // The host really is case insensitive, in every family, and stays so.
        let github = |owner_repo: &str| {
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: owner_repo.to_string(),
            })
        };
        for url in [
            "ssh://GITHUB.COM/o/r",
            "ssh://git@GitHub.com/o/r",
            "git@GitHub.com:o/r.git",
            "https://GitHub.COM/o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), github("o/r"), "{url}");
        }
        // `GitHub.com/o/r` used to be the fifth row of that accept list, as the
        // bare family's host-case coverage. Host case is not what is wrong with
        // it: git reads a value with no scheme and no colon as a relative LOCAL
        // PATH, so accepting it in any case meant asking GitHub about a
        // directory. See `parse_github_remote_refuses_a_bare_host_and_path` for
        // the measurement.
        assert_eq!(parse_github_remote("GitHub.com/o/r"), None);
    }

    /// `<transport>::<address>` is git's EXPLICIT remote-helper syntax, which
    /// takes precedence over everything else. It is not the scp-like shorthand,
    /// and there is no host and no `owner/repo` in it for dux to report.
    ///
    /// MEASURED, git 2.55.0, same stub ssh and `.invalid` host:
    ///
    /// ```text
    /// $ git ls-remote nonexistent-host.invalid::o/r
    /// git: 'remote-nonexistent-host.invalid' is not a git command.
    /// fatal: remote helper 'nonexistent-host.invalid' aborted session
    /// $ git ls-remote nonexistent-host.invalid::
    /// git: 'remote-nonexistent-host.invalid' is not a git command.
    /// fatal: remote helper 'nonexistent-host.invalid' aborted session
    /// $ git ls-remote nonexistent-host.invalid:o/r
    /// SSH-INVOKED argv: nonexistent-host.invalid git-upload-pack 'o/r'
    /// ```
    ///
    /// dux used to answer host `github.com`, repository `:o/r` for the first of
    /// those: a host git never contacts, and an owner with a stray colon on it.
    ///
    /// The `user@` spelling is refused too, but for a measurably DIFFERENT
    /// reason, and the difference is worth stating so nobody rediscovers it as a
    /// contradiction. Git requires a helper name to be made of URL-scheme
    /// characters, and `@` is not one, so `git@nonexistent-host.invalid::o/r` is
    /// NOT a helper invocation: it reaches ssh as
    /// `git@nonexistent-host.invalid git-upload-pack ':o/r'`. That path is still
    /// not `o/r`, and `:o` is not an owner any host can have, so answering
    /// `o/r`, or `:o/r`, for it would be wrong either way.
    #[test]
    fn parse_github_remote_refuses_gits_explicit_remote_helper_syntax() {
        for url in [
            "github.com::o/r",
            "git@github.com::o/r",
            "github.com::",
            "github.com::o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
        // The ordinary scp-like forms are untouched, including the verbatim
        // encoded one: git hands an scp-like path to ssh without decoding it.
        let github = |owner_repo: &str| {
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: owner_repo.to_string(),
            })
        };
        assert_eq!(parse_github_remote("github.com:o/r"), github("o/r"));
        assert_eq!(parse_github_remote("git@github.com:o/r.git"), github("o/r"));
        assert_eq!(parse_github_remote("github.com:o%2Fr/x"), github("o%2Fr/x"),);
    }

    /// The remote-helper rule tests the byte after the FIRST colon, and not a
    /// blunt "contains `::`", because a scheme-qualified IPv6 literal contains
    /// `::` and is not a helper invocation. Such an address never reaches the
    /// scp-like branch at all (the `//` after the scheme ends it first), and
    /// this pins that it still does not.
    ///
    /// MEASURED: `git ls-remote ssh://[::1]/o/r` reaches ssh as
    /// `::1 git-upload-pack '/o/r'`, an ordinary ssh remote. dux refuses it
    /// because `::1` is not a GitHub host, which is the only reason it should.
    #[test]
    fn parse_github_remote_leaves_a_scheme_qualified_ipv6_address_to_the_host_check() {
        assert_eq!(split_scp_like("ssh://[::1]/o/r"), None);
        assert_eq!(split_scp_like("ssh://[::1]:2222/o/r"), None);
        assert_eq!(parse_github_remote("ssh://[::1]/o/r"), None);
        assert_eq!(parse_github_remote("ssh://[::1]:2222/o/r"), None);
    }

    /// The full behaviour table, pinned as one test so no future rewrite of the
    /// parser can quietly move any single row of it.
    #[test]
    fn parse_github_remote_behaviour_table() {
        let github = |owner_repo: &str| {
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: owner_repo.to_string(),
            })
        };
        let cases: [(&str, Option<GitHubRemote>); 46] = [
            // Whitespace INSIDE the address. The literal check refuses only
            // whitespace at the edges, because a git remote really can hold it
            // there and refusing it is the point; an interior space is a
            // different thing, and it is part of the host or of the path. What
            // git would contact for the first of these is a host whose name
            // begins with a space, so none of them names a repository on
            // github.com, and the policy must not launder the space away.
            ("git@ github.com:o/r.git", None),
            ("ssh://git@ github.com/o/r", None),
            ("github.com :o/r", None),
            ("git@github.com: o/r", None),
            // GitHub's documented port-443 SSH endpoint is github.com reached
            // another way, so it parses, and it comes back NORMALISED because
            // the host is handed to `gh` as an API host.
            ("ssh://git@ssh.github.com:443/o/r.git", github("o/r")),
            ("ssh://git@ssh.github.com/o/r.git", github("o/r")),
            ("git@ssh.github.com:o/r.git", github("o/r")),
            // It is documented for ssh and nothing else, it has no enterprise
            // equivalent, and the match is the exact hostname.
            ("https://ssh.github.com/o/r", None),
            ("ssh://git@ssh.github.example.com/o/r", None),
            ("ssh://git@sshgithub.com/o/r", None),
            ("ssh://git@evil-ssh.github.com.attacker.example/o/r", None),
            // The colon precedes the first slash, so git reads this as the
            // scp-like form and contacts the host `user`.
            ("user:token@github.com/o/r", None),
            // Git matches a scheme case sensitively and reads
            // `<transport>::<address>` as an explicit remote helper, so neither
            // of these names a GitHub repository git could reach.
            ("SSH://git@GitHub.com/o/r", None),
            ("HTTPS://github.com/o/r", None),
            ("github.com::o/r", None),
            ("git@github.com::o/r", None),
            // The authority decides where git cuts for the ssh-style
            // transports, so both of these name something other than a
            // repository on github.com.
            ("ssh://user%2F@github.com/o/r.git", None),
            ("git://user%2F@github.com/o/r", None),
            ("git://user@github.com/o/r.git", None),
            // The web transports split before they decode, so the same escape
            // moves nothing and these are ordinary GitHub remotes. Measured:
            // both reach `https://nonexistent-host.invalid/o/r/` when the
            // authority is spelled against a host that does not resolve.
            ("https://user:p%40ss@github.com/o/r.git", github("o/r")),
            ("https://user%2Fx@github.com/o/r.git", github("o/r")),
            // No scheme and no colon before the first slash is not an address:
            // git reads it as a relative local path, so none of these name a
            // repository on github.com. This row used to read
            // `("github.com/o/r", github("o/r"))`.
            ("github.com/o/r", None),
            ("github.com/o/r/x", None),
            ("GitHub.com/o/r", None),
            (r"C:\repo", None),
            ("ssh://github.com/o/r/extra", None),
            ("github.com:o/r/extra", None),
            ("git@github.com:o/r/extra", None),
            ("ssh://github.com/o/r.git/", github("o/r")),
            ("ssh://github.com/o/r%2Egit", github("o/r")),
            ("user@github.com/o/r", None),
            ("user:tok@github.com/o/r", None),
            ("github.com:o/r", github("o/r")),
            ("git@GitHub.com:o/r.git", github("o/r")),
            ("ssh://GITHUB.COM/o/r", github("o/r")),
            ("ssh://github.com:99999/o/r", None),
            ("ssh://github.com:abc/o/r", None),
            ("git@github.com:o/r.git", github("o/r")),
            ("https://github.com/o/r.git", github("o/r")),
            ("ssh://git@github.com/o/r.git", github("o/r")),
            ("git@gitlab.com:o/r.git", None),
            ("ssh://git@gitlab.com/o/r.git", None),
            ("https://gitlab.com/o/r.git", None),
            ("/some/path/repo.git", None),
            ("file:///some/path/repo.git", None),
            // A remote is not a browser URL: the whole path is the repository
            // path, so this is not `o/r`.
            ("https://github.com/o/r/tree/main", None),
        ];
        for (url, expected) in cases {
            assert_eq!(parse_github_remote(url), expected, "{url}");
        }
    }

    #[test]
    fn parse_github_enterprise_https_url_preserves_host() {
        assert_eq!(
            parse_github_remote("https://github.example.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.example.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_enterprise_ssh_url_preserves_host() {
        assert_eq!(
            parse_github_remote("git@github.example.com:octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.example.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_url_with_user_and_git_suffix() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_url_without_git_suffix() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/octocat/Hello-World"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_url_without_user() {
        assert_eq!(
            parse_github_remote("ssh://github.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_url_drops_the_ssh_port() {
        // The port belongs to the ssh service, not to the host's API, and the
        // parsed host is handed to `gh`, so it must come back bare.
        assert_eq!(
            parse_github_remote("ssh://git@github.com:2222/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_enterprise_host_is_preserved() {
        assert_eq!(
            parse_github_remote("ssh://git@github.example.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.example.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    /// GitHub documents a second SSH endpoint, `ssh.github.com` on port 443,
    /// for people on networks that block port 22. A remote pointed at it is an
    /// ordinary GitHub remote and must produce a pull-request banner like any
    /// other, so both spellings of it parse.
    ///
    /// The host comes back as `github.com` rather than as written. MEASURED,
    /// git 2.55.0, isolated `HOME`, `GIT_CONFIG_NOSYSTEM=1` and a stub
    /// `GIT_SSH_COMMAND` printing its argv, so nothing left the machine:
    ///
    /// ```text
    /// ssh://git@ssh.github.com:443/o/r.git
    ///   -> -o SendEnv=GIT_PROTOCOL -p 443 git@ssh.github.com git-upload-pack '/o/r.git'
    /// ssh://git@ssh.github.com/o/r.git
    ///   -> git@ssh.github.com git-upload-pack '/o/r.git'
    /// git@ssh.github.com:o/r.git
    ///   -> git@ssh.github.com git-upload-pack 'o/r.git'
    /// ```
    ///
    /// The port is dropped as it is for every ssh remote: it is the ssh
    /// service's port and says nothing about the host's API.
    #[test]
    fn parse_github_documented_ssh_alt_host_normalises_to_github_com() {
        for url in [
            "ssh://git@ssh.github.com:443/o/r.git",
            "ssh://git@ssh.github.com/o/r.git",
            "ssh://ssh.github.com/o/r.git",
            "git@ssh.github.com:o/r.git",
            "ssh.github.com:o/r",
            "git+ssh://git@ssh.github.com/o/r.git",
            "ssh+git://git@ssh.github.com/o/r.git",
            "ssh://git@SSH.GitHub.com/o/r.git",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "o/r".to_string(),
                }),
                "{url}",
            );
        }
    }

    /// The alt host is accepted for the transport GitHub documents it for and
    /// for nothing else, and it is an EXACT hostname match rather than a prefix,
    /// suffix or substring test.
    ///
    /// `https://ssh.github.com/...` is not a documented endpoint, so dux does
    /// not invent one. Neither is an enterprise `ssh.` variant: GitHub
    /// Enterprise Server publishes no equivalent, so guessing at one would be
    /// making up an address on a customer's own network. And the native git
    /// protocol on that name is not documented either.
    #[test]
    fn parse_github_refuses_undocumented_ssh_alt_host_spellings() {
        for url in [
            // Not a documented endpoint for the web transports.
            "https://ssh.github.com/o/r",
            "https://ssh.github.com/o/r.git",
            "http://ssh.github.com/o/r",
            "git://ssh.github.com/o/r",
            // No documented GitHub Enterprise Server equivalent exists.
            "ssh://git@ssh.github.example.com/o/r",
            "git@ssh.github.example.com:o/r.git",
            // Exact match, not a substring or suffix test.
            "ssh://git@sshgithub.com/o/r",
            "ssh://git@evil-ssh.github.com.attacker.example/o/r",
            "ssh://git@ssh.github.com.attacker.example/o/r",
            "ssh://git@x.ssh.github.com/o/r",
            "git@sshgithub.com:o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// `user:token@host/o/r` is not the bare `host/path` form: the colon
    /// precedes the first slash, so git reads the WHOLE value as the scp-like
    /// spelling and the host it contacts is `user`. MEASURED, git 2.55.0, same
    /// isolated setup and stub ssh as above:
    ///
    /// ```text
    /// $ git remote add origin user:token@nonexistent-host.invalid/o/r
    /// $ git ls-remote origin
    /// STUB-SSH-ARGV: user git-upload-pack 'token@nonexistent-host.invalid/o/r'
    /// ```
    ///
    /// (Drop the colon and it really is a local path: `user@nonexistent-host
    /// .invalid/o/r` fails with "does not appear to be a git repository" and the
    /// stub ssh is never called.)
    ///
    /// It lives here rather than in the bare-form test because it is refused for
    /// a different reason: not "git reads this as a folder", but "the host git
    /// would contact is `user`, which is not GitHub".
    #[test]
    fn parse_github_remote_refuses_credentials_before_a_scp_like_colon() {
        assert_eq!(
            parse_github_remote("user:token@github.com/octocat/Hello-World"),
            None,
        );
        assert_eq!(parse_github_remote("user:token@github.com/o/r"), None);
    }

    #[test]
    fn parse_github_ssh_scheme_rejects_non_github_host() {
        // The parsed host is queried with `gh`, so a non-GitHub host must not
        // be accepted through the ssh:// spelling either.
        assert_eq!(
            parse_github_remote("ssh://git@gitlab.com/owner/repo.git"),
            None,
        );
        assert_eq!(
            parse_github_remote("ssh://git@gitlab.com:2222/owner/repo.git"),
            None,
        );
    }

    #[test]
    fn parse_github_https_url_does_not_leak_embedded_credentials() {
        assert_eq!(
            parse_github_remote("https://user:token@github.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_ssh_scheme_url_does_not_leak_embedded_credentials() {
        assert_eq!(
            parse_github_remote("ssh://user:token@github.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    #[test]
    fn parse_github_url_rejects_file_and_local_paths() {
        assert_eq!(parse_github_remote("file:///some/path/repo.git"), None);
        assert_eq!(parse_github_remote("/some/path/repo.git"), None);
        assert_eq!(parse_github_remote("../sibling/repo"), None);
    }

    /// `github.com/owner/repo`, with no scheme and no colon, is not a network
    /// address at all. Git's grammar offers exactly two remote spellings that
    /// leave the machine: a scheme-qualified URL, and the scp-like
    /// `[user@]host:path`, which REQUIRES the colon to come before any slash.
    /// A value with neither is a RELATIVE LOCAL PATH, and git reads it as one.
    ///
    /// MEASURED, git 2.55.0, isolated `HOME`, `GIT_CONFIG_NOSYSTEM=1`, a stub
    /// `GIT_SSH_COMMAND` that prints its argv, and a `.invalid` host so nothing
    /// can leave the machine:
    ///
    /// ```text
    /// $ git remote add bare nonexistent-host.invalid/o/r
    /// $ GIT_TRACE=1 git ls-remote bare
    /// trace: run_command: git-upload-pack 'nonexistent-host.invalid/o/r'
    /// trace: built-in: git upload-pack nonexistent-host.invalid/o/r
    /// fatal: 'nonexistent-host.invalid/o/r' does not appear to be a git repository
    /// $ mkdir -p nonexistent-host.invalid/o && git init --bare nonexistent-host.invalid/o/r
    /// $ git ls-remote bare; echo $?
    /// 0
    /// ```
    ///
    /// The stub ssh is never called, no name is ever resolved, and once the
    /// DIRECTORY exists git reads it happily as a local repository. Contrast
    /// the scp-like spelling of the same words, which does go over the network:
    ///
    /// ```text
    /// $ git remote add scp nonexistent-host.invalid:o/r
    /// $ git ls-remote scp
    /// STUB_SSH_ARGV: nonexistent-host.invalid git-upload-pack 'o/r'
    /// ```
    ///
    /// So a remote written the bare way points at a folder on disk. dux used to
    /// answer host `github.com`, repository `o/r` for it and go and ask GitHub
    /// about a directory. It is refused instead, whatever the case of the host
    /// and with or without a `user@` in front (which is not git syntax in any
    /// spelling either: with no scheme there is no authority for credentials to
    /// belong to).
    #[test]
    fn parse_github_remote_refuses_a_bare_host_and_path() {
        for url in [
            "github.com/octocat/Hello-World",
            "github.com/octocat/Hello-World.git",
            "GitHub.com/octocat/Hello-World",
            "github.com/o/r",
            "github.com/o/r/x",
            "user@github.com/octocat/Hello-World",
            "gitlab.com/o/r",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    /// The scp-like spelling's user part is optional: git treats `host:path` as
    /// ssh whenever the colon precedes any slash, with or without `user@`.
    #[test]
    fn parse_github_scp_like_without_user() {
        assert_eq!(
            parse_github_remote("github.com:octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    /// The path of an ssh remote IS the repository, so a third segment names a
    /// different repository than `owner/repo`. Answering `owner/repo` sent `gh`
    /// somewhere git never goes.
    #[test]
    fn parse_github_ssh_forms_reject_extra_path_segments() {
        assert_eq!(
            parse_github_remote("ssh://github.com/octocat/Hello-World/extra"),
            None
        );
        assert_eq!(
            parse_github_remote("github.com:octocat/Hello-World/extra"),
            None
        );
        assert_eq!(
            parse_github_remote("git@github.com:octocat/Hello-World/extra"),
            None,
        );
    }

    /// A trailing slash is not part of the repository name, and it has to come
    /// off before the `.git` suffix or the suffix no longer matches. Exactly ONE
    /// comes off, though: `Hello-World//` used to be pinned here as `o/r` and is
    /// now refused, because the second slash leaves an empty component and an
    /// empty component is not a repository. That is the same rule that stops a
    /// decoded `%2F` at the boundary from impersonating the syntactic slash; see
    /// `parse_github_remote_refuses_decoded_separators_at_the_path_boundaries`.
    #[test]
    fn parse_github_remote_trims_one_trailing_slash() {
        for url in [
            "ssh://github.com/octocat/Hello-World.git/",
            "https://github.com/octocat/Hello-World.git/",
            "github.com:octocat/Hello-World.git/",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                }),
                "{url}",
            );
        }
    }

    /// git percent-decodes the path of a real URL, so dux must too: the remote
    /// below addresses `/octocat/Hello-World.git`. A decoded DOT is legitimate
    /// inside a repository name and keeps working; a decoded SLASH never is,
    /// and is refused wherever it appears.
    #[test]
    fn parse_github_url_decodes_percent_escapes() {
        assert_eq!(
            parse_github_remote("ssh://github.com/octocat/Hello-World%2Egit"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
        // A decoded separator is refused rather than counted. This one was
        // already refused, by the segment count.
        assert_eq!(
            parse_github_remote("ssh://github.com/octocat/Hello-World%2Fextra"),
            None,
        );
        // And this one was pinned the other way round, as `octocat/Hello-World`,
        // which encoded the wrong assumption: that a decoded slash in the
        // MIDDLE of a path is a separator dux may act on. It is not. The real
        // service was asked, and `git ls-remote` for this address answers "Not
        // Found" while `octocat/Hello-World` exists, so the accepted answer
        // named a repository this remote does not address. Neither a GitHub
        // owner nor a repository name can contain a slash, so a decoded one
        // always means the address names something other than what it appears
        // to name, in every scheme and at every position.
        assert_eq!(
            parse_github_remote("https://github.com/octocat%2FHello-World"),
            None,
        );
        assert_eq!(
            parse_github_remote("https://github.com/octocat%2FHello-World.git"),
            None,
        );
        assert_eq!(
            parse_github_remote("ssh://github.com/octocat%2FHello-World.git"),
            None,
        );
    }

    /// Hostnames are case-insensitive, and the parsed host is handed to `gh`,
    /// so it is compared and stored lowercased. A capitalised host used to fall
    /// into the same silent no-pull-request-anywhere failure as the ssh one.
    ///
    /// This list used to carry `SSH://git@GitHub.com/octocat/Hello-World.git`
    /// as an accepted row, and it passed for the wrong reason: the round that
    /// made the HOST case insensitive checked the scheme after the `url` crate
    /// had lowercased it, which made the SCHEME case insensitive too. Git's is
    /// not (measured in
    /// `parse_github_remote_refuses_a_scheme_spelled_in_the_wrong_case`), so
    /// that row asserted dux answering for a remote git cannot connect with. It
    /// has moved to that test, as a refusal.
    #[test]
    fn parse_github_host_is_matched_and_stored_case_insensitively() {
        for url in [
            "git@GitHub.com:octocat/Hello-World.git",
            "GitHub.com:octocat/Hello-World.git",
            "ssh://GITHUB.COM/octocat/Hello-World.git",
            "ssh://git@GitHub.com/octocat/Hello-World.git",
            "https://GitHub.COM/octocat/Hello-World.git",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                }),
                "{url}",
            );
        }
        // The bare `GitHub.com/octocat/Hello-World` spelling was the sixth row
        // here and is now a refusal, for a reason that has nothing to do with
        // case: git reads a value with no scheme and no colon before the first
        // slash as a relative LOCAL PATH, so accepting it meant asking GitHub
        // about a directory on disk. See
        // `parse_github_remote_refuses_a_bare_host_and_path`.
        assert_eq!(parse_github_remote("GitHub.com/octocat/Hello-World"), None);
    }

    /// `github.com:octocat/…` parses "successfully" as a URL whose scheme is
    /// `github.com`, so the scp-like branch has to be tried first, and the
    /// scheme allow-list has to be closed as well.
    #[test]
    fn parse_github_url_accepts_only_git_transport_schemes() {
        assert_eq!(
            parse_github_remote("git://github.com/octocat/Hello-World.git"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
        assert_eq!(
            parse_github_remote("ftp://github.com/octocat/Hello-World"),
            None
        );
        // `git+ssh://` used to be pinned as rejected here. It is a valid git
        // scheme that invokes SSH, so it is now accepted; see
        // `parse_github_remote_accepts_the_ssh_scheme_aliases`.
        assert_eq!(
            parse_github_remote("git+ssh://github.com/octocat/Hello-World"),
            Some(GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            }),
        );
    }

    /// Pins what the URL parser actually does with the shapes at the edges,
    /// rather than assuming. An out-of-range or non-numeric port is a parse
    /// error, not a host we could mistakenly keep; a scheme with no host has no
    /// host to qualify.
    #[test]
    fn parse_github_url_rejects_malformed_authorities() {
        assert_eq!(
            parse_github_remote("ssh://github.com:99999/octocat/Hello-World"),
            None
        );
        assert_eq!(
            parse_github_remote("ssh://github.com:abc/octocat/Hello-World"),
            None
        );
        assert_eq!(parse_github_remote("ssh:///octocat/Hello-World"), None);
        // An internationalised host is normalised by the `url` crate (punycode
        // under http(s), percent-encoding under ssh) and neither result is a
        // GitHub host, so both are rejected. dux adds no normalisation of its
        // own.
        assert_eq!(
            parse_github_remote("https://gïthub.com/octocat/Hello-World"),
            None
        );
        assert_eq!(
            parse_github_remote("ssh://gïthub.com/octocat/Hello-World"),
            None
        );
    }

    /// GitHub does not allow a repository named `.git`, and a bare `.git`
    /// segment is the suffix rather than a name, so there is no repository left
    /// to ask about.
    #[test]
    fn parse_github_url_rejects_a_repository_named_dot_git() {
        assert_eq!(parse_github_remote("ssh://github.com/octocat/.git"), None);
        assert_eq!(parse_github_remote("https://github.com/octocat/.git"), None);
    }

    // ── unborn-HEAD / initial-commit tests ───────────────────────

    /// Create a temporary repo with `git init` but NO commit (unborn HEAD),
    /// with a local identity configured so a commit can be made if asked.
    fn init_test_repo_no_commit() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let out = test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        dir
    }

    #[test]
    fn repo_has_commits_is_false_on_unborn_head() {
        let repo = init_test_repo_no_commit();
        assert!(
            !repo_has_commits(repo.path()),
            "a fresh `git init` with no commits must report no commits"
        );
    }

    #[test]
    fn repo_has_commits_is_true_after_a_commit() {
        let repo = init_test_repo(); // makes one empty commit
        assert!(
            repo_has_commits(repo.path()),
            "a repo with a commit must report having commits"
        );
    }

    #[test]
    fn repo_has_commits_is_false_for_non_repo() {
        // A plain directory that is not a git repo must not report commits
        // (and must not panic).
        let tmp = tempfile::tempdir().unwrap();
        assert!(!repo_has_commits(tmp.path()));
    }

    #[test]
    fn create_initial_commit_makes_head_resolvable() {
        let repo = init_test_repo_no_commit();
        assert!(!repo_has_commits(repo.path()));
        create_initial_commit(repo.path()).expect("initial commit should succeed");
        assert!(
            repo_has_commits(repo.path()),
            "after create_initial_commit, HEAD must resolve to a commit"
        );
    }

    #[test]
    fn create_initial_commit_does_not_stage_existing_files() {
        // A folder can contain files before `git init`. The empty initial
        // commit must NOT commit them: they stay untracked afterwards.
        let repo = init_test_repo_no_commit();
        std::fs::write(repo.path().join("wip.txt"), "work in progress").unwrap();
        create_initial_commit(repo.path()).expect("initial commit should succeed");

        let out = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "status",
                "--porcelain",
            ])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&out.stdout);
        assert!(
            status.contains("?? wip.txt"),
            "existing file must remain untracked after the empty initial commit, got: {status:?}"
        );
    }

    #[test]
    fn create_initial_commit_is_idempotent_when_already_born() {
        // Called on a repo that already has a commit (e.g. one raced in), it must
        // NOT add a second commit — it returns Ok with the current branch so the
        // caller can still register the project.
        let repo = init_test_repo(); // one empty commit on "main"
        assert!(repo_has_commits(repo.path()));
        let count_before = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "rev-list",
                "--count",
                "HEAD",
            ])
            .output()
            .unwrap();
        let before = String::from_utf8_lossy(&count_before.stdout)
            .trim()
            .to_string();

        let branch =
            create_initial_commit(repo.path()).expect("already-born repo is idempotent success");
        assert_eq!(branch, "main", "returns the current branch");

        let count_after = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "rev-list",
                "--count",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&count_after.stdout).trim(),
            before,
            "must not add a second commit to a born repo"
        );
    }

    #[test]
    fn create_initial_commit_produces_a_truly_empty_commit() {
        // The commit must contain no files, regardless of an untracked working
        // tree — its tree must equal the empty tree.
        let repo = init_test_repo_no_commit();
        std::fs::write(repo.path().join("untracked.txt"), "x").unwrap();
        create_initial_commit(repo.path()).expect("initial commit should succeed");

        // `git show --stat` on a truly empty commit lists no files.
        let out = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "show",
                "--stat",
                "--format=",
                "HEAD",
            ])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        assert!(
            stat.trim().is_empty(),
            "the initial commit must have an empty tree (no files), got: {stat:?}"
        );
    }

    #[test]
    fn create_initial_commit_refuses_when_files_are_staged() {
        // If the user staged files before adding the repo, we must NOT silently
        // bake them into the "empty" initial commit. Refuse instead.
        let repo = init_test_repo_no_commit();
        std::fs::write(repo.path().join("secret.env"), "TOKEN=abc").unwrap();
        let add = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "add",
                "secret.env",
            ])
            .output()
            .unwrap();
        assert!(add.status.success());

        let result = create_initial_commit(repo.path());
        assert!(
            result.is_err(),
            "create_initial_commit must refuse when the index has staged content"
        );
        assert!(
            !repo_has_commits(repo.path()),
            "refusing must leave the repo commit-less (nothing was committed)"
        );
    }

    #[test]
    fn create_initial_commit_surfaces_the_git_error_when_it_fails() {
        // Exercise the failure path deterministically: a read-only object store
        // lets the read-only pre-checks pass (unborn HEAD, clean index) but makes
        // writing the commit objects fail. The error must be surfaced (carrying
        // git's stderr) and the repo must stay commit-less.
        if is_root() {
            // root bypasses DAC write bits, so the read-only trick can't fail git.
            return;
        }
        let repo = init_test_repo_no_commit();
        let objects = repo.path().join(".git/objects");
        let mut perms = std::fs::metadata(&objects).unwrap().permissions();
        let original = perms.clone();
        perms.set_readonly(true);
        std::fs::set_permissions(&objects, perms).unwrap();

        let result = create_initial_commit(repo.path());

        // Restore write permission so the TempDir can be cleaned up.
        std::fs::set_permissions(&objects, original).unwrap();

        let err = result.expect_err("commit into a read-only object store must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&repo.path().display().to_string()),
            "error must name the repo path, got: {msg}"
        );
        assert!(
            !repo_has_commits(repo.path()),
            "a failed commit must leave the repo commit-less"
        );
    }

    /// True when the test process runs as uid 0 (root ignores DAC write bits, so
    /// permission-based failure injection is a no-op).
    fn is_root() -> bool {
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
    }

    #[test]
    fn create_initial_commit_runs_no_hooks() {
        // The bootstrap commit is built with plumbing, so NO hook runs — not
        // pre-commit/commit-msg (which `--no-verify` would skip) and crucially not
        // post-commit/reference-transaction (which `--no-verify` does NOT skip).
        let repo = init_test_repo_no_commit();
        let hooks = repo.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        for name in ["pre-commit", "post-commit", "reference-transaction"] {
            let hook = hooks.join(name);
            std::fs::write(&hook, format!("#!/bin/sh\ntouch RAN_{name}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        create_initial_commit(repo.path()).expect("commit should succeed with hooks present");
        assert!(repo_has_commits(repo.path()));
        for name in ["pre-commit", "post-commit", "reference-transaction"] {
            assert!(
                !repo.path().join(format!("RAN_{name}")).exists(),
                "the {name} hook must not have executed"
            );
        }
    }

    #[test]
    fn create_initial_commit_works_on_a_bare_repo() {
        // A fresh `git init --bare` repo has no work tree, so `git commit` can't
        // be used — the plumbing path must still bootstrap it.
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "--bare", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        assert_eq!(repo_commit_state(dir.path()), CommitState::Unborn);

        create_initial_commit(dir.path()).expect("bare repo bootstrap should succeed");
        assert!(
            repo_has_commits(dir.path()),
            "the bare repo must have a commit after bootstrap"
        );
    }

    #[test]
    fn create_initial_commit_is_idempotent_on_a_born_detached_head() {
        // Born + detached HEAD (no symbolic ref): still idempotent success (no
        // second commit, no error), per the documented contract.
        let repo = init_test_repo(); // one commit on "main"
        let run = |args: &[&str]| {
            assert!(
                test_support::git_command()
                    .args(args)
                    .current_dir(repo.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["checkout", "--detach"]);
        assert_eq!(repo_commit_state(repo.path()), CommitState::Born);

        let branch = create_initial_commit(repo.path())
            .expect("a born detached-HEAD repo must be idempotent success, not an error");
        // Detached HEAD has no symbolic branch name — the returned branch is
        // empty (the worker degrades this to the "main" fallback, tested in
        // project_browser).
        assert!(
            branch.is_empty(),
            "a detached HEAD has no branch name, got {branch:?}"
        );
        let out = test_support::git_command()
            .args([
                "-C",
                repo.path().to_string_lossy().as_ref(),
                "rev-list",
                "--count",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "1",
            "must not add a second commit"
        );
    }

    #[test]
    fn create_initial_commit_is_race_safe_across_threads() {
        // Two threads bootstrap the SAME unborn repo concurrently (simulating two
        // dux instances). The update-ref CAS must let exactly one commit land —
        // never two — and the loser must NOT hard-error: it sees the repo is now
        // Born and returns Ok too. End state: exactly one commit.
        let repo = init_test_repo_no_commit();
        let path = repo.path().to_path_buf();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let p = path.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    create_initial_commit(&p)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        assert!(
            results.iter().all(|r| r.is_ok()),
            "neither racer should hard-error, got: {results:?}"
        );
        // Exactly one commit exists (the CAS prevented a second).
        let out = test_support::git_command()
            .args([
                "-C",
                path.to_string_lossy().as_ref(),
                "rev-list",
                "--count",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "1",
            "the race must leave exactly one commit"
        );
    }

    #[test]
    fn repo_commit_state_distinguishes_born_unborn_and_non_repo() {
        assert_eq!(
            repo_commit_state(init_test_repo_no_commit().path()),
            CommitState::Unborn
        );
        assert_eq!(
            repo_commit_state(init_test_repo().path()),
            CommitState::Born
        );
        // A non-repo path can't be classified — Indeterminate, never Unborn.
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(repo_commit_state(tmp.path()), CommitState::Indeterminate);
    }

    #[test]
    fn repo_path_kind_classifies_root_subdir_plain_and_bare() {
        // Catches the crux misclassifications, including offering `git init`
        // on a bare repository.
        let repo = init_test_repo();
        assert_eq!(repo_path_kind(repo.path()), RepoPathKind::WorkTreeRoot);

        let sub = repo.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        match repo_path_kind(&sub) {
            RepoPathKind::InsideWorkTree { root } => {
                assert_eq!(
                    root.canonicalize().unwrap(),
                    repo.path().canonicalize().unwrap()
                );
            }
            other => panic!("subdir must classify as InsideWorkTree, got {other:?}"),
        }

        let plain = tempfile::tempdir().unwrap();
        assert_eq!(repo_path_kind(plain.path()), RepoPathKind::NotARepo);

        let bare = tempfile::tempdir().unwrap();
        let out = test_support::git_command()
            .args(["init", "--bare"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(repo_path_kind(bare.path()), RepoPathKind::BareRoot);
    }

    #[test]
    fn repo_path_kind_flags_git_internal_directories() {
        // Catches the measured ladder hole: inside `<repo>/.git`, `--git-dir`
        // succeeds, `--is-bare-repository` is false, and `--show-toplevel`
        // exits 128; without the `--is-inside-git-dir` rung this fell to
        // Indeterminate and the fail-open add gate accepted `~/repo/.git`.
        let repo = init_test_repo();
        let git_dir = repo.path().join(".git");
        assert!(
            matches!(repo_path_kind(&git_dir), RepoPathKind::InsideGitDir { .. }),
            "<repo>/.git must classify as InsideGitDir"
        );
        assert!(
            matches!(
                repo_path_kind(&git_dir.join("objects")),
                RepoPathKind::InsideGitDir { .. }
            ),
            "<repo>/.git/objects must classify as InsideGitDir"
        );
    }

    #[test]
    fn repo_path_kind_flags_bare_repo_internals() {
        // Catches registering a bare repository's internals as a project.
        let bare = tempfile::tempdir().unwrap();
        let out = test_support::git_command()
            .args(["init", "--bare"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(
            matches!(
                repo_path_kind(&bare.path().join("objects")),
                RepoPathKind::InsideGitDir { .. }
            ),
            "a bare repo's objects/ must classify as InsideGitDir"
        );
    }

    #[test]
    fn repo_path_kind_classifies_repos_under_non_utf8_paths() {
        // Catches the fail-open gate bypass: `--show-toplevel` output for a
        // repo under a non-UTF8 path (legal on Linux) must be decoded from the
        // raw bytes; a lossy decode rewrites the byte to U+FFFD, fails
        // canonicalization, and falls to Indeterminate, which the add gate
        // accepts.
        use std::os::unix::ffi::OsStrExt;
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().join(std::ffi::OsStr::from_bytes(b"rep\xFFo"));
        std::fs::create_dir(&repo).unwrap();
        let out = test_support::git_command()
            .arg("-C")
            .arg(&repo)
            .arg("init")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git init under a non-UTF8 path failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(repo_path_kind(&repo), RepoPathKind::WorkTreeRoot);

        let sub = repo.join("src");
        std::fs::create_dir(&sub).unwrap();
        match repo_path_kind(&sub) {
            RepoPathKind::InsideWorkTree { root } => {
                assert_eq!(root.canonicalize().unwrap(), repo.canonicalize().unwrap());
            }
            other => panic!(
                "a subdir of a non-UTF8-path repo must classify as InsideWorkTree, got {other:?}"
            ),
        }
        assert!(
            matches!(
                repo_path_kind(&repo.join(".git")),
                RepoPathKind::InsideGitDir { .. }
            ),
            "a non-UTF8-path repo's .git must classify as InsideGitDir"
        );
    }

    #[test]
    fn init_repo_creates_a_repository_and_errors_on_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).expect("git init in an empty folder must succeed");
        assert!(is_git_repo(dir.path()));
        assert_eq!(repo_commit_state(dir.path()), CommitState::Unborn);

        let missing = dir.path().join("does-not-exist");
        assert!(
            init_repo(&missing).is_err(),
            "git init in a missing folder must surface an error"
        );
    }

    // ── remote_default_branch tests ──────────────────────────────

    #[test]
    fn remote_default_branch_returns_none_for_local_only_repo() {
        let repo = init_test_repo();
        // A repo created with `git init` has no remotes, so origin/HEAD
        // doesn't exist and the function should return None.
        assert_eq!(remote_default_branch(repo.path()), None);
    }

    #[test]
    fn remote_default_branch_returns_branch_from_cloned_repo() {
        // Set up a "remote" bare repo and clone it, which auto-sets origin/HEAD.
        let bare_dir = tempfile::tempdir().unwrap();
        let bare = bare_dir.path();
        let run = |cwd: &Path, args: &[&str]| {
            let out = test_support::git_command()
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(bare, &["init", "--bare", "-b", "main"]);

        // Create a temporary non-bare repo, add a commit, push to the bare.
        let staging_dir = tempfile::tempdir().unwrap();
        let staging = staging_dir.path();
        run(staging, &["clone", bare.to_str().unwrap(), "."]);
        run(staging, &["config", "user.name", "test"]);
        run(staging, &["config", "user.email", "t@t"]);
        run(staging, &["commit", "--allow-empty", "-m", "init"]);
        run(staging, &["push", "origin", "main"]);

        // Now clone the bare repo — this sets origin/HEAD automatically.
        let clone_dir = tempfile::tempdir().unwrap();
        let clone = clone_dir.path();
        run(clone, &["clone", bare.to_str().unwrap(), "."]);

        assert_eq!(remote_default_branch(clone), Some("main".to_string()),);
    }

    // ── switch_branch tests ──────────────────────────────────────

    #[test]
    fn switch_branch_switches_on_clean_tree() {
        let repo = init_test_repo();
        run_git(repo.path(), &["branch", "feat"]);
        assert_eq!(current_branch(repo.path()).unwrap(), "main");

        switch_branch(repo.path(), "feat").unwrap();

        assert_eq!(current_branch(repo.path()).unwrap(), "feat");
    }

    #[test]
    fn switch_branch_errors_when_target_missing() {
        let repo = init_test_repo();
        let err = switch_branch(repo.path(), "does-not-exist").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("git switch does-not-exist failed"),
            "expected failure message, got: {msg}"
        );
    }

    #[test]
    fn switch_branch_preserves_unrelated_untracked_files() {
        let repo = init_test_repo();
        run_git(repo.path(), &["branch", "feat"]);
        fs::write(repo.path().join("scratch.txt"), "unrelated\n").unwrap();

        switch_branch(repo.path(), "feat").unwrap();

        assert_eq!(current_branch(repo.path()).unwrap(), "feat");
        assert_eq!(
            fs::read_to_string(repo.path().join("scratch.txt")).unwrap(),
            "unrelated\n"
        );
    }

    #[test]
    fn switch_branch_errors_when_unstaged_changes_would_be_overwritten() {
        let repo = init_test_repo();
        // Create a tracked file on main.
        fs::write(repo.path().join("a.txt"), "main-v1\n").unwrap();
        commit_all(repo.path(), "add a.txt on main");

        // Fork feat branch with a different version of a.txt. Uses `switch
        // -c` to both create and switch to the branch; switch_branch is not
        // exercised here — that's the subject under test below.
        run_git(repo.path(), &["switch", "-c", "feat"]);
        fs::write(repo.path().join("a.txt"), "feat-v1\n").unwrap();
        commit_all(repo.path(), "modify a.txt on feat");

        // Back on main, introduce unstaged changes to a.txt.
        run_git(repo.path(), &["switch", "main"]);
        fs::write(repo.path().join("a.txt"), "dirty\n").unwrap();

        // Switching to feat should refuse because it would overwrite.
        let err = switch_branch(repo.path(), "feat").unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("overwritten") || msg.contains("would be"),
            "expected conflict error, got: {msg}"
        );
    }

    // ── current_branch_opt tests ─────────────────────────────────

    #[test]
    fn current_branch_opt_returns_branch_on_normal_head() {
        let tmp = init_test_repo();
        assert_eq!(
            current_branch_opt(tmp.path()).unwrap(),
            Some("main".to_string())
        );
    }

    #[test]
    fn current_branch_opt_returns_none_on_detached_head() {
        let tmp = init_test_repo();
        let p = tmp.path();
        // Create a second commit so there is a parent to detach onto.
        std::fs::write(p.join("f"), b"x").unwrap();
        run_git(p, &["add", "."]);
        run_git(p, &["commit", "-m", "second"]);
        run_git(p, &["checkout", "--detach", "HEAD~1"]);
        assert_eq!(current_branch_opt(p).unwrap(), None);
    }

    #[test]
    fn current_branch_opt_errors_on_non_repo() {
        let tmp = tempfile::tempdir().unwrap(); // not a git repo
        assert!(current_branch_opt(tmp.path()).is_err());
    }

    #[test]
    fn switch_branch_if_needed_switches_from_detached_head() {
        let tmp = init_test_repo();
        let p = tmp.path().to_path_buf();
        run_git(&p, &["checkout", "--detach", "HEAD"]);
        // Must not error on detached HEAD; must end up on main.
        switch_branch_if_needed(&p, "main").unwrap();
        assert_eq!(current_branch_opt(&p).unwrap(), Some("main".to_string()));
    }

    #[test]
    fn pull_branch_switches_to_requested_branch_before_pull() {
        let bare_dir = tempfile::tempdir().unwrap();
        let bare = bare_dir.path();
        run_git(bare, &["init", "--bare", "-b", "main"]);

        let staging_dir = tempfile::tempdir().unwrap();
        let staging = staging_dir.path();
        run_git(staging, &["clone", bare.to_str().unwrap(), "."]);
        run_git(staging, &["config", "user.name", "test"]);
        run_git(staging, &["config", "user.email", "t@t"]);
        run_git(staging, &["commit", "--allow-empty", "-m", "init"]);
        run_git(staging, &["push", "origin", "main"]);
        run_git(staging, &["switch", "-c", "feature"]);
        run_git(staging, &["commit", "--allow-empty", "-m", "feature"]);
        run_git(staging, &["push", "origin", "feature"]);

        let clone_dir = tempfile::tempdir().unwrap();
        let clone = clone_dir.path();
        run_git(clone, &["clone", bare.to_str().unwrap(), "."]);
        run_git(clone, &["switch", "feature"]);
        assert_eq!(current_branch(clone).unwrap(), "feature");

        pull_branch(clone, "main").unwrap();

        assert_eq!(current_branch(clone).unwrap(), "main");
    }

    #[test]
    fn pull_current_branch_on_detached_head_returns_clear_error() {
        let repo = init_test_repo();
        // A second commit is required so HEAD~1 exists for the detach.
        fs::write(repo.path().join("detach.txt"), "x\n").unwrap();
        run_git(repo.path(), &["add", "detach.txt"]);
        run_git(repo.path(), &["commit", "-m", "second commit"]);
        run_git(repo.path(), &["checkout", "--detach", "HEAD~1"]);

        let err = pull_current_branch(repo.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("detached"),
            "expected 'detached' in error, got: {msg}"
        );
        assert!(
            !msg.contains("symbolic-ref"),
            "expected no 'symbolic-ref' in error, got: {msg}"
        );
    }

    #[test]
    fn push_on_detached_head_returns_clear_error() {
        let repo = init_test_repo();
        // A second commit is required so HEAD~1 exists for the detach.
        fs::write(repo.path().join("detach.txt"), "x\n").unwrap();
        run_git(repo.path(), &["add", "detach.txt"]);
        run_git(repo.path(), &["commit", "-m", "second commit"]);
        run_git(repo.path(), &["checkout", "--detach", "HEAD~1"]);

        let err = push(repo.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("detached"),
            "expected 'detached' in error, got: {msg}"
        );
        assert!(
            !msg.contains("symbolic-ref"),
            "expected no 'symbolic-ref' in error, got: {msg}"
        );
    }

    /// The two rules must not have leaked into each other. Everything below is
    /// something a PERSON may type into the pull request field, and every one of
    /// them must be refused as a project's CONFIGURED address, which git alone
    /// decides. This is the safety property that keeps the leniency from naming
    /// a different repository than the one on disk.
    #[test]
    fn independent_check_lenient_typed_forms_are_not_configured_addresses() {
        // Only the forms git itself does NOT accept as an address. A plain
        // `https://host/owner/repo` is a real address and legitimately parses as
        // both, which is not a leak.
        let lenient = [
            "example/application",
            "github.com/example/application",
            "https://github.com/example/application/issues",
            "https://github.com/example/application/security/dependabot",
            "https://github.com/example/application/this/is/a/made/up/path",
        ];
        for raw in lenient {
            assert!(
                crate::pr_reference::parse_typed_reference(raw).is_ok(),
                "a person may type this: {raw}"
            );
            assert_eq!(
                parse_github_remote(raw),
                None,
                "but it must NEVER be read as a configured address: {raw}"
            );
        }
        for both in [
            "git@github.com:example/application.git",
            "https://github.com/example/application",
        ] {
            assert!(
                parse_github_remote(both).is_some(),
                "a real address must still parse: {both}"
            );
            assert!(
                crate::pr_reference::parse_typed_reference(both).is_ok(),
                "and a person may equally type it: {both}"
            );
        }
    }
}
