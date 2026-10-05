//! Review 13: reproductions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::engine::Engine;
use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::worktree_ops::{HoldOwner, WorktreeOpKind, WorktreeOps};

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

/// A detached-from-any-agent dux worktree of project p1.
fn free_worktree(engine: &mut Engine, root: &Path) -> PathBuf {
    let repo = root.join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = engine.paths.worktrees_root.join("p1-name").join("free");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &repo,
        &["worktree", "add", "-b", "free", worktree.to_str().unwrap()],
    );
    // An agent record only to borrow sample_session's project naming; removed.
    let _ = sample_session("unused", "p1", "free");
    worktree
}

/// The registry keys every hold and claim on its LEXICAL spelling and
/// compares keys by plain prefix, so a hold taken through a link whose target
/// is a folder being removed is invisible to that removal, and the removal is
/// invisible to the hold. The containment rule (any spelling of the occupant
/// under any spelling of the folder) is not what the registry asks.
#[test]
fn review13_a_hold_through_a_link_is_seen_by_a_removal_of_its_target() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let link = tmp.path().join("folder-link");
    std::os::unix::fs::symlink(&worktree, &link).unwrap();
    assert!(crate::worktree_ops::folder_contains(&worktree, &link));

    let ops = WorktreeOps::new();
    ops.hold_as(
        HoldOwner::CreateOp("create-1".into()),
        &link,
        WorktreeOpKind::CreateAgent,
    )
    .unwrap();
    assert!(
        ops.holders(&worktree)
            .contains(&WorktreeOpKind::CreateAgent),
        "an agent being created at a link to the worktree is not among the worktree's holders"
    );
}

#[test]
fn review13_a_removal_of_a_folder_refuses_new_work_through_a_link_to_it() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let link = tmp.path().join("folder-link");
    std::os::unix::fs::symlink(&worktree, &link).unwrap();

    let ops = WorktreeOps::new();
    let _claim = ops.announce_removal(&worktree);
    assert!(
        ops.hold_as(
            HoldOwner::CreateOp("create-1".into()),
            &link,
            WorktreeOpKind::CreateAgent,
        )
        .is_err(),
        "a standalone agent create at a link to a worktree being removed was admitted"
    );
}

/// The last look a removal takes (and the destructive clearance) asks
/// `stored_occupant`, whose "being created" rung asks the registry's
/// holders: a standalone agent being created at a link that resolves to the
/// worktree is not found, so the worktree is cleared for git while the create
/// is still on its way into it.
#[test]
fn review13_the_last_look_sees_an_agent_being_created_at_a_link_to_the_folder() {
    let (engine, tmp) = test_engine();
    let worktree = tmp.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let link = tmp.path().join("folder-link");
    std::os::unix::fs::symlink(&worktree, &link).unwrap();
    engine
        .worktree_ops()
        .hold_as(
            HoldOwner::CreateOp("create-1".into()),
            &link,
            WorktreeOpKind::CreateAgent,
        )
        .unwrap();
    let db = engine.paths.sessions_db_path.clone();
    let ops = engine.worktree_ops().clone();
    let occupant =
        std::thread::spawn(move || crate::engine::stored_occupant(&db, &ops, &worktree, None))
            .join()
            .unwrap()
            .unwrap();
    assert!(
        occupant.is_some(),
        "the last look found nothing in a worktree an agent is being created in through a link"
    );
}

/// A manager removal requested at a link to a worktree resolves to the
/// worktree (git removes the real folder) but claims and waits on the LINK:
/// a push running in the worktree is not waited for, and git removes the
/// folder under it.
#[test]
fn review13_a_manager_removal_requested_through_a_link_waits_for_work_in_the_worktree() {
    let (mut engine, tmp) = test_engine();
    let worktree = free_worktree(&mut engine, tmp.path());
    let link = engine
        .paths
        .worktrees_root
        .join("p1-name")
        .join("free-link");
    std::os::unix::fs::symlink(&worktree, &link).unwrap();
    let guard = engine
        .worktree_ops()
        .hold(&worktree, WorktreeOpKind::Push)
        .unwrap();
    let admission = engine.admit_manager_removal("p1", &link, false).unwrap();
    let crate::worktree_manager::RemovalAdmission::Admitted(ticket) = admission else {
        // Refused: nothing to show.
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(ticket.run()).unwrap());
    let early = rx.recv_timeout(Duration::from_secs(5));
    let still_there = worktree.exists();
    drop(guard);
    assert!(
        still_there,
        "the worktree was removed while a push held it (outcome {early:?})"
    );
}

// Tests added with the fixes for the thirteenth review.

/// A background worker that never starts is answered for, so the busy the
/// surface showed for it gets its final.
#[test]
fn a_worker_that_never_starts_is_answered_for() {
    let (mut engine, tmp) = test_engine();
    let project = crate::engine::test_support::sample_project("p1", tmp.path().to_str().unwrap());
    engine.force_worker_spawn_failure = true;
    engine.spawn_manageable_worktrees_worker(project.clone(), Some("op-1".to_string()));
    match engine.worker_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(crate::worker::WorkerEvent::ManageableWorktreesReady {
            result: Err(_),
            status_op_id,
            ..
        }) => assert_eq!(status_op_id.as_deref(), Some("op-1")),
        other => panic!("expected a failed answer, got {}", other.is_ok()),
    }
    engine.force_worker_spawn_failure = true;
    engine.spawn_project_worktrees_worker(project, Some("op-2".to_string()));
    assert!(matches!(
        engine.worker_rx.recv_timeout(Duration::from_secs(5)),
        Ok(crate::worker::WorkerEvent::ProjectWorktreesReady { result: Err(_), .. })
    ));
}

/// A destructive claim on a link covers the link alone, under the same one
/// comparison everything else uses: a hold in its target is not refused.
#[test]
fn a_link_claim_covers_its_lexical_path_only() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let ops = WorktreeOps::new();
    let _claim = ops.claim_for_destructive(&link).unwrap();
    assert!(ops.hold(&real, WorktreeOpKind::Pull).is_ok());
    assert!(ops.hold(&link, WorktreeOpKind::Pull).is_err());
}

/// A removal resumed at a start whose worker never starts gives the busy it
/// already sent a final, and its key is retired.
#[test]
fn a_resumed_removal_that_never_starts_gives_its_busy_a_final() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    let worktree = free_worktree(&mut engine, tmp.path());
    let mut session = sample_session("s-gone", "p1", "free");
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine
        .session_store
        .insert_pending_worktree_removal(&crate::storage::PendingWorktreeRemoval {
            session_id: session.id.clone(),
            label: "gone".to_string(),
            project_path: repo.to_string_lossy().into_owned(),
            managed: session.workspace.as_managed().unwrap().clone(),
            delete_branch: None,
            process_sessions: Vec::new(),
            process_snapshot: Vec::new(),
            process_registry: Default::default(),
        })
        .unwrap();
    crate::engine::fail_next_worker_spawn();
    engine.resume_pending_worktree_removals();
    let mut busy_key = None;
    let mut final_for_it = false;
    while let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(500)) {
        if let crate::worker::WorkerEvent::PollerStatus(status) = event {
            match status.tone {
                crate::statusline::StatusTone::Busy => busy_key = status.key.clone(),
                _ if status.key.is_some() && status.key == busy_key => final_for_it = true,
                _ => {}
            }
        }
    }
    let key = busy_key.expect("the resumed removal sent its busy");
    assert!(final_for_it, "the busy got a final");
    assert!(!engine.status_op_is_live(&key), "its key is retired");
    assert!(worktree.exists(), "nothing ran");
}
