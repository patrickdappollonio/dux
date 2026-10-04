//! Fourth adversarial review of the worktree-removal races branch. Each test
//! reproduces a defect against the branch's own stated rules.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{
    sample_project, sample_session, sample_standalone_session, test_engine,
};
use crate::engine::{BeginDeleteSessionOutcome, Engine};
use crate::ids::TabId;
use crate::process_sessions::ProcessSession;
use crate::pty::PtyClient;
use crate::storage::PendingWorktreeRemoval;

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

fn setsid_command(cwd: &Path, script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.current_dir(cwd).args(["-c", script]);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    command
}

/// A session leader started with `setsid` in `cwd` that put a job in the
/// background and exited. Returns the session and the job's pid.
fn leaderless_job_in(cwd: &Path, pidfile: &Path) -> (ProcessSession, i32) {
    let mut leader = setsid_command(
        cwd,
        &format!(
            "sleep 300 > /dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ),
    )
    .spawn()
    .expect("spawn the leader");
    let session = ProcessSession::started_now(leader.id());
    let _ = leader.wait();
    let job: i32 = std::fs::read_to_string(pidfile)
        .expect("job pid")
        .trim()
        .parse()
        .unwrap();
    assert!(alive(job), "the job outlives its leader");
    (session, job)
}

fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

fn kill(pid: i32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
}

/// The deleted agent's CLI, still running, so the removal is parked waiting
/// for it (nothing in these tests reaps it).
fn running_cli(engine: &mut Engine, worktree: &Path) {
    let tab = engine.sessions[0].slot_tab_id().to_owned();
    let client = PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "sleep 60".to_string()],
        worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    if let Some(process) = client.process_session() {
        engine.process_registry.register(
            &engine.sessions[0].id.clone(),
            process,
            client.spawn_dir(),
        );
    }
    engine.providers.insert(TabId::new(tab.as_str()), client);
}

/// "Crash" the first run: the pending row is all the next start has.
fn crash(first: Engine) -> PendingWorktreeRemoval {
    let mut first = first;
    let rows = first
        .session_store
        .load_pending_worktree_removals()
        .unwrap();
    assert_eq!(rows.len(), 1, "the delete recorded its removal");
    for entry in std::mem::take(&mut first.terminating_ptys) {
        entry.client.force_terminate();
    }
    drop(first);
    rows[0].clone()
}

/// The next start finishes the recorded removal; waits for its final.
fn resume_at_next_start(repo: &Path, row: &PendingWorktreeRemoval) {
    let (mut next, _next_tmp) = test_engine();
    next.config.shutdown_timeout_seconds = 1;
    next.projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    next.session_store
        .insert_pending_worktree_removal(row)
        .unwrap();
    next.resume_pending_worktree_removals();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(crate::worker::WorkerEvent::StatusOpCompleted { .. }) =
            next.worker_rx.recv_timeout(Duration::from_millis(100))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the resumed removal never reported"
        );
    }
}

