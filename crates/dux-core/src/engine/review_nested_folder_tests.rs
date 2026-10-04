//! Adversarial review: a folder dux is using INSIDE a worktree is a folder the
//! worktree's removal deletes. Every check the branch added compares folders
//! for equality, so something running in a subfolder of the worktree is never
//! seen, and the removal deletes the folder out from under it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{
    sample_project, sample_session, sample_standalone_session, test_engine,
};
use crate::engine::{BeginDeleteSessionOutcome, Command, Engine};
use crate::ids::TabId;
use crate::pty::PtyClient;
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

/// A standalone agent `s-alone` running (a live PTY, registered as dux
/// registers a launched provider) in `folder`.
fn standalone_running_in(engine: &mut Engine, folder: &Path) {
    std::fs::create_dir_all(folder).unwrap();
    let session = sample_standalone_session("s-alone", folder.to_str().unwrap());
    let tab = session.slot_tab_id().to_owned();
    engine.sessions.push(session);
    let client = PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "sleep 60".to_string()],
        folder,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    if let Some(process) = client.process_session() {
        engine
            .process_registry
            .register("s-alone", process, client.spawn_dir());
    }
    engine.providers.insert(TabId::new(tab.as_str()), client);
}

fn pump_until_removed(engine: &mut Engine) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        for removal in engine.reap_terminating_ptys().removals {
            let _ = engine.dispatch_deferred_worktree_removal(removal);
        }
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        let done = matches!(
            event,
            crate::worker::WorkerEvent::WorktreeRemoveCompleted { .. }
        );
        engine.process_worker_event(event);
        if done {
            return;
        }
    }
    panic!("the removal never reported");
}

/// A standalone agent the user pointed at a subfolder of an agent's worktree
/// is running there. Deleting the managed agent "with its worktree" must not
/// delete the folder that standalone agent runs in: dux never removes a
/// standalone agent's folder, and never removes a folder something it started
/// is still using.
#[test]
fn deleting_an_agent_keeps_a_worktree_a_standalone_agent_runs_inside() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let inside = worktree.join("frontend");
    standalone_running_in(&mut engine, &inside);
    std::fs::write(inside.join("notes.txt"), "the standalone agent's work").unwrap();

    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    pump_until_removed(&mut engine);

    let still_running = engine
        .providers
        .get(crate::ids::TabIdRef::new("s-alone-slot"))
        .is_some_and(|client| client.is_live());
    assert!(still_running, "the standalone agent is still running");
    assert!(
        inside.join("notes.txt").exists(),
        "the removal deleted {} while standalone agent s-alone was running in it",
        inside.display()
    );
}

/// The worktree manager decides from live state: a worktree with no agent of
/// its own, but a live standalone agent running in a folder inside it, is in
/// use and must not be admitted for removal.
#[test]
fn the_manager_refuses_a_worktree_a_standalone_agent_runs_inside() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "left");
    // The worktree's own agent was deleted with the worktree kept.
    engine.sessions.retain(|session| session.id != "s-left");
    let inside = worktree.join("frontend");
    standalone_running_in(&mut engine, &inside);

    let admission = engine
        .admit_manager_removal("p1", &worktree, false)
        .expect("known project");
    assert!(
        matches!(admission, RemovalAdmission::Refused(_)),
        "the manager admitted the removal of {} while standalone agent s-alone runs in {}",
        worktree.display(),
        inside.display()
    );
}

/// Nothing new may start in a claimed folder. A standalone agent created in a
/// subfolder of a worktree whose removal has begun is created inside a folder
/// that is about to go, and the removal then deletes it.
#[test]
fn a_standalone_agent_cannot_be_created_inside_a_worktree_being_removed() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let inside = worktree.join("frontend");
    std::fs::create_dir_all(&inside).unwrap();
    // Keep the removal waiting so the create lands inside its window.
    let pull = crate::engine::InFlightKey::Pull(worktree.to_string_lossy().into_owned());
    engine.mark_in_flight(pull.clone());
    engine
        .hold_path_for_in_flight(&pull, &worktree, crate::worktree_ops::WorktreeOpKind::Pull)
        .unwrap();
    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    assert!(engine.worktree_ops().is_being_removed(&worktree));

    let _ = engine
        .apply(Command::DispatchCreateAgentRequest {
            request: Box::new(crate::worker::CreateAgentRequest::Standalone {
                folder: inside.clone(),
                title: "mine".into(),
                provider: crate::model::ProviderKind::new("claude"),
            }),
            busy_message: "Creating\u{2026}".to_string().into(),
            term_size: (80, 24),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let done = matches!(
            event,
            crate::worker::WorkerEvent::AgentLaunchReady(_)
                | crate::worker::WorkerEvent::AgentLaunchFailed(_)
                | crate::worker::WorkerEvent::CreateAgentFailed { .. }
        );
        engine.process_worker_event(event);
        if done {
            break;
        }
    }
    let created = engine
        .sessions
        .iter()
        .any(|session| session.workspace.as_managed().is_none());
    assert!(
        !created,
        "a standalone agent was created in {}, inside a worktree whose removal had begun",
        inside.display()
    );
    engine.clear_in_flight(&pull);
}

