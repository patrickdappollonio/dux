//! A deleted agent's worktree is removed only once every process dux launched
//! for that agent has gone: each provider tab, each companion terminal, and
//! everything still running in those PTYs' sessions, background jobs included.
//!
//! These drive the real delete path end to end against a real git worktree.
//! Each fixture leaves a writer behind in the worktree that ignores the polite
//! signals and keeps creating files there, which is what a dev server shutting
//! down (Astro regenerating `.astro/`) does to `git worktree remove --force`:
//! git empties the directory, the writer puts something back, and git's final
//! `rmdir` fails with "Directory not empty".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{BeginDeleteSessionOutcome, Engine, RemovedBranches};
use crate::ids::TabId;
use crate::pty::PtyClient;
use crate::test_scratch::ScratchDir;

/// A writer that ignores SIGTERM and SIGHUP, records its pid, and keeps
/// recreating files in its working directory until it is SIGKILLed: a busy
/// loop of shell builtins over a few dozen names, so something reappears
/// behind git however git orders its deletes, without the folder growing.
/// Written to a file so no quoting layer can expand it early.
fn writer_script(pidfile: &Path) -> PathBuf {
    let script = pidfile.with_extension(format!("{}.sh", writer_pids(pidfile).len()));
    let body = format!(
        "trap '' TERM HUP\necho $$ >> '{}'\ni=0\n\
         while :; do i=$(( (i + 1) % 40 )); : > .astro-$i 2>/dev/null; done\n",
        pidfile.display()
    );
    std::fs::write(&script, body).expect("write writer script");
    script
}

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::fixture_git()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    engine: Engine,
    _tmp: ScratchDir,
    repo: PathBuf,
    worktree: PathBuf,
    pidfile: PathBuf,
}

impl Drop for Fixture {
    /// SIGKILL every writer that is still around, so a failing run does not leak
    /// a process looping in a temp directory.
    fn drop(&mut self) {
        for pid in writer_pids(&self.pidfile) {
            if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
        }
    }
}

fn writer_pids(pidfile: &Path) -> Vec<i32> {
    std::fs::read_to_string(pidfile)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

fn process_alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

/// A repository with a dux-created worktree on branch `feat`, an agent `s1`
/// whose managed workspace is that worktree, and a short close grace so the
/// SIGKILL step is reached quickly.
fn fixture() -> Fixture {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("repo dir");
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    // Enough files that the delete takes long enough for the writer to land
    // something behind it.
    for i in 0..400 {
        std::fs::write(repo.join("src").join(format!("f{i}.txt")), "x").expect("seed file");
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    let worktree = tmp.path().join("wt-feat");
    git(
        &repo,
        &["worktree", "add", "-b", "feat", worktree.to_str().unwrap()],
    );

    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let mut session = sample_session("s1", "p1", "feat");
    session
        .workspace
        .as_managed_mut()
        .expect("managed test session")
        .worktree_path = worktree.to_string_lossy().to_string();
    engine.sessions.push(session);
    let pidfile = tmp.path().join("writers.pid");
    Fixture {
        engine,
        _tmp: tmp,
        repo,
        worktree,
        pidfile,
    }
}

/// Spawn a PTY in the worktree running `script` under `sh -c`.
fn spawn_in(worktree: &Path, script: String) -> PtyClient {
    PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), script],
        worktree,
        24,
        80,
        1000,
        &[],
    )
    .expect("spawn sh")
}

