//! Fifth adversarial review of the worktree-removal races branch.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::Engine;
use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::ids::TabId;
use crate::pty::PtyClient;

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

fn alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // A zombie has exited.
    !stat
        .rsplit(')')
        .next()
        .is_some_and(|rest| rest.trim_start().starts_with('Z'))
}

fn kill(pid: i32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
}

fn wait_for_pid(pidfile: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(pidfile)
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the job never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An agent's tab, launched the way `process_agent_launch_ready` registers it.
fn launch_tab(engine: &mut Engine, session_id: &str, worktree: &Path, script: &str) {
    let tab = engine
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .unwrap()
        .slot_tab_id()
        .to_owned();
    let client = PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), script.to_string()],
        worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    if let Some(process) = client.process_session() {
        engine
            .process_registry
            .register(session_id, process, client.spawn_dir());
        client.set_leader_exit_hook(engine.process_registry.leader_exit_hook(process));
    }
    engine.providers.insert(TabId::new(tab.as_str()), client);
}

/// "dux must never remove a folder that something it started is still using."
/// An agent's shell started a background server (a job-controlled job, so the
/// PTY's group signals at quit do not reach it) in its worktree, and dux was
/// quit cleanly. The server keeps running. At the next start the user deletes
/// the agent with its worktree: nothing dux knows of the earlier run's
/// sessions survived the restart, so nothing ends the server and git removes
/// the worktree out from under it.
#[test]
fn a_job_an_agent_left_running_before_a_clean_quit_is_not_removed_from_under() {
    let (mut first, tmp) = test_engine();
    first.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    first
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut first, tmp.path(), &repo, "agent");
    let pidfile = tmp.path().join("job.pid");
    launch_tab(
        &mut first,
        "s-agent",
        &worktree,
        &format!(
            "set -m; nohup sleep 300 >/dev/null 2>&1 & echo $! > '{}'; sleep 60",
            pidfile.display()
        ),
    );
    let job = wait_for_pid(&pidfile);
    let session = first.sessions[0].clone();

    // A clean quit: the PTYs are stopped the ordinary way.
    first.shutdown_ptys(Duration::from_millis(500));
    drop(first);
    std::thread::sleep(Duration::from_millis(200));
    assert!(alive(job), "the server survives the quit");

    // The next start: the agent is back (dormant) and the user deletes it with
    // its worktree.
    // (The next start of dux on the SAME config directory: what the first
    // run saved is all it has.)
    let mut next = crate::engine::test_support::test_engine_at(tmp.path());
    next.config.shutdown_timeout_seconds = 1;
    next.projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    next.sessions.push(session);
    let outcome =
        crate::engine::test_support::delete_through_pipeline(&mut next, "s-agent", true, None);

    let running = alive(job);
    let removed = !worktree.exists();
    kill(job);
    assert!(
        !(running && removed),
        "the worktree at {} was removed while pid {job}, a server the agent started there in \
         the run before, was still running in it (delete outcome ok: {})",
        worktree.display(),
        outcome.is_ok()
    );
}

/// "dux never creates, moves or removes a standalone agent's folder", and
/// never removes a folder something it started is still using. A standalone
/// agent lives, running, in an untracked folder inside a managed agent's
/// worktree. The changes pane lists that folder as untracked, and its "delete
/// the folder" (Command::DiscardFile with a folder confirmation) removes the
/// standalone agent's files out from under its running process: the only thing
/// it asks of the path registry is a hold on the worktree, and nothing asks
/// what occupies the folder it deletes.
#[test]
fn the_changes_pane_does_not_delete_a_running_standalone_agents_folder() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let scratch = worktree.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("notes.md"), "the standalone agent's work\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            scratch.to_str().unwrap(),
        ));
    // The standalone agent is running in its folder.
    let tab = engine.sessions[1].slot_tab_id().to_owned();
    let client = PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "sleep 60".to_string()],
        &scratch,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    if let Some(process) = client.process_session() {
        engine
            .process_registry
            .register_standalone("s-alone", process, client.spawn_dir());
    }
    engine.providers.insert(TabId::new(tab.as_str()), client);

    let outcome = engine.apply(crate::engine::Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "scratch".to_string(),
        is_untracked: true,
        confirmed: crate::git::ConfirmedEntry::Folder { files: 1 },
    });
    let survived = scratch.join("notes.md").exists();
    if let Some(client) = engine
        .providers
        .remove(TabId::new(tab.as_str()).as_ref_id())
    {
        client.force_terminate();
    }
    assert!(
        survived,
        "the changes pane deleted the files of standalone agent s-alone, running in {} \
         (outcome ok: {})",
        scratch.display(),
        outcome.is_ok()
    );
}

// Tests added with the fixes for the fifth review.