/// A process session number recorded by an EARLIER run of dux is reused by an
/// unrelated program whose session has no leader any more: the ordinary shape
/// of a double-forked daemon (it calls `setsid`, forks, and the leader exits).
/// The next start finishes the recorded removal and "ends every process in the
/// sessions the agent ran in", and a leaderless session is taken for the
/// original one whatever its members' start times say, so the unrelated
/// program, started an hour after dux recorded that session, is killed.
#[test]
fn a_resumed_removal_does_not_kill_an_unrelated_program_that_reused_a_session_number() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "gone");
    let session = engine.sessions.remove(0);

    // An unrelated program: a session leader that started a background job
    // and exited, leaving the job in a session with no leader.
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

    // What the earlier run recorded: the agent's PTY led session `sid`, an
    // hour before this program existed.
    let recorded = crate::process_sessions::ProcessSession {
        sid,
        started_at_secs: crate::process_sessions::ProcessSession::started_now(sid).started_at_secs
            - 3600,
        boot: crate::process_sessions::current_boot(),
    };
    engine
        .session_store
        .insert_pending_worktree_removal(&crate::storage::PendingWorktreeRemoval {
            session_id: session.id.clone(),
            label: "gone".to_string(),
            project_path: repo.to_string_lossy().into_owned(),
            managed: session.workspace.as_managed().unwrap().clone(),
            delete_branch: None,
            process_sessions: vec![recorded],
            process_snapshot: Vec::new(),
        })
        .unwrap();

    engine.resume_pending_worktree_removals();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(crate::worker::WorkerEvent::StatusOpCompleted { .. }) =
            engine.worker_rx.recv_timeout(Duration::from_millis(100))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the resumed removal never reported"
        );
    }
    let alive = rustix::process::test_kill_process(daemon_pid).is_ok();
    let _ = rustix::process::kill_process(daemon_pid, rustix::process::Signal::KILL);
    assert!(!worktree.exists(), "the recorded removal ran");
    assert!(
        alive,
        "dux killed pid {daemon}, an unrelated program started an hour after the session it \
         recorded, because the program's leaderless session reused that session's number"
    );
}

/// The first agent of a project, with dux's worktrees folder reached through a
/// symlink (a dotfiles-managed `~/.config`, say). The create takes its hold on
/// the new worktree's path BEFORE the project's folder under the worktrees
/// root exists, so the hold's key falls back to the path as spelled, symlink
/// and all, while everything else (git's listing, the manager's request, the
/// agent's recorded path) names the resolved folder. The manager then sees no
/// create in the folder and removes the worktree under the running create.
#[test]
fn the_manager_refuses_a_first_worktree_being_created_through_a_symlinked_root() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    let real = tmp.path().join("real-worktrees");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.path().join("linked-worktrees");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    engine.paths.worktrees_root = link.clone();
    let mut project = sample_project("p1", repo.to_str().unwrap());
    project.startup_command = Some("sleep 4".to_string());
    engine.projects.push(project.clone());

    engine
        .apply(Command::DispatchCreateAgentRequest {
            request: Box::new(crate::worker::CreateAgentRequest::NewProject {
                project,
                custom_name: Some("fresh".into()),
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: false,
            }),
            busy_message: "Creating\u{2026}".to_string().into(),
            term_size: (80, 24),
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let created = loop {
        let out = crate::test_git::fixture_git()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        if let Some(found) = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .find(|path| path.contains("fresh"))
        {
            break PathBuf::from(found);
        }
        assert!(
            Instant::now() < deadline,
            "the create never made its worktree"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        engine.is_in_flight(&crate::engine::InFlightKey::CreateAgent),
        "the create is still running its startup command"
    );

    let admission = engine
        .admit_manager_removal("p1", &created, true)
        .expect("known project");
    let admitted = matches!(admission, RemovalAdmission::Admitted(_));
    drop(admission);
    // Let the create finish so nothing outlives the test.
    let settle = Instant::now() + Duration::from_secs(30);
    while Instant::now() < settle {
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let done = matches!(
            event,
            crate::worker::WorkerEvent::AgentLaunchReady(_)
                | crate::worker::WorkerEvent::AgentLaunchFailed(_)
                | crate::worker::WorkerEvent::CreateAgentFailed { .. }
        );
        engine.process_worker_event(event);
        if done {
            break;
        }
    }
    assert!(
        !admitted,
        "the manager admitted removing {} while an agent was being created in it",
        created.display()
    );
}
