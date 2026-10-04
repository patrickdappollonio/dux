//! Review 12: reproductions.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{BeginDeleteSessionOutcome, Engine};

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

fn init_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--initial-branch=main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
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

fn pump_until_removed(engine: &mut Engine) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        for removal in engine.reap_terminating_ptys().removals {
            let _ = engine.dispatch_deferred_worktree_removal(removal);
        }
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        let outcome = match &event {
            crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } => {
                Some(result.clone().map(|_| ()))
            }
            _ => None,
        };
        engine.process_worker_event(event);
        if let Some(outcome) = outcome {
            return outcome;
        }
    }
    panic!("the removal never reported");
}

/// A standalone agent recorded at a symbolic link INSIDE a worktree (the
/// link points outside it). The containment rule resolves the link before
/// comparing, so the agent is judged to live outside the worktree; the
/// worktree's removal then deletes the very path the agent is recorded at.
#[test]
fn review12_a_worktree_removal_keeps_a_standalone_agent_recorded_at_a_link_inside_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = worktree.join("work");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            link.to_str().unwrap(),
        ));
    let started = engine.begin_delete_session("s-agent", true, None);
    if matches!(started, BeginDeleteSessionOutcome::AsyncStarted { .. }) {
        engine.finish_delete_session_memory("s-agent");
        let outcome = pump_until_removed(&mut engine);
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "the standalone agent's recorded folder (a link inside the worktree) was \
             deleted with the worktree (outcome {outcome:?})"
        );
    }
}

/// The same, for the changes pane deleting an untracked folder that holds
/// the link a standalone agent is recorded at.
#[test]
fn review12_a_changes_pane_folder_delete_keeps_a_standalone_agent_recorded_at_a_link_inside_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let dir = worktree.join("dir");
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("work");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            link.to_str().unwrap(),
        ));
    let _ = engine.apply(crate::engine::Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "dir".to_string(),
        is_untracked: true,
        confirmed: crate::git::ConfirmedEntry::Folder { files: 1 },
    });
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the standalone agent's recorded folder (a link inside the deleted folder) is gone"
    );
}

/// The TUI discards a file through `Engine::apply(Command::DiscardFile)` on
/// its UI thread. For an untracked symbolic link that discard now clears the
/// delete right there: `DestructiveCheck::clear` opens the session database
/// (whose `open` runs the migration, which begins an IMMEDIATE transaction)
/// and reads the process table, all on the engine thread. While another
/// connection holds the database's write lock (the registry's writer, a web
/// worker), the UI thread sits in SQLite's busy wait.
#[test]
fn review12_discarding_an_untracked_link_never_waits_on_the_database_on_the_engine_thread() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, worktree.join("link")).unwrap();

    // Another connection holds the write lock for a moment.
    let blocker = rusqlite::Connection::open(&engine.paths.sessions_db_path).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE;").unwrap();

    let started = Instant::now();
    let outcome = engine.apply(crate::engine::Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "link".to_string(),
        is_untracked: true,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    let took = started.elapsed();
    blocker.execute_batch("ROLLBACK;").unwrap();
    assert!(
        took < Duration::from_secs(1),
        "the engine thread waited {took:?} on the session database (outcome {:?})",
        outcome.err()
    );
}

/// Deleting a symbolic link is judged at the link's own path, never followed:
/// it leaves its target where it is. But the claim a link's delete takes is
/// keyed by `path_key`, which canonicalizes the link, so the claim lands on
/// the TARGET: an operation running in the target refuses the link's delete,
/// and while the claim lives, the target itself reads as being removed.
#[test]
fn review12_a_links_delete_claims_the_link_not_its_target() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let other = agent_worktree(&mut engine, tmp.path(), &repo, "other");
    // An untracked link in one agent's worktree that points at another's.
    std::os::unix::fs::symlink(&other, worktree.join("other-link")).unwrap();

    // A pull is running in the OTHER worktree.
    let _pull = engine
        .worktree_ops()
        .hold(&other, crate::worktree_ops::WorktreeOpKind::Pull)
        .unwrap();

    let outcome = engine.apply(crate::engine::Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "other-link".to_string(),
        is_untracked: true,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(
        outcome.is_ok(),
        "deleting a link was refused over what runs in its target: {:?}",
        outcome.err()
    );
}

