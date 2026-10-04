//! Third adversarial review of the worktree-removal races branch. Each test
//! reproduces a defect against the branch's own stated rules.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{
    sample_project, sample_session, sample_standalone_session, test_engine,
};
use crate::engine::{BeginDeleteSessionOutcome, Engine};
use crate::ids::TabId;
use crate::process_sessions::{ProcessSession, UNOWNED_PTYS};
use crate::pty::PtyClient;
use crate::worktree_manager::{RemovalAdmission, RemovalOutcome};

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

/// What a closed terminal (or an exited CLI) leaves behind: a session leader
/// started with `setsid` in `cwd` that put a job in the background and
/// exited, so the job is a member of a session whose leader is gone. Returns
/// the session as dux recorded it at spawn and the job's pid. The caller kills
/// the job.
fn leaderless_job_in(cwd: &Path, pidfile: &Path) -> (ProcessSession, i32) {
    let mut leader = std::process::Command::new("sh");
    leader.current_dir(cwd).args([
        "-c",
        &format!(
            "sleep 300 > /dev/null 2>&1 & echo $! > '{}'",
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
    let mut leader = leader.spawn().expect("spawn the leader");
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

/// Register `session` for `owner` and record what its leader left running,
/// exactly as the PTY's leader-exit hook does.
fn register_leaderless(engine: &Engine, owner: &str, session: ProcessSession, folder: &Path) {
    engine.process_registry.register(owner, session, folder);
    let found = crate::process_sessions::survivors_at_leader_exit(session);
    assert!(!found.is_empty(), "the job is recorded as a survivor");
    engine.process_registry.record_survivors(session, &found);
}

/// The worktree manager's removal (the web route sends any path it is given)
/// ends every process dux registered in the requested folder BEFORE it decides
/// whether the folder is a managed worktree at all. A request naming the
/// project's own checkout, where project terminals start, is answered "not a
/// managed worktree" and nothing is removed, but a job a closed project
/// terminal left running there (a dev server) has already been killed.
#[test]
fn the_manager_kills_nothing_for_a_folder_it_then_refuses_as_not_managed() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));

    // A project terminal opened at the repository root, closed, with a job
    // left running in it.
    let (session, job) = leaderless_job_in(&repo, &tmp.path().join("job.pid"));
    register_leaderless(&engine, UNOWNED_PTYS, session, &repo);

    let admission = engine
        .admit_manager_removal("p1", &repo, false)
        .expect("known project");
    let outcome = match admission {
        RemovalAdmission::Refused(outcome) => Ok(outcome),
        RemovalAdmission::Admitted(removal) => removal.run(),
    };
    let still_running = alive(job);
    kill(job);
    assert!(
        matches!(outcome, Ok(RemovalOutcome::NotManaged)),
        "{outcome:?}"
    );
    assert!(repo.exists());
    assert!(
        still_running,
        "the manager refused the request as not a managed worktree, but first killed pid {job}, \
         a job a closed project terminal left running in the project's folder"
    );
}

/// A standalone agent's processes are never ended. A standalone agent ran in a
/// folder inside another agent's worktree and left a job running there; the
/// user deleted the standalone agent (its record only, which never touches its
/// folder or what runs in it). Deleting the managed agent with its worktree
/// then ends that job: the standalone agent's sessions went to the "retired"
/// list, which every later removal of an enclosing folder purges.
#[test]
fn removing_a_worktree_never_ends_what_a_deleted_standalone_agent_left_running() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let inside = worktree.join("frontend");
    std::fs::create_dir_all(&inside).unwrap();
    engine.sessions.push(sample_standalone_session(
        "s-alone",
        inside.to_str().unwrap(),
    ));
    let (session, job) = leaderless_job_in(&inside, &tmp.path().join("job.pid"));
    register_leaderless(&engine, "s-alone", session, &inside);

    // Delete the standalone agent: dux's record of it, nothing else.
    assert!(matches!(
        engine.begin_delete_session("s-alone", false, None),
        BeginDeleteSessionOutcome::Inline { .. }
    ));
    engine.finish_delete_session_memory("s-alone");
    assert!(
        alive(job),
        "deleting a standalone agent leaves its job alone"
    );

    // Delete the managed agent with its worktree.
    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "the removal never reported");
        for removal in engine.reap_terminating_ptys().removals {
            let _ = engine.dispatch_deferred_worktree_removal(removal);
        }
        if let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) {
            let done = matches!(
                event,
                crate::worker::WorkerEvent::WorktreeRemoveCompleted { .. }
            );
            engine.process_worker_event(event);
            if done {
                break;
            }
        }
    }
    let still_running = alive(job);
    kill(job);
    assert!(
        still_running,
        "dux killed pid {job}, which a standalone agent started in {}; a standalone agent's \
         processes are never ended",
        inside.display()
    );
    // And while they run they occupy the folder: the worktree around them is
    // kept rather than deleted out from under them.
    assert!(
        worktree.exists(),
        "the worktree around a running standalone job is kept"
    );
}