/// The registry is persisted through every change, not only when a removal is
/// parked: after a clean quit and a restart, the worktree manager's removal of
/// a folder an earlier run's agent left a job running in ends that job before
/// git runs (or refuses), exactly as the agent delete does.
#[test]
fn a_job_left_before_a_clean_quit_is_ended_before_the_manager_removes_its_folder() {
    let (mut first, tmp) = test_engine();
    first.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    first
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut first, tmp.path(), &repo, "agent");
    let pidfile = tmp.path().join("job.pid");
    launch_tab(
        &mut first,
        "s-agent",
        &worktree,
        &format!(
            "set -m; nohup sleep 300 >/dev/null 2>&1 & echo $! > '{}'; sleep 60",
            pidfile.display()
        ),
    );
    let job = wait_for_pid(&pidfile);
    first.shutdown_ptys(Duration::from_millis(500));
    drop(first);
    std::thread::sleep(Duration::from_millis(200));
    assert!(alive(job), "the server survives the quit");

    // The next start: the agent is gone (deleted keeping its worktree in the
    // earlier run is the same picture), so the manager lists the folder.
    let mut next = crate::engine::test_support::test_engine_at(tmp.path());
    next.config.shutdown_timeout_seconds = 1;
    next.projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let admission = next.admit_manager_removal("p1", &worktree, false);
    let outcome = match admission {
        Some(crate::worktree_manager::RemovalAdmission::Admitted(removal)) => Some(removal.run()),
        _ => None,
    };
    let running = alive(job);
    let removed = !worktree.exists();
    kill(job);
    assert!(
        !(running && removed),
        "the manager removed {} while pid {job}, started there in the run before, still ran \
         in it (outcome: {outcome:?})",
        worktree.display()
    );
}

/// The manager's re-check under its claim reads the agents as they are then,
/// not as they were at admission: an agent attached to the folder in between
/// keeps it.
#[test]
fn the_managers_recheck_sees_an_agent_attached_after_admission() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "late");
    // Admitted while no agent holds the folder.
    let session = engine.sessions.pop().unwrap();
    let Some(crate::worktree_manager::RemovalAdmission::Admitted(removal)) =
        engine.admit_manager_removal("p1", &worktree, false)
    else {
        panic!("nothing occupies the folder at admission");
    };
    // An agent is attached to it before the removal runs: its row is written.
    engine.session_store.create_session(&session).unwrap();
    engine.sessions.push(session);

    let outcome = removal.run();
    assert!(
        matches!(
            outcome,
            Ok(crate::worktree_manager::RemovalOutcome::Attached)
        ),
        "{outcome:?}"
    );
    assert!(worktree.exists());
}

/// A landing create keeps its hold on the folder until its row is written:
/// the guard, not the bookkeeping, releases it.
#[test]
fn a_landing_create_holds_its_folder_until_the_guard_drops() {
    let (mut engine, tmp) = test_engine();
    let folder = tmp.path().join("wt");
    std::fs::create_dir_all(&folder).unwrap();
    engine
        .worktree_ops()
        .hold_as(
            crate::worktree_ops::HoldOwner::CreateOp("op-1".to_string()),
            &folder,
            crate::worktree_ops::WorktreeOpKind::CreateAgent,
        )
        .unwrap();
    let guard = engine.note_create_finished_holding("op-1");
    assert_eq!(
        engine.worktree_ops().holders(&folder),
        vec![crate::worktree_ops::WorktreeOpKind::CreateAgent]
    );
    drop(guard);
    assert!(engine.worktree_ops().holders(&folder).is_empty());
}

/// The registry is written to its own table on every change and read back at
/// the next start, minus the sessions whose recorded processes are all gone.
#[test]
fn the_registry_table_round_trips_and_prunes_dead_sessions() {
    let (engine, tmp) = test_engine();
    let folder = tmp.path().join("wt");
    std::fs::create_dir_all(&folder).unwrap();
    // A session whose leader is long gone, with nothing recorded: pruned.
    let dead = crate::process_sessions::ProcessSession {
        sid: 9_999_990,
        started_at_secs: 1,
        boot: crate::process_sessions::current_boot(),
    };
    engine.process_registry.register("s-dead", dead, &folder);
    // A live one (this test's own process session is not ours to register, so
    // use a short-lived child that leads its own session).
    let mut child = std::process::Command::new("sleep");
    child.arg("30");
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        child.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = child.spawn().unwrap();
    let live = crate::process_sessions::ProcessSession::started_now(child.id());
    engine.process_registry.register("s-live", live, &folder);
    drop(engine);

    let reloaded = crate::engine::test_support::test_engine_at(tmp.path());
    // The prune of what is gone runs off the starting thread.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sessions = reloaded.process_registry.sessions_in(&folder);
    while sessions.contains(&dead) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        sessions = reloaded.process_registry.sessions_in(&folder);
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(sessions.contains(&live), "{sessions:?}");
    assert!(!sessions.contains(&dead), "{sessions:?}");
}