/// The other half: while a link's delete holds its claim, the link's target
/// (another agent's worktree) refuses new work as if it were being removed.
#[test]
fn review12_a_links_claim_does_not_mark_its_target_as_being_removed() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let other = agent_worktree(&mut engine, tmp.path(), &repo, "other");
    let link = worktree.join("other-link");
    std::os::unix::fs::symlink(&other, &link).unwrap();
    let ops = engine.worktree_ops().clone();
    let _claim = ops.claim_for_destructive(&link).expect("the link's claim");
    assert!(
        ops.hold(&other, crate::worktree_ops::WorktreeOpKind::Pull)
            .is_ok(),
        "a pull in the other agent's worktree was refused because a link to it is being deleted"
    );
}

/// The editor's delete holds its own root, then claims the target. A link
/// pointing at a folder AROUND the root (`up -> ..`) is claimed at its
/// target, whose holders include the editor's own root hold, so deleting
/// such a link is refused every time, by the delete itself.
#[test]
fn review12_the_editor_can_delete_a_link_to_a_folder_around_its_root() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let link = worktree.join("up");
    std::os::unix::fs::symlink(worktree.parent().unwrap(), &link).unwrap();
    let ops = engine.worktree_ops().clone();
    // What `guard_destructive_targets` does: hold the root, claim the target.
    let _root = ops
        .hold(&worktree, crate::worktree_ops::WorktreeOpKind::EditorWrite)
        .unwrap();
    let claim = ops.claim_for_destructive(&link);
    assert!(
        claim.is_ok(),
        "a link's delete was refused by the editor's own hold on its root: {:?}",
        claim.err()
    );
}

// Tests added with the fixes for the twelfth review.

/// The engine thread never reads the session database or the process table
/// for a destructive command: every changes-pane delete (a link, a folder, a
/// nested repository) only claims and asks the engine's own state on it, and
/// clears and deletes on a worker. The hooks panic if a blocking read runs on
/// the engine thread during the command.
#[test]
fn destructive_commands_never_read_the_store_or_the_process_table_on_the_engine_thread() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, worktree.join("link")).unwrap();
    std::fs::create_dir_all(worktree.join("scratch")).unwrap();
    std::fs::write(worktree.join("scratch").join("a.txt"), "a").unwrap();
    init_repo(&worktree.join("nested"));
    for (path, confirmed) in [
        ("link", crate::git::ConfirmedEntry::File),
        ("scratch", crate::git::ConfirmedEntry::Folder { files: 1 }),
        ("nested", crate::git::ConfirmedEntry::Repository),
    ] {
        let reaction = engine
            .apply(crate::engine::Command::DiscardFile {
                worktree_path: worktree.clone(),
                path: path.to_string(),
                is_untracked: true,
                confirmed,
            })
            .unwrap_or_else(|err| panic!("{path}: {err}"));
        assert!(
            matches!(reaction, crate::engine::EventReaction::Status(ref s) if s.key.is_some()),
            "{path}: a keyed busy status, the delete running on a worker"
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(
                Instant::now() < deadline,
                "{path}: the delete never reported"
            );
            let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            if matches!(event, crate::worker::WorkerEvent::StatusOpCompleted { .. }) {
                engine.process_worker_event(event);
                break;
            }
            engine.process_worker_event(event);
        }
        assert!(
            std::fs::symlink_metadata(worktree.join(path)).is_err() || path == "scratch",
            "{path} was deleted"
        );
    }
    assert!(real.exists(), "the link's target stays");
}

/// The hook itself: a blocking read under the engine-thread mark panics.
#[test]
#[should_panic(expected = "ran on the engine thread during a destructive command")]
fn the_engine_thread_hook_catches_a_process_table_read() {
    let _mark = crate::engine::destructive_guard::engine_thread();
    let _ = crate::process_sessions::read_process_table();
}
