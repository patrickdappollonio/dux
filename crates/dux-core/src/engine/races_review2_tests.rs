//! Adversarial review (second pass) of the worktree-removal coordination.

use std::path::{Path, PathBuf};

use crate::engine::Engine;
use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::worktree_manager::RemovalAdmission;

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

fn repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    repo
}

fn agent_worktree(
    engine: &mut Engine,
    repo: &Path,
    worktree: &Path,
    id: &str,
    branch: &str,
) -> PathBuf {
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        repo,
        &["worktree", "add", "-b", branch, worktree.to_str().unwrap()],
    );
    let mut session = sample_session(id, "p1", branch);
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine.sessions.push(session);
    worktree.to_path_buf()
}

/// A worktree whose agent was deleted with the worktree kept, and a DORMANT
/// managed agent whose own worktree sits inside it. dux names worktrees after
/// branches, so this is what an ordinary sequence produces: agent `a` is
/// renamed to `b` (its folder keeps the name `a`), a new agent `a/c` is
/// created (its worktree lands at `.../a/c`, inside the first), and the first
/// agent is deleted keeping its worktree. Removing that leftover from the
/// worktree manager then deletes the dormant agent's worktree with it, since
/// `git worktree remove --force` deletes everything under the folder. Every
/// other occupancy check in the branch asks "in or under"; the manager's
/// "attached" check alone still asks "equal", and the PTY check only sees
/// running agents.
#[test]
fn the_manager_never_removes_a_folder_a_dormant_agents_worktree_lives_in() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let base = tmp.path().join("worktrees").join("p1-name");
    let outer = agent_worktree(&mut engine, &repo, &base.join("a"), "s-a", "a");
    git(&repo, &["branch", "-m", "a", "b"]);
    let inner = agent_worktree(&mut engine, &repo, &base.join("a").join("c"), "s-ac", "a/c");
    std::fs::write(inner.join("precious.txt"), "uncommitted work").unwrap();
    // The outer agent is deleted, keeping its worktree.
    engine.sessions.retain(|session| session.id != "s-a");

    let admission = engine
        .admit_manager_removal("p1", &outer, false)
        .expect("known project");
    if let RemovalAdmission::Admitted(removal) = admission {
        let outcome = removal.run();
        assert!(
            inner.join("precious.txt").exists(),
            "removing {} from the worktree manager deleted agent s-ac's worktree {} \
             (and its uncommitted file) with it; outcome: {outcome:?}",
            outer.display(),
            inner.display()
        );
        panic!(
            "the manager admitted the removal of {} while agent s-ac's worktree lives inside it",
            outer.display()
        );
    }
}

/// A session number dux recorded this run, from an agent that was deleted
/// with its worktree kept. Its leader exited long ago and the number was
/// reused by an unrelated program: a session leader that started a
/// background job and exited, leaving the job in a leaderless session. A
/// later removal of that folder (the worktree manager here) takes every
/// leaderless session with a recorded number for the original one, whatever
/// its members' start times say, so the unrelated program, started an hour
/// after the recorded session, is killed. The resumed-removal path refuses
/// that without identities; the this-run path has no such guard.
#[test]
fn a_removal_this_run_does_not_kill_an_unrelated_program_that_reused_a_session_number() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(
        &mut engine,
        &repo,
        &tmp.path().join("worktrees").join("p1-name").join("kept"),
        "s-kept",
        "kept",
    );

    let pidfile = tmp.path().join("daemon.pid");
    let mut leader = std::process::Command::new("sh");
    leader.args([
        "-c",
        &format!(
            "sleep 60 > /dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ),
    ]);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        leader.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut leader = leader.spawn().expect("spawn the daemon's leader");
    let sid = leader.id();
    let _ = leader.wait();
    let daemon: i32 = std::fs::read_to_string(&pidfile)
        .expect("daemon pid")
        .trim()
        .parse()
        .unwrap();
    let daemon_pid = rustix::process::Pid::from_raw(daemon).unwrap();
    assert!(rustix::process::test_kill_process(daemon_pid).is_ok());

    // What this run recorded for the deleted agent: a PTY that led session
    // `sid` in the worktree an hour before this program existed.
    let recorded = crate::process_sessions::ProcessSession {
        sid,
        started_at: crate::process_sessions::ProcessSession::started_now(sid)
            .started_at
            .saturating_sub(3_600_000_000_000),
        boot: crate::process_sessions::current_boot(),
    };
    engine
        .process_registry
        .register("s-kept", recorded, &worktree);
    let _ = engine.process_registry.forget_agent("s-kept");
    engine.sessions.retain(|session| session.id != "s-kept");

    let admission = engine
        .admit_manager_removal("p1", &worktree, false)
        .expect("known project");
    let RemovalAdmission::Admitted(removal) = admission else {
        panic!("nothing occupies the worktree, so the manager admits its removal");
    };
    let outcome = removal.run();
    let alive = rustix::process::test_kill_process(daemon_pid).is_ok();
    let _ = rustix::process::kill_process(daemon_pid, rustix::process::Signal::KILL);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(
        alive,
        "dux killed pid {daemon}, an unrelated program started an hour after the session it \
         recorded, because the program's leaderless session reused that session's number"
    );
}

