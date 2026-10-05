//! Review 23: reproductions.

use std::path::Path;

use crate::engine::test_support::{sample_project, test_engine};

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

/// A project checkout on `main` that tracks a symbolic link `work` pointing at
/// a folder outside it, and a branch `feat` that deletes (or, with `replace`,
/// turns into a file) that link.
fn link_fixture(tmp: &Path, replace: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let shared = tmp.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("notes.md"), "mine\n").unwrap();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README"), "r\n").unwrap();
    std::os::unix::fs::symlink(&shared, repo.join("work")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    git(&repo, &["rm", "-q", "work"]);
    if replace {
        std::fs::write(repo.join("work"), "now a file\n").unwrap();
        git(&repo, &["add", "work"]);
    }
    git(&repo, &["commit", "-q", "-m", "drop the link"]);
    git(&repo, &["switch", "-q", "main"]);
    assert!(
        repo.join("work")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    (repo, shared)
}

fn standalone_at_link_survives_switch(replace: bool) {
    let (mut engine, tmp) = test_engine();
    let (repo, _shared) = link_fixture(tmp.path(), replace);
    let folder = repo.join("work");
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-work",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    let still_link = folder
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink());
    assert!(
        still_link,
        "the switch removed the standalone agent's folder {} (a link the checkout tracks) \
         without asking (result {result:?})",
        folder.display()
    );
}

/// A standalone agent's folder is the link `repo/work` the project tracks.
/// Switching the project to a branch that deletes the link makes git unlink
/// the agent's folder: the move check only asks about deleted paths through
/// the folders they leave empty, and a link at the top level has none.
#[test]
fn review23_switch_deletes_a_standalone_agents_folder_that_is_a_tracked_link() {
    standalone_at_link_survives_switch(false);
}

/// The same link, replaced by a tracked file: git unlinks the agent's folder
/// and writes a file there, because the check leaves every path HEAD tracks
/// to git.
#[test]
fn review23_switch_replaces_a_standalone_agents_folder_that_is_a_tracked_link() {
    standalone_at_link_survives_switch(true);
}

/// The removal's order ends "last look, git", but the git step runs git TWICE
/// when the first `git worktree remove --force` fails while the worktree is
/// still registered: it sleeps a second and runs it again, with no look in
/// between. A shell dux started that walks into the worktree during that
/// second (here: a process standing in it, started after the look) has the
/// folder removed from under it. The first failure is made with a lock that
/// is lifted during the pause, standing in for any transient refusal.
#[test]
fn review23_removal_retries_git_after_the_last_look_without_looking_again() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README"), "r\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let worktree = tmp.path().join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            worktree.to_str().unwrap(),
        ],
    );
    git(&repo, &["worktree", "lock", worktree.to_str().unwrap()]);
    // The caller's last look happened here: nothing stands in the folder.
    let r = repo.clone();
    let w = worktree.clone();
    let removal = std::thread::spawn(move || {
        crate::git::remove_worktree_keep_branch(&r, &w).map_err(|e| format!("{e:#}"))
    });
    std::thread::sleep(std::time::Duration::from_millis(400));
    // During the pause: something dux started walks in, and the transient
    // refusal clears.
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let walked_in = Kill(
        std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&worktree)
            .spawn()
            .unwrap(),
    );
    git(&repo, &["worktree", "unlock", worktree.to_str().unwrap()]);
    let result = removal.join().unwrap();
    let removed = !worktree.exists();
    drop(walked_in);
    assert!(
        !removed,
        "git ran a second time a second after the last look and removed {} while a process \
         stood in it (result {result:?})",
        worktree.display()
    );
}

// ---- the removal's order, and its retry ----

/// The order every removal keeps, pinned: holds, claims, spawns, flush, last
/// look; git runs after.
#[test]
fn a_removal_waits_holds_claims_spawns_flush_then_looks() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("wt");
    std::fs::create_dir_all(&folder).unwrap();
    let ops = crate::worktree_ops::WorktreeOps::new();
    let registry = crate::process_sessions::AgentProcessRegistry::default();
    let crate::worktree_ops::RemovalClaim::Lead(lease) = ops.announce_removal(&folder) else {
        panic!("the only removal leads");
    };
    let _ = crate::engine::events::removal_steps::take();
    let looked = crate::engine::wait_then_last_look(
        &lease,
        folder.to_str().unwrap(),
        std::time::Duration::from_secs(5),
        |_| {},
        &[&registry],
        || None,
    );
    assert!(looked.is_ok(), "{looked:?}");
    assert_eq!(
        crate::engine::events::removal_steps::take(),
        vec!["holds", "claims", "spawns", "flush", "look"]
    );
}

/// git's retry goes through the whole look again first: an occupant that
/// arrived after the first look keeps the folder, and git runs only once.
#[test]
fn a_removal_retry_looks_again_and_keeps_the_folder_for_a_new_occupant() {
    let looks = std::cell::Cell::new(0);
    let runs = std::cell::Cell::new(0);
    let result: Result<(), String> = crate::engine::remove_after_last_look(
        || {
            looks.set(looks.get() + 1);
            if looks.get() == 1 {
                Ok(())
            } else {
                Err("the worktree at x was kept: a terminal walked in".to_string())
            }
        },
        || {
            runs.set(runs.get() + 1);
            Err(anyhow::Error::new(crate::git::RemovalWorthRetrying {
                git_error: "locked".to_string(),
            }))
        },
    );
    assert_eq!(looks.get(), 2, "looked again before the retry");
    assert_eq!(runs.get(), 1, "and did not run git again");
    assert!(result.unwrap_err().contains("walked in"));
}