/// A removal finished at the next start (after a crash or a forced quit) must
/// end what dux started in the worktree, as the same removal would have in the
/// run that accepted it. The agent's closed terminal left a job running in the
/// worktree. The delete records the request with a snapshot of the agent's
/// processes, but that snapshot is taken only from sessions whose leader is
/// alive: the job, whose leader is gone, is known to that run solely through
/// the in-memory survivors, which are never written down. The next start sees
/// a leaderless session with no recorded members, ends nothing, and git
/// removes the worktree out from under the still-running job.
#[test]
fn a_removal_finished_at_the_next_start_ends_a_closed_terminals_job_first() {
    let (mut first, tmp) = test_engine();
    first.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    first
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut first, tmp.path(), &repo, "agent");

    // The agent's companion terminal, already closed, left a job behind.
    let (closed, job) = leaderless_job_in(&worktree, &tmp.path().join("job.pid"));
    register_leaderless(&first, "s-agent", closed, &worktree);

    // The agent's CLI is still running, so the removal waits for it.
    let tab = first.sessions[0].slot_tab_id().to_owned();
    let client = PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "sleep 60".to_string()],
        &worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    if let Some(process) = client.process_session() {
        first
            .process_registry
            .register("s-agent", process, client.spawn_dir());
    }
    first.providers.insert(TabId::new(tab.as_str()), client);

    assert!(matches!(
        first.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    first.finish_delete_session_memory("s-agent");
    // Wait for the delete's snapshot to be written down, then "crash": the row
    // is all the next start has.
    let deadline = Instant::now() + Duration::from_secs(20);
    let row = loop {
        let rows = first
            .session_store
            .load_pending_worktree_removals()
            .unwrap();
        assert_eq!(rows.len(), 1);
        if !rows[0].process_snapshot.is_empty() {
            break rows[0].clone();
        }
        assert!(Instant::now() < deadline, "the snapshot was never recorded");
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(first);

    let (mut next, _next_tmp) = test_engine();
    next.config.shutdown_timeout_seconds = 1;
    next.projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    next.session_store
        .insert_pending_worktree_removal(&row)
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
    let still_running = alive(job);
    let removed = !worktree.exists() || !crate::git::worktree_is_registered(&repo, &worktree);
    kill(job);
    assert!(
        !(removed && still_running),
        "the next start removed the worktree at {} while pid {job}, a job the agent's closed \
         terminal left running there, was still running",
        worktree.display()
    );
}

/// The live case of the rule above: a standalone agent that still exists runs
/// in a folder inside the worktree and has a job there. The worktree is kept,
/// the message names the standalone agent, and the job is never touched.
#[test]
fn removing_a_worktree_never_ends_a_live_standalone_agents_processes() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let inside = worktree.join("frontend");
    std::fs::create_dir_all(&inside).unwrap();
    engine.sessions.push(sample_standalone_session(
        "s-alone",
        inside.to_str().unwrap(),
    ));
    let (session, job) = leaderless_job_in(&inside, &tmp.path().join("job.pid"));
    engine.process_registry.mark_standalone("s-alone");
    register_leaderless(&engine, "s-alone", session, &inside);

    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    let message = loop {
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the removal reports");
        if let crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } = event {
            break result.expect_err("the worktree is kept");
        }
    };
    let still_running = alive(job);
    kill(job);
    assert!(message.contains("standalone"), "{message}");
    assert!(
        still_running,
        "a live standalone agent's job is never ended"
    );
    assert!(worktree.exists());
}

