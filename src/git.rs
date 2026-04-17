use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use content_inspector::{ContentType, inspect};

use crate::model::ChangedFile;

/// Where a branch was found when checking for its existence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchLocation {
    /// The branch exists as a local `refs/heads/` ref.
    Local,
    /// The branch exists only as a remote tracking ref (`refs/remotes/origin/`).
    Remote,
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

pub fn is_dirty(repo_path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub fn pull_current_branch(repo_path: &Path) -> Result<()> {
    let branch = current_branch(repo_path)?;
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "pull",
            "--ff-only",
            "origin",
            &branch,
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

/// Checks whether a branch exists locally or on the `origin` remote.
///
/// Uses the plumbing command `git rev-parse --verify --quiet` and inspects
/// only the exit code — no stdout is parsed.
pub fn branch_exists(repo_path: &Path, name: &str) -> Option<BranchLocation> {
    let repo = repo_path.to_string_lossy();
    let local_ref = format!("refs/heads/{name}");
    let local = Command::new("git")
        .args([
            "-C",
            repo.as_ref(),
            "rev-parse",
            "--verify",
            "--quiet",
            &local_ref,
        ])
        .output()
        .ok()
        .is_some_and(|o| o.status.success());
    if local {
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

pub fn create_worktree(
    repo_path: &Path,
    worktrees_root: &Path,
    project_name: &str,
    custom_name: Option<&str>,
) -> Result<(String, PathBuf)> {
    create_worktree_from_start_point(repo_path, worktrees_root, project_name, None, custom_name)
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

pub fn mirror_worktree_contents(source: &Path, destination: &Path) -> Result<()> {
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
    sync_directory_contents(&source, &destination)
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
            branch_name,
        ])
        .output()?;
    Ok(RemoveResult {
        branch_already_deleted: !branch_output.status.success(),
    })
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
        let path = String::from_utf8_lossy(&record[3..]).to_string();

        if path.is_empty() {
            continue;
        }

        if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
            let _ = records.next();
        }

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

    if !path_bytes.is_empty() {
        return Some((String::from_utf8_lossy(path_bytes).to_string(), stat));
    }

    let _old_path = records.next()?;
    let new_path = records.next()?;
    Some((String::from_utf8_lossy(new_path).to_string(), stat))
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
    let branch = current_branch(worktree_path)?;
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "push", "-u", "origin", &branch])
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

pub fn is_under(base: &Path, candidate: &Path) -> bool {
    match (base.canonicalize(), candidate.canonicalize()) {
        (Ok(b), Ok(c)) => c.starts_with(b),
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

/// Rename a git branch inside a worktree. Runs `git branch -m <old> <new>`
/// from within the worktree directory.
pub fn rename_branch(worktree_path: &Path, old_name: &str, new_name: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "branch",
            "-m",
            old_name,
            new_name,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git branch rename failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
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

/// Returns `"owner/repo"` parsed from the `origin` remote URL, or `None` if
/// the remote doesn't point to GitHub or the command fails.
pub fn remote_owner_repo(worktree_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            worktree_path.to_string_lossy().as_ref(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_github_owner_repo(&url)
}

/// Extracts `"owner/repo"` from a GitHub remote URL.
///
/// Supports SSH (`git@github.com:owner/repo.git`), HTTPS
/// (`https://github.com/owner/repo.git`), and bare
/// (`github.com/owner/repo`) forms.
fn parse_github_owner_repo(url: &str) -> Option<String> {
    // SSH: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        if rest.contains('/') {
            return Some(rest.to_string());
        }
    }
    // HTTPS / plain: https://github.com/owner/repo.git or github.com/owner/repo
    let needle = "github.com/";
    if let Some(idx) = url.find(needle) {
        let rest = &url[idx + needle.len()..];
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        // Expect exactly "owner/repo"
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Some(format!("{}/{}", parts[0], parts[1]));
        }
    }
    None
}

fn sync_directory_contents(source: &Path, destination: &Path) -> Result<()> {
    let mut source_entries = Vec::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_git_admin_entry(&name) {
            continue;
        }
        source_entries.push(name);
        sync_entry(&entry.path(), &destination.join(entry.file_name()))?;
    }

    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_git_admin_entry(&name) {
            continue;
        }
        if !source_entries.iter().any(|candidate| candidate == &name) {
            remove_path(&entry.path())?;
        }
    }

    Ok(())
}

fn sync_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        sync_symlink(source, destination)?;
        return Ok(());
    }

    if file_type.is_dir() {
        if destination.exists() {
            let destination_meta = fs::symlink_metadata(destination)?;
            if !destination_meta.file_type().is_dir() || destination_meta.file_type().is_symlink() {
                remove_path(destination)?;
            }
        }
        if !destination.exists() {
            fs::create_dir(destination)?;
        }
        sync_directory_contents(source, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }

    if destination.exists() {
        let destination_meta = fs::symlink_metadata(destination)?;
        if destination_meta.file_type().is_dir() || destination_meta.file_type().is_symlink() {
            remove_path(destination)?;
        }
    }
    fs::copy(source, destination)?;
    fs::set_permissions(destination, metadata.permissions())?;
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
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn is_git_admin_entry(name: &std::ffi::OsStr) -> bool {
    name == ".git"
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
            let out = Command::new("git")
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
        let out = Command::new("git")
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

    fn run_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
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

    #[test]
    fn mirror_worktree_contents_copies_visible_tree_and_preserves_git_admin_state() {
        let repo = init_test_repo();
        fs::write(repo.path().join("tracked.txt"), "original tracked\n").unwrap();
        fs::write(repo.path().join("delete-me.txt"), "delete me\n").unwrap();
        commit_all(repo.path(), "tracked files");

        let source = add_worktree(repo.path(), "mirror-source");
        let destination = add_worktree(repo.path(), "mirror-destination");

        fs::write(source.join("tracked.txt"), "modified tracked\n").unwrap();
        fs::remove_file(source.join("delete-me.txt")).unwrap();
        fs::write(source.join(".env"), "TOKEN=abc\n").unwrap();
        fs::create_dir_all(source.join("scratch").join("nested")).unwrap();
        fs::write(
            source.join("scratch").join("nested").join("note.txt"),
            "untracked\n",
        )
        .unwrap();

        let destination_git_before = fs::read_to_string(destination.join(".git")).unwrap();
        mirror_worktree_contents(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("tracked.txt")).unwrap(),
            "modified tracked\n"
        );
        assert!(!destination.join("delete-me.txt").exists());
        assert_eq!(
            fs::read_to_string(destination.join(".env")).unwrap(),
            "TOKEN=abc\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("scratch").join("nested").join("note.txt"))
                .unwrap(),
            "untracked\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join(".git")).unwrap(),
            destination_git_before
        );
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
            let out = Command::new("git")
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
        let (branch, path) =
            create_worktree(repo.path(), &worktrees_root, "proj", Some("my-agent")).unwrap();
        assert_eq!(branch, "my-agent");
        assert!(path.ends_with("proj/my-agent"));
        assert!(path.exists());
    }

    #[test]
    fn create_worktree_generates_name_when_none() {
        let repo = init_test_repo();
        let worktrees_root = repo.path().join("agents");
        let (branch, path) = create_worktree(repo.path(), &worktrees_root, "proj", None).unwrap();
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

    #[test]
    fn parse_github_url_strips_extra_path() {
        // URLs with extra path segments after owner/repo
        assert_eq!(
            parse_github_owner_repo("https://github.com/octocat/Hello-World/tree/main"),
            Some("octocat/Hello-World".to_string()),
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
            let out = Command::new("git")
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
}
