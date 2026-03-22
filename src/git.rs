use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::model::ChangedFile;

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
    Ok((branch_name, worktree_path))
}

pub fn remove_worktree(repo_path: &Path, worktree_path: &Path, branch_name: &str) -> Result<()> {
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
        return Err(anyhow!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "branch",
            "-D",
            branch_name,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git branch delete failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn changed_files(worktree_path: &Path) -> Result<Vec<ChangedFile>> {
    let wt = worktree_path.to_string_lossy();

    // 1. Get file statuses via porcelain (config-immune).
    let output = Command::new("git")
        .args(["-C", wt.as_ref(), "status", "--porcelain"])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() || line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let path = line[3..].to_string();
        files.push(ChangedFile {
            status,
            path,
            additions: 0,
            deletions: 0,
        });
    }

    // 2. Get line-level stats via diff --numstat (plumbing-style output).
    if let Ok(ns) = Command::new("git")
        .args(["-C", wt.as_ref(), "diff", "--numstat"])
        .output()
    {
        if ns.status.success() {
            let text = String::from_utf8_lossy(&ns.stdout);
            let stats: HashMap<String, (usize, usize)> = text
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split('\t');
                    let add = parts.next()?.parse::<usize>().ok()?;
                    let del = parts.next()?.parse::<usize>().ok()?;
                    let path = parts.next()?.to_string();
                    Some((path, (add, del)))
                })
                .collect();
            for file in &mut files {
                if let Some(&(a, d)) = stats.get(&file.path) {
                    file.additions = a;
                    file.deletions = d;
                }
            }
        }
    }

    Ok(files)
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
    fn docker_name_uses_dash() {
        assert!(docker_style_name().contains('-'));
    }
}
