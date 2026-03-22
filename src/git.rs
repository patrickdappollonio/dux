use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::model::{ChangedFile, FileStage};

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
    let branch_name = docker_style_name();
    let project_root = worktrees_root.join(project_name);
    fs::create_dir_all(&project_root)?;
    let worktree_path = project_root.join(&branch_name);
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "worktree",
            "add",
            "-b",
            &branch_name,
            worktree_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let canonical = worktree_path.canonicalize().unwrap_or(worktree_path);
    Ok((branch_name, canonical))
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
        .args(["-C", wt.as_ref(), "status", "--porcelain"])
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
                stage: FileStage::Unstaged,
            });
            continue;
        }

        if index_status != ' ' {
            staged.push(ChangedFile {
                status: index_status.to_string(),
                path: path.clone(),
                additions: 0,
                deletions: 0,
                stage: FileStage::Staged,
            });
        }

        if worktree_status != ' ' {
            unstaged.push(ChangedFile {
                status: worktree_status.to_string(),
                path: path.clone(),
                additions: 0,
                deletions: 0,
                stage: FileStage::Unstaged,
            });
        }
    }

    if let Ok(ns) = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--numstat"])
        .output()
    {
        if ns.status.success() {
            let stats = parse_numstat(&ns.stdout);
            for file in &mut unstaged {
                if let Some(&(a, d)) = stats.get(&file.path) {
                    file.additions = a;
                    file.deletions = d;
                }
            }
        }
    }

    if let Ok(ns) = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--cached", "--numstat"])
        .output()
    {
        if ns.status.success() {
            let stats = parse_numstat(&ns.stdout);
            for file in &mut staged {
                if let Some(&(a, d)) = stats.get(&file.path) {
                    file.additions = a;
                    file.deletions = d;
                }
            }
        }
    }

    Ok((staged, unstaged))
}

fn parse_numstat(raw: &[u8]) -> HashMap<String, (usize, usize)> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let add = parts.next()?.parse::<usize>().ok()?;
            let del = parts.next()?.parse::<usize>().ok()?;
            let path = parts.next()?.to_string();
            Some((path, (add, del)))
        })
        .collect()
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

pub fn staged_diff(worktree_path: &Path) -> Result<String> {
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--cached"])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

/// Return the contents of a file as it exists at HEAD, or `None` for new
/// (untracked) files. Uses the plumbing command `cat-file` which is immune
/// to user configuration.
pub fn file_at_head(worktree_path: &Path, path: &str) -> Result<Option<String>> {
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
    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
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

pub fn docker_style_name() -> String {
    use petname::{Generator, Petnames};

    Petnames::default()
        .generate_one(2, "-")
        .expect("petname generation should not fail")
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
}