/// The nested layout from the manager test above, pinned for every other
/// removal path: an agent delete with its worktree, and a removal finished at
/// the next start. Each keeps the outer folder and names the dormant agent.
#[test]
fn every_removal_path_keeps_a_folder_a_dormant_agents_worktree_lives_in() {
    // The agent delete.
    let (mut engine, tmp) = test_engine();
    let repo_path = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo_path.to_str().unwrap()));
    let base = tmp.path().join("worktrees").join("p1-name");
    let outer = agent_worktree(&mut engine, &repo_path, &base.join("a"), "s-a", "a");
    git(&repo_path, &["branch", "-m", "a", "b"]);
    let inner = agent_worktree(
        &mut engine,
        &repo_path,
        &base.join("a").join("c"),
        "s-ac",
        "a/c",
    );
    if let Some(managed) = engine.sessions[0].workspace.as_managed_mut() {
        managed.branch_name = "b".to_string();
    }
    assert!(matches!(
        engine.begin_delete_session("s-a", true, Some(true)),
        crate::engine::BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-a");
    let message = loop {
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the removal reports");
        if let crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } = event {
            break result.expect_err("the outer worktree is kept");
        }
    };
    assert!(
        message.contains("a/c") || message.contains("s-ac-title"),
        "{message}"
    );
    assert!(inner.exists() && outer.exists());

    // A removal finished at the next start.
    let (mut engine, tmp) = test_engine();
    let repo_path = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo_path.to_str().unwrap()));
    let base = tmp.path().join("worktrees").join("p1-name");
    let outer = agent_worktree(&mut engine, &repo_path, &base.join("a"), "s-a", "a");
    git(&repo_path, &["branch", "-m", "a", "b"]);
    let inner = agent_worktree(
        &mut engine,
        &repo_path,
        &base.join("a").join("c"),
        "s-ac",
        "a/c",
    );
    let gone = engine.sessions.remove(0);
    engine
        .session_store
        .insert_pending_worktree_removal(&crate::storage::PendingWorktreeRemoval {
            session_id: gone.id.clone(),
            label: "a".to_string(),
            project_path: repo_path.to_string_lossy().into_owned(),
            managed: gone.workspace.as_managed().unwrap().clone(),
            delete_branch: Some(true),
            process_sessions: Vec::new(),
            process_snapshot: Vec::new(),
            process_registry: Default::default(),
        })
        .unwrap();
    engine.resume_pending_worktree_removals();
    let warning = match engine.worker_rx.try_recv() {
        Ok(crate::worker::WorkerEvent::PollerStatus(status)) => status,
        _ => panic!("the resumed removal is refused out loud"),
    };
    assert!(
        warning.message.contains("Kept the worktree"),
        "{}",
        warning.message
    );
    assert!(inner.exists() && outer.exists());

    // And the manager lists the outer folder as in use, naming the agent.
    let project = engine.projects[0].clone();
    let listed = crate::worktree_manager::list_manageable_worktrees_with_busy(
        &project,
        &engine.paths,
        &engine.sessions,
        engine.worktree_ops(),
        &engine.busy_folders(),
    )
    .unwrap();
    let row = listed
        .iter()
        .find(|entry| entry.path.ends_with("a"))
        .expect("the outer folder is listed");
    assert!(!row.is_removable());
    assert!(
        row.busy
            .as_deref()
            .is_some_and(|busy| busy.contains("s-ac-title")),
        "{:?}",
        row.busy
    );
}