/// A survivor recorded AFTER the removal was written down (the agent's
/// terminal closes while the delete waits) is added to the pending row, so a
/// later start ends the same set the live dispatch would have.
#[test]
fn a_survivor_recorded_after_the_delete_is_written_into_the_pending_row() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    // A running CLI keeps the removal waiting.
    let tab = engine.sessions[0].slot_tab_id().to_owned();
    let client = PtyClient::spawn_with_env(
        "sh",
        &[
            "-c".to_string(),
            "trap '' TERM HUP; echo ready; sleep 30".to_string(),
        ],
        &worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    engine.providers.insert(TabId::new(tab.as_str()), client);
    let _ = engine.begin_delete_session("s-agent", true, None);
    engine.finish_delete_session_memory("s-agent");

    // A closed terminal's job, recorded only now.
    let (session, job) = leaderless_job_in(&worktree, &tmp.path().join("job.pid"));
    engine
        .process_registry
        .register(UNOWNED_PTYS, session, &worktree);
    let found = crate::process_sessions::survivors_at_leader_exit(session);
    engine.process_registry.record_survivors(session, &found);

    let rows = engine
        .session_store
        .load_pending_worktree_removals()
        .unwrap();
    kill(job);
    for entry in std::mem::take(&mut engine.terminating_ptys) {
        entry.client.force_terminate();
    }
    assert_eq!(rows.len(), 1);
    assert!(rows[0].process_sessions.contains(&session), "{:?}", rows[0]);
    assert!(
        found
            .iter()
            .all(|identity| rows[0].process_snapshot.contains(identity)),
        "{:?}",
        rows[0]
    );
}

/// A manager request for a path that is not one of the project's managed
/// worktrees is refused before anything is claimed: the folder is not left
/// reading "being removed".
#[test]
fn a_manager_refusal_leaves_nothing_claimed() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    for path in [repo.clone(), tmp.path().to_path_buf(), PathBuf::from("/")] {
        let admission = engine
            .admit_manager_removal("p1", &path, false)
            .expect("known project");
        assert!(
            matches!(
                admission,
                RemovalAdmission::Refused(RemovalOutcome::NotManaged)
            ),
            "{}",
            path.display()
        );
        assert!(
            !engine.worktree_ops().is_being_removed(&path),
            "{}",
            path.display()
        );
    }
}

/// Stage, unstage and discard hold their worktree like every other operation:
/// refused, with a sentence, once its removal has begun.
#[test]
fn stage_unstage_and_discard_are_refused_in_a_worktree_being_removed() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    std::fs::write(worktree.join("new.txt"), "x").unwrap();
    let _claim = engine.worktree_ops().announce_removal(&worktree);
    let wt = worktree.clone();
    for command in [
        crate::engine::Command::StageFile {
            worktree_path: wt.clone(),
            path: "new.txt".to_string(),
        },
        crate::engine::Command::UnstageFile {
            worktree_path: wt.clone(),
            path: "new.txt".to_string(),
        },
        crate::engine::Command::DiscardFile {
            worktree_path: wt.clone(),
            path: "new.txt".to_string(),
            is_untracked: true,
            confirmed: crate::git::ConfirmedEntry::Folder { files: 1 },
        },
    ] {
        let refused = engine.apply(command).map(|_| ()).expect_err("refused");
        assert!(
            refused.to_string().contains("dux is removing the worktree"),
            "{refused}"
        );
    }
    assert!(worktree.join("new.txt").exists(), "nothing was discarded");
}

/// Removals of `/a` and `/a/c` both run to the end without git ever running
/// for both at once (the ordering itself is pinned in `worktree_ops`): the
/// later waits for the earlier, and the inner worktree goes either way.
#[test]
fn removals_of_a_folder_and_one_inside_it_both_finish() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let base = tmp.path().join("worktrees").join("p1-name");
    std::fs::create_dir_all(&base).unwrap();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "a",
            base.join("a").to_str().unwrap(),
        ],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "a-c",
            base.join("a").join("c").to_str().unwrap(),
        ],
    );
    for (id, branch, path) in [
        ("s-inner", "a-c", base.join("a").join("c")),
        ("s-outer", "a", base.join("a")),
    ] {
        let mut session = sample_session(id, "p1", branch);
        if let Some(managed) = session.workspace.as_managed_mut() {
            managed.worktree_path = path.to_string_lossy().into_owned();
        }
        engine.sessions.push(session);
    }
    // The inner one is deleted first (its record goes), then the outer one.
    for id in ["s-inner", "s-outer"] {
        assert!(matches!(
            engine.begin_delete_session(id, true, Some(true)),
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        engine.finish_delete_session_memory(id);
    }
    let mut results = Vec::new();
    while results.len() < 2 {
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("each removal reports");
        if let crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } = event {
            results.push(result);
        }
    }
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert!(!base.join("a").exists());
}