/// Block until `count` writers have recorded their pid, so the delete starts
/// while they are definitely running.
fn wait_for_writers(pidfile: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while writer_pids(pidfile).len() < count {
        assert!(Instant::now() < deadline, "the writer never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Let each writer get past its first file, so it is in its loop.
    std::thread::sleep(Duration::from_millis(100));
}

/// Delete the agent with its worktree the way both surfaces do, driving the
/// reaper as their tick does, and return what the removal worker reported.
fn delete_and_wait(engine: &mut Engine) -> Result<RemovedBranches, String> {
    let outcome = engine.begin_delete_session("s1", true, Some(true));
    assert!(
        matches!(outcome, BeginDeleteSessionOutcome::AsyncStarted { .. }),
        "a worktree-removing delete is the deferred path"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for removal in engine.reap_terminating_ptys().removals {
            let _ = engine.dispatch_deferred_worktree_removal(removal);
        }
        match engine.worker_rx.try_recv() {
            Ok(crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. }) => {
                return result;
            }
            Ok(_) => continue,
            Err(_) => {
                assert!(Instant::now() < deadline, "the removal never reported");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn assert_removed_cleanly(fx: &Fixture, result: Result<RemovedBranches, String>) {
    assert!(
        result.is_ok(),
        "the removal must wait for every process in the worktree: {result:?}"
    );
    assert!(!fx.worktree.exists(), "the worktree directory is gone");
    for pid in writer_pids(&fx.pidfile) {
        assert!(!process_alive(pid), "writer {pid} was stopped first");
    }
    let out = crate::test_git::fixture_git()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&fx.repo)
        .output()
        .expect("git worktree list");
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(
        !listing.contains("wt-feat"),
        "git no longer lists the worktree: {listing}"
    );
}

/// The reported bug: the provider exits on SIGTERM, but a dev server it
/// started in the worktree is still running and writing there. Like a CLI's
/// background command, the server has its own process group and its own pipes,
/// so neither the PTY's group signals nor the PTY closing reach it. The
/// leader's exit is not the agent's exit.
#[test]
fn removal_waits_for_a_child_the_provider_left_running() {
    let mut fx = fixture();
    let script = format!(
        "set -m; sh '{}' </dev/null >/dev/null 2>&1 & echo ready; exec cat",
        writer_script(&fx.pidfile).display()
    );
    let client = spawn_in(&fx.worktree, script);
    fx.engine.providers.insert(TabId::new("s1-slot"), client);
    wait_for_writers(&fx.pidfile, 1);

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}

/// A companion terminal of the agent is closed by the delete, and the removal
/// waits for it (and what runs in it) as it waits for the agent's tabs.
#[test]
fn removal_waits_for_a_companion_terminal() {
    let mut fx = fixture();
    fx.engine.config.terminal.command = "sh".to_string();
    fx.engine.config.terminal.args = vec![
        "-c".to_string(),
        format!("exec sh '{}'", writer_script(&fx.pidfile).display()),
    ];
    // The terminal's shell IS the writer: it ignores both polite signals, the
    // way an interactive shell ignores SIGTERM.
    let (_tid, _label) = fx
        .engine
        .create_companion_terminal("s1", 24, 80)
        .expect("create companion terminal");
    wait_for_writers(&fx.pidfile, 1);

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}

/// A job-controlled background job sits in a process group of its own, which
/// none of the PTY's group signals reach. It is still in the PTY's session, and
/// the removal waits for it.
#[test]
fn removal_waits_for_a_background_job_in_its_own_process_group() {
    let mut fx = fixture();
    let script = format!(
        "set -m; sh '{}' & echo ready; exec cat",
        writer_script(&fx.pidfile).display()
    );
    let client = spawn_in(&fx.worktree, script);
    let leader = client.child_process_id().expect("leader pid") as i32;
    fx.engine.providers.insert(TabId::new("s1-slot"), client);
    wait_for_writers(&fx.pidfile, 1);
    let writer = writer_pids(&fx.pidfile)[0];
    let writer_group = rustix::process::getpgid(rustix::process::Pid::from_raw(writer))
        .expect("writer group")
        .as_raw_nonzero()
        .get();
    assert_ne!(
        writer_group, leader,
        "the fixture must put the job in a group of its own"
    );

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}

/// A child that called `setsid` has left the PTY's session, so only its
/// parentage ties it to the agent. It is found while its parent is alive,
/// before anything is asked to exit, and the removal waits for it too.
#[cfg(target_os = "linux")]
#[test]
fn removal_waits_for_a_child_that_left_the_session() {
    let mut fx = fixture();
    let script = format!(
        "setsid sh '{}' & echo ready; exec cat",
        writer_script(&fx.pidfile).display()
    );
    let client = spawn_in(&fx.worktree, script);
    fx.engine.providers.insert(TabId::new("s1-slot"), client);
    wait_for_writers(&fx.pidfile, 1);

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}

/// Several tabs and a terminal at once: nothing is removed until the last of
/// them, and everything they started, has gone.
#[test]
fn removal_waits_for_every_tab_and_terminal_of_the_agent() {
    let mut fx = fixture();
    for (n, tab) in ["s1-slot", "s1-tab-2"].into_iter().enumerate() {
        let script = format!(
            "sh '{}' & echo ready; exec cat",
            writer_script(&fx.pidfile).display()
        );
        let client = spawn_in(&fx.worktree, script);
        fx.engine.providers.insert(TabId::new(tab), client);
        wait_for_writers(&fx.pidfile, n + 1);
    }
    fx.engine.agent_tabs.insert(
        TabId::new("s1-tab-2"),
        crate::engine::test_support::sample_tab("s1-tab-2", "s1", "codex", 1),
    );
    fx.engine.config.terminal.command = "sh".to_string();
    fx.engine.config.terminal.args = vec![
        "-c".to_string(),
        format!("exec sh '{}'", writer_script(&fx.pidfile).display()),
    ];
    fx.engine
        .create_companion_terminal("s1", 24, 80)
        .expect("create companion terminal");
    wait_for_writers(&fx.pidfile, 3);

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}

/// Something dux did not start and cannot see (a daemon in a session of its
/// own) keeps writing into the worktree. Nothing dux knows of is left to stop,
/// git removes what it can, and the final says what is left and how to finish
/// rather than reporting a bare git error. The registration is gone, the
/// branch the user asked to delete is deleted, and dux deletes nothing itself.
#[test]
fn a_writer_dux_cannot_see_leaves_a_folder_and_the_final_says_how_to_finish() {
    let mut fx = fixture();
    let script = writer_script(&fx.pidfile);
    let mut daemon = std::process::Command::new("sh");
    daemon.arg(&script).current_dir(&fx.worktree);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        daemon.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut daemon = daemon.spawn().expect("spawn the unseen writer");
    wait_for_writers(&fx.pidfile, 1);

    let result = delete_and_wait(&mut fx.engine);
    let _ = daemon.kill();
    let _ = daemon.wait();
    let message = result.expect_err("git cannot finish under a writer nobody stopped");

    let path = crate::home_path::shorten_home(&fx.worktree);
    for needle in [
        path.as_str(),
        "kept writing into it",
        "dev server",
        "Still in it: .astro-",
        "then delete the folder",
        "Branch feat was deleted as asked.",
    ] {
        assert!(message.contains(needle), "missing {needle:?} in: {message}");
    }
    assert!(fx.worktree.exists(), "dux never deletes the folder itself");
    let listing = crate::test_git::fixture_git()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&fx.repo)
        .output()
        .expect("git worktree list");
    assert!(
        !String::from_utf8_lossy(&listing.stdout).contains("wt-feat"),
        "no dangling registration is left behind"
    );

    // Both surfaces wrap the worker's message in the same shared sentence.
    let facts = crate::wire::DeleteReportFacts {
        label: "feat".to_string(),
        ..Default::default()
    };
    let surfaced = crate::wire::delete_session_failure_message(Some(&facts), &message);
    assert!(
        surfaced
            .to_string()
            .starts_with("Worktree delete failed for ")
            && surfaced.to_string().contains("kept writing into it"),
        "{surfaced}"
    );
}

/// Relaunching an agent whose previous CLI of the same provider is still
/// shutting down (a detach a moment ago, a restart) waits for it, says so, and
/// starts only once it is gone, so two CLIs never share one worktree and one
/// conversation. The wait is bounded by the close grace, after which the
/// reaper force-kills the old one.
#[test]
fn a_relaunch_waits_for_the_previous_run_of_its_provider() {
    let mut fx = fixture();
    let old = spawn_in(
        &fx.worktree,
        "trap '' TERM HUP; echo ready; exec sleep 30".to_string(),
    );
    fx.engine.providers.insert(TabId::new("s1-slot"), old);
    let ready_by = Instant::now() + Duration::from_secs(5);
    while !fx.engine.providers[crate::ids::TabIdRef::new("s1-slot")].has_output() {
        assert!(Instant::now() < ready_by, "the old CLI never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(
        fx.engine.begin_detach_session("s1"),
        crate::engine::DetachSessionOutcome::Started { .. }
    ));
    assert_eq!(
        fx.engine.terminating_ptys.len(),
        1,
        "the old CLI is terminating"
    );

    let session = fx.engine.sessions[0].clone();
    let request = fx.engine.build_agent_launch_request(
        session,
        true,
        (24, 80),
        crate::worker::AgentLaunchKind::Reconnect {
            status_message: "Reconnecting".to_string().into(),
        },
    );
    fx.engine
        .apply(crate::engine::Command::DispatchAgentLaunch {
            request: Box::new(request),
        })
        .expect("dispatch the relaunch");

    let mut waited_out_loud = false;
    let mut old_gone_at = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    let launched_at = loop {
        let _ = fx.engine.reap_terminating_ptys();
        if old_gone_at.is_none() && fx.engine.terminating_ptys.is_empty() {
            old_gone_at = Some(Instant::now());
        }
        match fx.engine.worker_rx.try_recv() {
            Ok(crate::worker::WorkerEvent::PollerStatus(status)) => {
                waited_out_loud |= status
                    .message
                    .to_string()
                    .contains("Waiting for the previous");
            }
            Ok(crate::worker::WorkerEvent::AgentLaunchReady(_)) => break Instant::now(),
            Ok(crate::worker::WorkerEvent::AgentLaunchFailed(data)) => {
                panic!("the relaunch failed: {}", data.message)
            }
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "the relaunch never happened");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let old_gone_at = old_gone_at.expect("the old CLI was reaped before the new one came up");
    assert!(
        launched_at >= old_gone_at,
        "the new CLI came up only after the old one was gone"
    );
    assert!(waited_out_loud, "the wait was announced");
}

/// A startup command still running when the agent is deleted with its
/// worktree runs in a session dux registered for the agent, so the removal
/// ends it, and what it started, before git touches a file. The run then says
/// the agent was deleted rather than writing a log for an agent that is gone.
#[test]
fn removal_ends_a_startup_command_still_running_in_the_worktree() {
    let mut fx = fixture();
    let script = writer_script(&fx.pidfile);
    let session = fx.engine.sessions[0].clone();
    let run = crate::startup::StartupCommandRun {
        project: fx.engine.projects[0].clone(),
        managed: session
            .workspace
            .as_managed()
            .expect("managed test session")
            .clone(),
        session,
        command: format!("sh '{}'", script.display()),
        terminal: crate::config::StartupCommandTerminalConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string()],
        },
        env: Vec::new(),
    };
    let paths = fx.engine.paths.clone();
    let registry = fx.engine.process_registry.clone();
    let handle =
        std::thread::spawn(move || crate::startup::run_startup_command(&paths, run, &registry));
    wait_for_writers(&fx.pidfile, 1);

    let result = delete_and_wait(&mut fx.engine);
    // Whatever happened, nothing of the test's may keep running.
    for pid in writer_pids(&fx.pidfile) {
        if let Some(pid) = rustix::process::Pid::from_raw(pid) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
    let startup = handle.join().expect("startup thread");
    assert_removed_cleanly(&fx, result);
    let err = startup.status.expect_err("the run reports the deletion");
    assert!(err.contains("was deleted"), "{err}");
}

fn pending_rows(engine: &Engine) -> Vec<crate::storage::PendingWorktreeRemoval> {
    engine
        .session_store
        .load_pending_worktree_removals()
        .expect("read pending removals")
}

/// An agent CLI that ignores the polite signals, so a delete has to wait out
/// the whole grace for it.
fn stubborn_tab(fx: &mut Fixture) {
    let client = spawn_in(
        &fx.worktree,
        "trap '' TERM HUP; echo ready; exec sleep 30".to_string(),
    );
    fx.engine.providers.insert(TabId::new("s1-slot"), client);
    let ready_by = Instant::now() + Duration::from_secs(5);
    while !fx.engine.providers[crate::ids::TabIdRef::new("s1-slot")].has_output() {
        assert!(Instant::now() < ready_by, "the CLI never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The removal is written down when the delete is accepted and cleared once
/// it has run.
#[test]
fn a_removal_is_recorded_until_it_has_run() {
    let mut fx = fixture();
    stubborn_tab(&mut fx);
    let outcome = fx.engine.begin_delete_session("s1", true, Some(true));
    assert!(matches!(
        outcome,
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    let rows = pending_rows(&fx.engine);
    assert_eq!(rows.len(), 1, "recorded the moment the delete is accepted");
    assert_eq!(rows[0].managed.worktree_path, fx.worktree.to_string_lossy());
    assert_eq!(rows[0].delete_branch, Some(true));
    assert!(
        !rows[0].process_sessions.is_empty(),
        "with the sessions to end"
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    let result = loop {
        for removal in fx.engine.reap_terminating_ptys().removals {
            let _ = fx.engine.dispatch_deferred_worktree_removal(removal);
        }
        if let Ok(crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. }) =
            fx.engine.worker_rx.try_recv()
        {
            break result;
        }
        assert!(Instant::now() < deadline, "the removal never reported");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_removed_cleanly(&fx, result);
    assert!(
        pending_rows(&fx.engine).is_empty(),
        "cleared once it has run"
    );
}

/// Quitting while the deleted agent's CLI is still in its grace used to lose
/// the removal: the worktree and the branch stayed with no agent to delete
/// them from. A clean quit now finishes it inside the shutdown wait.
#[test]
fn a_clean_quit_finishes_a_removal_still_waiting_on_the_agent() {
    let mut fx = fixture();
    stubborn_tab(&mut fx);
    let _ = fx.engine.begin_delete_session("s1", true, Some(true));

    let _report = fx.engine.shutdown_ptys(Duration::from_secs(1));

    assert!(!fx.worktree.exists(), "the quit finished the removal");
    assert!(pending_rows(&fx.engine).is_empty());
    let branches = crate::test_git::fixture_git()
        .args(["branch", "--list", "feat"])
        .current_dir(&fx.repo)
        .output()
        .expect("git branch");
    assert!(
        String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
        "and the branch the user asked to delete"
    );
}

/// A quit that could not finish (forced, or a crash) leaves the removal
/// recorded, and the next start finishes it in the background with a status
/// that says what it is doing and how it went.
#[test]
fn the_next_start_finishes_a_removal_the_last_run_left_behind() {
    let mut fx = fixture();
    stubborn_tab(&mut fx);
    let _ = fx.engine.begin_delete_session("s1", true, Some(true));
    // What both surfaces do next: the agent leaves the list at once.
    fx.engine
        .finish_delete_session("s1")
        .expect("vanish the agent")
        .expect("it was there");
    // The crash: everything in memory is gone, the database row is not.
    fx.engine.pending_group_removals.clear();
    fx.engine.removal_coordination.claims.clear();
    for entry in std::mem::take(&mut fx.engine.terminating_ptys) {
        entry.client.force_terminate();
    }
    assert_eq!(pending_rows(&fx.engine).len(), 1);
    assert!(fx.worktree.exists());

    fx.engine.resume_pending_worktree_removals();

    let mut busy = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    let final_status = loop {
        match fx.engine.worker_rx.try_recv() {
            Ok(crate::worker::WorkerEvent::PollerStatus(status)) => busy = Some(status),
            Ok(crate::worker::WorkerEvent::StatusOpCompleted { resolved }) => break resolved,
            Ok(_) => {}
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "the resumed removal never reported"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let busy = busy.expect("a busy status first");
    assert!(
        busy.message.contains("had not finished when it last quit"),
        "{}",
        busy.message
    );
    let reaction = final_status.into_reaction();
    let crate::engine::EventReaction::Status(update) = reaction else {
        panic!("expected a status final");
    };
    assert!(
        update.message.contains("Finished removing the worktree"),
        "{}",
        update.message
    );
    assert!(!fx.worktree.exists(), "the removal was finished");
    assert!(pending_rows(&fx.engine).is_empty());
}

/// A recorded removal whose directory another agent now works in is refused
/// out loud and forgotten, never run.
#[test]
fn a_left_over_removal_never_takes_a_directory_another_agent_now_uses() {
    let mut fx = fixture();
    let session = fx.engine.sessions[0].clone();
    fx.engine
        .session_store
        .insert_pending_worktree_removal(&crate::storage::PendingWorktreeRemoval {
            session_id: "gone-agent".to_string(),
            label: "old".to_string(),
            project_path: fx.repo.to_string_lossy().into_owned(),
            managed: session
                .workspace
                .as_managed()
                .expect("managed test session")
                .clone(),
            delete_branch: Some(true),
            process_sessions: Vec::new(),
            process_snapshot: Vec::new(),
            process_registry: Default::default(),
        })
        .expect("record");

    fx.engine.resume_pending_worktree_removals();

    let warning = match fx.engine.worker_rx.try_recv() {
        Ok(crate::worker::WorkerEvent::PollerStatus(status)) => status,
        _ => panic!("expected the refusal"),
    };
    assert_eq!(warning.tone, crate::statusline::StatusTone::Warning);
    assert!(
        warning.message.contains("Kept the worktree"),
        "{}",
        warning.message
    );
    assert!(
        fx.worktree.exists(),
        "the other agent's directory is untouched"
    );
    assert!(
        pending_rows(&fx.engine).is_empty(),
        "and the request is forgotten"
    );
}

/// A force-stop that overtakes a delete still waiting on the agent's CLI kills
/// that CLI at once, and the worktree removal waiting on it is still
/// dispatched, exactly once. A terminating PTY leaves only through the reaper,
/// so no path can drop one and strand the removal behind it.
#[test]
fn a_force_stop_overtaking_a_delete_still_dispatches_its_removal_exactly_once() {
    let mut fx = fixture();
    stubborn_tab(&mut fx);
    let _ = fx.engine.begin_delete_session("s1", true, Some(true));
    assert_eq!(fx.engine.pending_group_removals.len(), 1);

    let outcome = fx.engine.force_detach_session("s1");
    assert!(
        matches!(outcome, crate::engine::ForceDetachOutcome::Stopped { .. }),
        "{outcome:?}"
    );

    let mut dispatched = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fx.engine.terminating_ptys.is_empty() || !fx.engine.pending_group_removals.is_empty() {
        dispatched.extend(fx.engine.reap_terminating_ptys().removals);
        assert!(
            Instant::now() < deadline,
            "the overtaken PTY was never reaped"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Long after the grace the overtaken entry would have had: nothing more.
    std::thread::sleep(Duration::from_millis(1200));
    dispatched.extend(fx.engine.reap_terminating_ptys().removals);
    assert_eq!(dispatched.len(), 1, "dispatched exactly once");
    assert_eq!(dispatched[0].session_id, "s1");
}

/// A removal finished at the next start takes the same claim on the folder a
/// delete takes: nothing new starts there while it runs, and work already
/// running there is waited for before git removes anything.
#[test]
fn a_resumed_removal_claims_the_folder_and_waits_for_work_in_it() {
    let mut fx = fixture();
    let session = fx.engine.sessions.remove(0);
    fx.engine
        .session_store
        .insert_pending_worktree_removal(&crate::storage::PendingWorktreeRemoval {
            session_id: session.id.clone(),
            label: "feat".to_string(),
            project_path: fx.repo.to_string_lossy().into_owned(),
            managed: session
                .workspace
                .as_managed()
                .expect("managed test session")
                .clone(),
            delete_branch: Some(true),
            process_sessions: Vec::new(),
            process_snapshot: Vec::new(),
            process_registry: Default::default(),
        })
        .expect("record");
    // A pull already running in the worktree when dux starts.
    let pull = fx
        .engine
        .worktree_ops()
        .hold(&fx.worktree, crate::worktree_ops::WorktreeOpKind::Pull)
        .expect("nothing claims the folder yet");

    fx.engine.resume_pending_worktree_removals();

    assert!(
        fx.engine
            .worktree_ops()
            .hold(
                &fx.worktree,
                crate::worktree_ops::WorktreeOpKind::EditorWrite
            )
            .is_err(),
        "nothing new may start in a folder being removed"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(fx.worktree.exists(), "the removal waits for the pull");
    drop(pull);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(crate::worker::WorkerEvent::StatusOpCompleted { .. }) =
            fx.engine.worker_rx.try_recv()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the resumed removal never reported"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!fx.worktree.exists(), "removed once the pull let go");
}

/// The removal's last look under its claim: a session started in the folder
/// that the removal did not already end, and that is still running, keeps the
/// folder, with what is running named.
#[test]
fn the_last_look_keeps_a_folder_something_new_runs_in() {
    let fx = fixture();
    let mut sleeper = std::process::Command::new("sh");
    sleeper.args(["-c", "sleep 30"]).current_dir(&fx.worktree);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        sleeper.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut sleeper = sleeper.spawn().expect("spawn");
    let registry = crate::process_sessions::AgentProcessRegistry::default();
    let late = crate::process_sessions::ProcessSession::started_now(sleeper.id());
    registry.register("someone-else", late, &fx.worktree);
    let crate::worktree_ops::RemovalClaim::Lead(lease) =
        fx.engine.worktree_ops().announce_removal(&fx.worktree)
    else {
        panic!("the only removal leads");
    };

    let known = crate::engine::RemovalProcesses::none();
    let occupant = crate::engine::occupant_after_wait(&lease, &registry, &registry, &known);
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    let occupant = occupant.expect("the folder is occupied");
    assert!(
        occupant.contains("sleep") || occupant.contains("sh"),
        "{occupant}"
    );

    // The same session, once the removal has ended it itself, is not news.
    let ended = crate::engine::RemovalProcesses {
        sessions: vec![late],
        ..crate::engine::RemovalProcesses::none()
    };
    assert!(crate::engine::occupant_after_wait(&lease, &registry, &registry, &ended).is_none());
}

/// A terminal's shell started a background job (in a process group of its
/// own, so closing the terminal does not take it) and exited. The job keeps the
/// shell's session number, but with the shell gone that number proves nothing
/// on its own; what makes the job dux's is that dux saw it still running the
/// moment it saw the shell exit, and recorded it. A later removal of the
/// worktree ends it through that record.
#[test]
fn a_job_left_by_a_shell_that_exited_is_ended_through_what_was_recorded_at_its_exit() {
    let mut fx = fixture();
    let pidfile = fx.pidfile.clone();
    fx.engine.config.terminal.command = "sh".to_string();
    fx.engine.config.terminal.args = vec![
        "-c".to_string(),
        format!(
            "set -m; sleep 60 </dev/null >/dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ),
    ];
    let (tid, _) = fx
        .engine
        .create_companion_terminal("s1", 24, 80)
        .expect("create terminal");
    wait_for_writers(&pidfile, 1);
    let job = writer_pids(&pidfile)[0];
    let session = fx.engine.companion_terminals[&tid]
        .client
        .process_session()
        .expect("session");

    // The shell exits on its own; the prune drops its client, and dux looks.
    let deadline = Instant::now() + Duration::from_secs(10);
    while fx.engine.companion_terminals.contains_key(&tid) {
        let _ = fx.engine.prune_exited_ptys();
        assert!(Instant::now() < deadline, "the shell never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
    while fx
        .engine
        .process_registry
        .survivors_of(&[session])
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "nothing was recorded at the shell's exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(process_alive(job), "the job outlived its shell");

    let result = delete_and_wait(&mut fx.engine);
    assert_removed_cleanly(&fx, result);
}
