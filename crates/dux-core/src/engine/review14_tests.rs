//! Review 14: reproductions.

use std::path::{Path, PathBuf};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{Command, Engine};

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::fixture_git()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository whose HEAD tracks `f.txt` and a plain FILE called `notes`.
fn init_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--initial-branch=main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    std::fs::write(repo.join("notes"), "a tracked file\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "init"]);
}

fn agent_worktree(engine: &mut Engine, root: &Path, repo: &Path, name: &str) -> PathBuf {
    let worktree = root.join("worktrees").join("p1-name").join(name);
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        repo,
        &["worktree", "add", "-b", name, worktree.to_str().unwrap()],
    );
    let mut session = sample_session(&format!("s-{name}"), "p1", name);
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine.sessions.push(session);
    worktree
}

/// The tracked file `notes` has been replaced in the working tree by a folder
/// of the same name, and a standalone agent works in a folder inside it. git
/// lists only ` D notes` (a tracked file deleted), so the changes pane offers
/// "discard" as restoring one file from HEAD. That discard runs
/// `git checkout -- notes`, which removes the folder in the way recursively,
/// so the standalone agent's folder (and everything else in `notes/`) is
/// deleted with no claim, no occupancy question and no clearance.
#[test]
fn review14_discarding_a_deleted_tracked_file_keeps_a_standalone_agents_folder_in_its_place() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    // The agent turned the file into a folder.
    std::fs::remove_file(worktree.join("notes")).unwrap();
    let folder = worktree.join("notes").join("mine");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("precious.txt"), "work\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            folder.to_str().unwrap(),
        ));
    // What both surfaces ask before the discard: git calls it tracked.
    let untracked = crate::git::discard_classify(&worktree, "notes").unwrap();
    assert!(!untracked, "git reports `notes` as a tracked, deleted file");
    let outcome = engine.apply(Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "notes".to_string(),
        is_untracked: untracked,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(
        folder.join("precious.txt").exists(),
        "the standalone agent's folder inside `notes/` was deleted by a discard confirmed as \
         restoring one file (outcome: {:?})",
        outcome.as_ref().map(|_| ()).map_err(|e| e.to_string())
    );
}

/// The same, with a dux project's repository (a nested clone) where the
/// tracked file was: discarding the "deleted file" deletes the repository,
/// history included.
#[test]
fn review14_discarding_a_deleted_tracked_file_keeps_a_projects_repository_in_its_place() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    std::fs::remove_file(worktree.join("notes")).unwrap();
    let nested = worktree.join("notes");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "--initial-branch=main"]);
    git(&nested, &["config", "user.email", "t@example.com"]);
    git(&nested, &["config", "user.name", "t"]);
    std::fs::write(nested.join("own.txt"), "own\n").unwrap();
    git(&nested, &["add", "."]);
    git(&nested, &["commit", "-m", "own history"]);
    engine
        .projects
        .push(sample_project("p2", nested.to_str().unwrap()));
    let untracked = crate::git::discard_classify(&worktree, "notes").unwrap();
    assert!(!untracked, "git reports `notes` as a tracked, deleted file");
    let outcome = engine.apply(Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "notes".to_string(),
        is_untracked: untracked,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(
        nested.join(".git").exists(),
        "project p2's repository (with its history) was deleted by a discard confirmed as \
         restoring one file (outcome: {:?})",
        outcome.as_ref().map(|_| ()).map_err(|e| e.to_string())
    );
}
