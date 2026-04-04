use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use content_inspector::{ContentType, inspect};

use crate::model::ChangedFile;

enum DiffStat {
    Text(usize, usize),
    Binary,
}

const NULL_DEVICE: &str = "/dev/null";

pub fn current_branch(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "branch",
            "--show-current",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", repo_path.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git branch failed for {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
            "--porcelain",
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

pub fn pull_current_branch(repo_path: &Path) -> Result<String> {
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
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn create_worktree(
    repo_path: &Path,
    worktrees_root: &Path,
    project_name: &str,
) -> Result<(String, PathBuf)> {
    create_worktree_from_start_point(repo_path, worktrees_root, project_name, None)
}

pub fn create_worktree_from_start_point(
    repo_path: &Path,
    worktrees_root: &Path,
    project_name: &str,
    start_point: Option<&str>,
) -> Result<(String, PathBuf)> {
    let branch_name = docker_style_name();
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
            "--porcelain",
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

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let bytes = line.as_bytes();
        let index_status = bytes[0] as char;
        let worktree_status = bytes[1] as char;
        let path = line[3..].to_string();

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
        .args(["-C", wt.as_ref(), "diff", "--numstat"])
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
        .args(["-C", wt.as_ref(), "diff", "--cached", "--numstat"])
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
            "--",
            NULL_DEVICE,
            rel_path,
        ])
        .output()
        .ok()?;

    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }

    parse_numstat_line(String::from_utf8_lossy(&output.stdout).lines().next()?)
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
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let _ = parts.next()?;
            let _ = parts.next()?;
            let path = parts.next()?.to_string();
            Some((path, parse_numstat_line(line)?))
        })
        .collect()
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
    use petname::{Generator, Petnames};

    Petnames::default()
        .generate_one(2, "-")
        .expect("petname generation should not fail")
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
}
