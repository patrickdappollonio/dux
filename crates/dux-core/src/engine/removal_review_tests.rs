//! Adversarial review tests for removal coordination. Each test asserts the
//! behaviour the branch promises; a failure is a reproduced defect.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{BeginDeleteSessionOutcome, Command, Engine};
use crate::worktree_manager::RemovalAdmission;

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).into_owned()
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

/// One dux-made worktree `name` under the project's worktrees dir, and an agent
/// `s-<name>` on it.
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

fn registered_worktrees(repo: &Path) -> Vec<PathBuf> {
    git(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

/// An agent being created in a NEW worktree is "an agent being created on
/// it": while its startup command provisions the folder, the worktree manager
/// lists the folder and must refuse to remove it, judged from live state.
/// At the tip only an agent adopting an EXISTING worktree registers in the
/// registry, so the manager admits the removal and `git worktree remove
/// --force` runs under the create.
#[test]
fn the_manager_refuses_a_worktree_a_new_agent_is_being_created_in() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    let mut project = sample_project("p1", repo.to_str().unwrap());
    // Long enough to look while the create provisions the folder.
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

    // Wait for the create's worktree to be registered (the startup command is
    // running in it now).
    let deadline = Instant::now() + Duration::from_secs(20);
    let created = loop {
        let listed = registered_worktrees(&repo);
        if let Some(found) = listed
            .into_iter()
            .find(|path| path.to_string_lossy().contains("fresh"))
        {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "the create never made its worktree"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        engine.is_in_flight(&crate::engine::InFlightKey::CreateAgent),
        "the create is still running"
    );

    // The manager lists it as a free worktree with a Remove action.
    let project = engine.projects[0].clone();
    let listed = crate::worktree_manager::list_manageable_worktrees(
        &project,
        &engine.paths,
        &engine.sessions,
        engine.worktree_ops(),
    )
    .unwrap();
    let row_removable = listed
        .iter()
        .any(|entry| entry.path.to_string_lossy().contains("fresh") && entry.is_removable());

    let admission = engine
        .admit_manager_removal("p1", &created, true)
        .expect("known project");
    let admitted = matches!(admission, RemovalAdmission::Admitted(_));
    let mut removed_under_create = false;
    if let RemovalAdmission::Admitted(ticket) = admission {
        let _ = ticket.run();
        removed_under_create =
            !created.exists() && engine.is_in_flight(&crate::engine::InFlightKey::CreateAgent);
    }
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
        !row_removable && !admitted && !removed_under_create,
        "listed as removable: {row_removable}; removed while the create still ran: \
         {removed_under_create}. The manager admitted the removal of {} while an agent was being created in it",
        created.display()
    );
}

/// Deleting an agent but KEEPING its worktree stops its process with a grace
/// period. Until that process has exited it is still running in the folder,
/// so a worktree-manager removal decided "from live state" must not run
/// `git worktree remove --force` under it, the same rule the agent-delete
/// path keeps by deferring its own removal until the process is reaped.
#[test]
fn the_manager_does_not_remove_a_worktree_a_stopping_agent_still_runs_in() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let tab = engine.sessions[0].slot_tab_id().to_owned();
    let client = crate::pty::PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "trap '' TERM; sleep 30".to_string()],
        &worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn");
    engine.providers.insert(tab, client);

    // The delete dialog's "keep the worktree" answer, as a surface runs it.
    assert!(matches!(
        engine.begin_delete_session("s-agent", false, None),
        BeginDeleteSessionOutcome::Inline { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    assert_eq!(
        engine.terminating_ptys.len(),
        1,
        "the agent's process is still exiting"
    );

    let admission = engine
        .admit_manager_removal("p1", &worktree, false)
        .expect("known project");
    let removed = match admission {
        RemovalAdmission::Admitted(ticket) => {
            let _ = ticket.run();
            !worktree.exists()
        }
        RemovalAdmission::Refused(_) => false,
    };
    assert!(
        !removed,
        "the manager force-removed the worktree while the deleted agent's process was \
         still running in it"
    );
}

/// Two agents of a project share one worktree (the cascade's own comment
/// names the case). Deleting the project stops both; the first is deleted
/// "inline" because the other is still its sibling, and the second then
/// removes the worktree. That removal must wait for EVERY process still
/// running in the folder, not only its own agent's: here the first agent's
/// process is still in its grace period when `git worktree remove --force`
/// runs. (The process ignores SIGHUP as well as SIGTERM, and the delete waits
/// until its trap is installed: closing a PTY sends both signals, so a process
/// that only ignored SIGTERM, or had not set its trap yet, would be gone at
/// once and there would be nothing left to wait for.)
#[test]
fn a_project_delete_waits_for_every_agent_process_in_a_shared_worktree() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "shared");
    // A second agent on the same folder, with no process of its own.
    let mut second = sample_session("s-second", "p1", "shared");
    if let Some(managed) = second.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine.sessions.push(second);

    let tab = engine.sessions[0].slot_tab_id().to_owned();
    let client = crate::pty::PtyClient::spawn_with_env(
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
    .expect("spawn");
    // Only once the trap is in place: a signal that lands before it would
    // end the process at once and leave nothing to wait for.
    let ready_by = Instant::now() + Duration::from_secs(5);
    while !client.has_output() {
        assert!(
            Instant::now() < ready_by,
            "the agent's process never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = client.child_process_id().expect("pid") as i32;
    engine.providers.insert(tab, client);

    engine
        .apply(Command::DeleteProject {
            project_id: "p1".into(),
            project_name: "demo".into(),
        })
        .unwrap();
    assert_eq!(
        engine.terminating_ptys.len(),
        1,
        "the first agent's process is still exiting"
    );

    // Give the removal worker the time it needs, while the process lives.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && worktree.exists() {
        if let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) {
            engine.process_worker_event(event);
        }
    }
    assert!(
        worktree.exists(),
        "the shared worktree was force-removed while an agent's process was still running in it"
    );
    assert!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()).is_ok(),
        "and that process is still running, inside its grace"
    );
    let _ = rustix::process::kill_process_group(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    );
}

