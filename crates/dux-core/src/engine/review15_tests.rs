//! Review 15: reproductions.

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

/// HEAD tracks `dir/a.txt`. The agent replaced the folder `dir` with a plain,
/// untracked FILE called `dir`. git lists ` D dir/a.txt` (a tracked file
/// deleted) and `?? dir`. Restoring `dir/a.txt` runs `git checkout --
/// dir/a.txt`, which unlinks the untracked file `dir` to make room for the
/// folder: a file the user never confirmed deleting is gone.
#[test]
fn review15_restoring_a_file_under_a_path_now_held_by_a_file_keeps_that_file() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join("dir")).unwrap();
    std::fs::write(repo.join("dir").join("a.txt"), "tracked\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "dir"]);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    std::fs::remove_dir_all(worktree.join("dir")).unwrap();
    std::fs::write(worktree.join("dir"), "irreplaceable notes\n").unwrap();
    let untracked = crate::git::discard_classify(&worktree, "dir/a.txt").unwrap();
    assert!(
        !untracked,
        "git reports `dir/a.txt` as a tracked, deleted file"
    );
    let outcome = engine.apply(Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "dir/a.txt".to_string(),
        is_untracked: untracked,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    let still_a_file = std::fs::symlink_metadata(worktree.join("dir")).is_ok_and(|m| m.is_file());
    assert!(
        still_a_file,
        "the untracked file `dir` was deleted by a discard confirmed as restoring `dir/a.txt` \
         (outcome: {:?})",
        outcome.as_ref().map(|_| ()).map_err(|e| e.to_string())
    );
}

/// The tracked file `notes` was replaced by a symbolic link to a folder
/// elsewhere, and a standalone agent was started AT the link (its folder is
/// recorded as `<worktree>/notes`). An untracked link like that is cleared
/// before it is deleted, because "an agent's folder can be the link itself";
/// restoring the tracked file removes the very same link with no clearance,
/// and the standalone agent's folder path is now a plain file.
#[test]
fn review15_restoring_a_tracked_file_over_a_link_that_is_a_standalone_agents_folder_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::remove_file(worktree.join("notes")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, worktree.join("notes")).unwrap();
    let folder = worktree.join("notes");
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            folder.to_str().unwrap(),
        ));
    let untracked = crate::git::discard_classify(&worktree, "notes").unwrap();
    assert!(!untracked, "git reports `notes` as a tracked file");
    let outcome = engine.apply(Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "notes".to_string(),
        is_untracked: untracked,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(
        std::fs::symlink_metadata(&folder).is_ok_and(|m| m.file_type().is_symlink()),
        "the link that IS the standalone agent's folder was removed by a restore, with no \
         occupancy question (outcome: {:?})",
        outcome.as_ref().map(|_| ()).map_err(|e| e.to_string())
    );
}

/// The same with the link one level up: HEAD tracks `dir/a.txt`, `dir` is now
/// a link to a folder elsewhere, and a standalone agent was started at
/// `<worktree>/dir`. Restoring `dir/a.txt` makes git replace the link with a
/// real directory.
#[test]
fn review15_restoring_a_file_beneath_a_link_that_is_a_standalone_agents_folder_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join("dir")).unwrap();
    std::fs::write(repo.join("dir").join("a.txt"), "tracked\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "dir"]);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::remove_dir_all(worktree.join("dir")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, worktree.join("dir")).unwrap();
    let folder = worktree.join("dir");
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            folder.to_str().unwrap(),
        ));
    let untracked = crate::git::discard_classify(&worktree, "dir/a.txt").unwrap();
    assert!(!untracked);
    let outcome = engine.apply(Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "dir/a.txt".to_string(),
        is_untracked: untracked,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(
        std::fs::symlink_metadata(&folder).is_ok_and(|m| m.file_type().is_symlink()),
        "the link that IS the standalone agent's folder was replaced by a restore \
         (outcome: {:?})",
        outcome.as_ref().map(|_| ()).map_err(|e| e.to_string())
    );
}