/// A standalone agent's processes, live or retired, are never ended, and the
/// owner kind is kept for life. A standalone agent ran in a folder inside a
/// managed agent's worktree; the user deleted both (the managed one with its
/// worktree). The standalone agent's leader exits while the managed agent's
/// removal is still waiting, and what it left running is written into that
/// removal's pending row as ordinary evidence: the row carries no owner kind.
/// dux then quits uncleanly. The next start finishes the removal from the row
/// with an empty registry, kills the standalone agent's job, and removes the
/// worktree around it.
#[test]
fn a_removal_finished_at_the_next_start_never_ends_a_standalone_agents_job() {
    let (mut first, tmp) = test_engine();
    first.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    first
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut first, tmp.path(), &repo, "agent");
    running_cli(&mut first, &worktree);

    let inside = worktree.join("frontend");
    std::fs::create_dir_all(&inside).unwrap();
    first.sessions.push(sample_standalone_session(
        "s-alone",
        inside.to_str().unwrap(),
    ));
    let (alone, job) = leaderless_job_in(&inside, &tmp.path().join("job.pid"));
    first
        .process_registry
        .register_standalone("s-alone", alone, &inside);

    // Delete the standalone agent (its record only).
    assert!(matches!(
        first.begin_delete_session("s-alone", false, None),
        BeginDeleteSessionOutcome::Inline { .. }
    ));
    first.finish_delete_session_memory("s-alone");

    // Delete the managed agent with its worktree; it waits for its CLI.
    assert!(matches!(
        first.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    first.finish_delete_session_memory("s-agent");

    // The standalone agent's leader-exit hook records what it left running.
    let found = crate::process_sessions::survivors_at_leader_exit(alone);
    first.process_registry.record_survivors(alone, &found);

    let row = crash(first);
    assert!(alive(job), "the job is still running when dux next starts");
    resume_at_next_start(&repo, &row);

    let still_running = alive(job);
    kill(job);
    assert!(
        still_running,
        "the next start killed pid {job}, a job a standalone agent left running in {}; the \
         pending row recorded it as something to end ({:?} / {:?}) with no owner kind",
        inside.display(),
        row.process_sessions,
        row.process_snapshot
    );
}

/// "Everything it would end persisted and kept current": a session registered
/// in the folder after the delete recorded its removal (a startup-command
/// rerun claimed just before the delete spawns its command just after it) is
/// ended by the live dispatch, which reads the in-memory registry, but is never
/// written into the pending row while its leader runs. After an unclean quit
/// the next start finishes the removal from the row alone and removes the
/// worktree out from under the still-running command.
#[test]
fn a_session_registered_after_the_delete_is_kept_current_in_the_pending_row() {
    let (mut first, tmp) = test_engine();
    first.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    first
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut first, tmp.path(), &repo, "agent");
    running_cli(&mut first, &worktree);

    // The rerun is claimed (as the surface does on the engine thread) ...
    let claim = first
        .claim_startup_rerun("s-agent", "agent", worktree.to_str().unwrap())
        .expect("rerun claimed");

    // ... the delete lands ...
    assert!(matches!(
        first.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    first.finish_delete_session_memory("s-agent");

    // ... and the rerun's worker spawns its command and registers it, exactly
    // as `run_claimed_startup_command` does.
    let mut command = setsid_command(&worktree, "exec sleep 300")
        .spawn()
        .expect("spawn the startup command");
    let session = ProcessSession::started_now(command.id());
    claim.run.register_session(session, &worktree);
    // The registry's writer thread saves each change a moment after it is
    // made (never on the engine thread); the crash comes after that moment.
    assert!(first.process_registry.flush(Duration::from_secs(10)));

    let row = crash(first);
    // The crash released dux's hold; the command itself keeps running.
    drop(claim);
    let recorded = row.process_sessions.contains(&session);

    resume_at_next_start(&repo, &row);
    let still_running = command.try_wait().ok().flatten().is_none();
    let removed = !worktree.exists() || !crate::git::worktree_is_registered(&repo, &worktree);
    let _ = command.kill();
    let _ = command.wait();
    assert!(
        !(removed && still_running),
        "the next start removed the worktree at {} while the startup command (pid {}) was \
         still running in it",
        worktree.display(),
        session.sid
    );
    assert!(
        recorded,
        "the pending row never learned of session {} registered in {} after the delete: {:?}",
        session.sid,
        worktree.display(),
        row.process_sessions
    );
}

/// "The manager checks the path before it claims anything; it refuses anything
/// that isn't a managed worktree, claiming nothing." A request for a folder
/// INSIDE a live agent's managed worktree passes the path-only pre-check
/// (it lies under the project's managed root) and is announced as a removal
/// before git's listing is consulted, so until the worker gets round to
/// refusing it as not managed, work in that folder of a live agent is refused
/// with "dux is removing the worktree ... The agent that owned it was deleted".
#[test]
fn the_manager_claims_nothing_for_a_folder_inside_a_live_agents_worktree() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let inside = worktree.join("src");
    std::fs::create_dir_all(&inside).unwrap();

    let admission = engine
        .admit_manager_removal("p1", &inside, false)
        .expect("known project");
    let claimed = engine.worktree_ops().is_being_removed(&inside);
    let save = engine.worktree_ops().hold(
        inside.join("main.rs"),
        crate::worktree_ops::WorktreeOpKind::EditorWrite,
    );
    let outcome = match admission {
        crate::worktree_manager::RemovalAdmission::Refused(outcome) => Ok(outcome),
        crate::worktree_manager::RemovalAdmission::Admitted(removal) => {
            drop(save);
            removal.run()
        }
    };
    assert!(
        matches!(
            outcome,
            Ok(crate::worktree_manager::RemovalOutcome::NotManaged)
        ),
        "{outcome:?}"
    );
    assert!(
        !claimed,
        "{} is not a managed worktree, yet the manager announced its removal before checking, \
         refusing an editor save into the live agent's worktree meanwhile",
        inside.display()
    );
}