/// A deleted agent's worktree removal is waiting (here for a pull). Nothing
/// new may start in the folder once its removal has begun; in particular dux
/// never removes a folder a standalone agent runs in. A standalone agent
/// created in that folder during the wait is neither refused nor seen by the
/// removal, which then force-removes the folder out from under it.
#[test]
fn a_standalone_agent_created_in_a_folder_being_removed_does_not_lose_its_folder() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
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

    // The user starts a standalone agent in that folder while the removal
    // waits.
    let reaction = engine
        .apply(Command::DispatchCreateAgentRequest {
            request: Box::new(crate::worker::CreateAgentRequest::Standalone {
                folder: worktree.clone(),
                title: "mine".into(),
                provider: crate::model::ProviderKind::new("claude"),
            }),
            busy_message: "Creating\u{2026}".to_string().into(),
            term_size: (80, 24),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut created = false;
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
            created = engine
                .sessions
                .iter()
                .any(|session| session.workspace.as_managed().is_none());
            break;
        }
    }
    if !created {
        // Refused or failed: the folder is protected the way it should be.
        let _ = reaction;
        return;
    }

    // The pull finishes; the removal goes on.
    engine.clear_in_flight(&pull);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let done = matches!(
            event,
            crate::worker::WorkerEvent::WorktreeRemoveCompleted { .. }
        );
        engine.process_worker_event(event);
        if done {
            break;
        }
    }
    assert!(
        worktree.exists(),
        "a standalone agent was created in a folder already being removed, and the removal \
         then deleted the folder it runs in"
    );
}

/// The listing says the same thing the admission does: a folder a deleted
/// agent's CLI is still stopping in is shown busy, with why, and is not offered
/// for removal.
#[test]
fn the_manager_lists_a_folder_a_stopping_agent_still_runs_in_as_busy() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let tab = engine.sessions[0].slot_tab_id().to_owned();
    let client = crate::pty::PtyClient::spawn_with_env(
        "sh",
        &["-c".to_string(), "trap '' TERM HUP; sleep 30".to_string()],
        &worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn");
    engine.providers.insert(tab, client);
    let _ = engine.begin_delete_session("s-agent", false, None);
    engine.finish_delete_session_memory("s-agent");

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
        .find(|entry| entry.path.ends_with("agent"))
        .expect("listed");
    assert!(!row.is_removable());
    assert_eq!(
        row.busy.as_deref(),
        Some("a process dux started there that is still stopping")
    );
    match engine.admit_manager_removal("p1", &worktree, false) {
        Some(RemovalAdmission::Refused(crate::worktree_manager::RemovalOutcome::Busy {
            reason,
        })) => assert_eq!(reason, "a process dux started there that is still stopping"),
        _ => panic!("refused as busy, with why"),
    }
    for entry in std::mem::take(&mut engine.terminating_ptys) {
        entry.client.force_terminate();
    }
}

/// Once a removal has claimed a folder, nothing new starts there: not a
/// relaunch of another agent sharing it, not a terminal. The removal's last
/// look before git relies on it.
#[test]
fn nothing_new_starts_in_a_folder_being_removed() {
    let (mut engine, tmp) = test_engine();
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let _claim = engine.worktree_ops().announce_removal(&worktree);

    let session = engine.sessions[0].clone();
    let request = engine.build_agent_launch_request(
        session,
        false,
        (24, 80),
        crate::worker::AgentLaunchKind::Reconnect {
            status_message: "Reconnecting".to_string().into(),
        },
    );
    let reaction = engine
        .apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        })
        .unwrap();
    let crate::engine::EventReaction::DispatchAgentLaunchView(view) = reaction else {
        panic!("a launch view");
    };
    assert!(!view.launched, "the launch is refused");
    assert!(
        view.status
            .expect("with a sentence")
            .message
            .contains("dux is removing the worktree")
    );
    let refused = engine
        .create_companion_terminal("s-agent", 24, 80)
        .expect_err("no terminal opens there");
    assert!(
        refused.to_string().contains("dux is removing the worktree"),
        "{refused}"
    );
}
