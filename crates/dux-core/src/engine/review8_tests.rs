//! Review 8: reproductions.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{BeginDeleteSessionOutcome, Command, Engine, ProjectPersistenceAction};

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

/// A project added while an agent's worktree removal is waiting, whose
/// repository is inside that worktree, is deleted by the removal.
#[test]
fn review8_a_project_added_inside_a_worktree_being_removed_is_not_deleted() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let nested = worktree.join("vendor-lib");
    init_repo(&nested);
    std::fs::write(nested.join("precious.txt"), "the user's work").unwrap();

    // Keep the removal waiting.
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
    // Let the removal worker start and reach its wait.
    for removal in engine.reap_terminating_ptys().removals {
        let _ = engine.dispatch_deferred_worktree_removal(removal);
    }
    std::thread::sleep(Duration::from_millis(500));

    let mut project = sample_project("p2", nested.to_str().unwrap());
    project.name = "vendor".to_string();
    let _ = engine
        .apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project,
                status_message: "Added".to_string().into(),
            }),
            status_op_id: None,
        })
        .unwrap();
    let added = engine.projects.iter().any(|project| project.id == "p2");

    engine.clear_in_flight(&pull);
    pump_until_removed(&mut engine);

    if added {
        assert!(
            nested.join("precious.txt").exists(),
            "project p2 was added at {} while its enclosing worktree was being removed, \
             and the removal deleted the project's repository",
            nested.display()
        );
    }
}

/// The worktree manager admits a removal, and before its worker claims the
/// folder a standalone agent is created on a subfolder of it (its launch then
/// fails, so nothing of it runs). The re-check under the claim reads the agents
/// again but only matches a worktree's EXACT path, so the standalone agent's
/// folder, inside the worktree, is deleted with it.
#[test]
fn review8_the_manager_recheck_under_the_claim_sees_a_standalone_agent_inside() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "left");
    engine.sessions.retain(|session| session.id != "s-left");

    let admission = engine
        .admit_manager_removal("p1", &worktree, false)
        .expect("known project");
    let crate::worktree_manager::RemovalAdmission::Admitted(removal) = admission else {
        panic!("the free worktree was not admitted");
    };

    // Lands between admission and the worker's claim.
    let inside = worktree.join("frontend");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("notes.txt"), "the user's own work").unwrap();
    let standalone =
        crate::engine::test_support::sample_standalone_session("s-alone", inside.to_str().unwrap());
    engine.session_store.create_session(&standalone).unwrap();
    engine.sessions.push(standalone);

    let outcome = removal.run();
    assert!(
        inside.join("notes.txt").exists(),
        "the manager removed {} although standalone agent s-alone lives in {} ({outcome:?})",
        worktree.display(),
        inside.display()
    );
}

/// Registering a PTY's session runs on the engine thread (a launch landing, a
/// terminal opening). With the session database attached, every registration
/// opens a new SQLite connection and rewrites the whole registry, and waits
/// for any other writer of the database to finish: the UI thread blocks.
#[test]
fn review8_registering_a_session_does_not_block_on_the_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("sessions.sqlite3");
    let _ = crate::storage::SessionStore::open(&db).unwrap();
    let registry = crate::process_sessions::AgentProcessRegistry::default();
    registry.attach_store(&db);
    std::thread::sleep(Duration::from_millis(200));

    // Uncontended cost of one registration.
    let started = Instant::now();
    for n in 0..20u32 {
        registry.register(
            "agent",
            crate::process_sessions::ProcessSession {
                sid: 4_000_000 + n,
                started_at: 1,
                boot: crate::process_sessions::current_boot(),
            },
            tmp.path(),
        );
    }
    let per_call = started.elapsed() / 20;
    eprintln!("uncontended register: {per_call:?} per call");

    // Another connection (a worker) holding a write transaction for 2s.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let holder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        conn.execute_batch("COMMIT").unwrap();
    });
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    registry.register(
        "agent",
        crate::process_sessions::ProcessSession {
            sid: 4_100_000,
            started_at: 1,
            boot: crate::process_sessions::current_boot(),
        },
        tmp.path(),
    );
    let blocked = started.elapsed();
    holder.join().unwrap();
    eprintln!("contended register: {blocked:?}");
    assert!(
        blocked < Duration::from_millis(500),
        "registering a session on the engine thread blocked for {blocked:?} waiting for the \
         session database"
    );
}

// Tests added with the fixes for the eighth review.

/// A project cannot be added inside a folder whose removal has begun, and
/// the refusal says why.
#[test]
fn adding_a_project_inside_a_folder_being_removed_is_refused() {
    let (mut engine, tmp) = test_engine();
    let outer = tmp.path().join("going");
    let repo = outer.join("lib");
    init_repo(&repo);
    let claim = engine.worktree_ops().announce_removal(&outer);
    let reaction = engine
        .apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project: sample_project("p9", repo.to_str().unwrap()),
                status_message: "Added".to_string().into(),
            }),
            status_op_id: None,
        })
        .unwrap();
    drop(claim);
    assert!(engine.projects.iter().all(|project| project.id != "p9"));
    let crate::engine::EventReaction::Status(status) = reaction else {
        panic!("the add answers with a refusal");
    };
    assert!(
        status.message.to_string().contains("add a project there"),
        "{}",
        status.message
    );
}

/// The last look before git asks the one occupancy rule over the projects
/// as the session database has them: a project whose repository lies inside
/// the worktree keeps it, and is named.
#[test]
fn the_last_look_keeps_a_worktree_holding_a_projects_repository() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let nested = worktree.join("vendor-lib");
    init_repo(&nested);
    // A removal kept waiting, then a project row written straight into the
    // database (as an add on another path would have), inside the worktree.
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
    for removal in engine.reap_terminating_ptys().removals {
        let _ = engine.dispatch_deferred_worktree_removal(removal);
    }
    engine
        .session_store
        .upsert_project(&crate::config::ProjectConfig {
            id: "p2".to_string(),
            path: nested.to_string_lossy().into_owned(),
            name: Some("vendor".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        })
        .unwrap();
    engine.clear_in_flight(&pull);
    let deadline = Instant::now() + Duration::from_secs(60);
    let result = loop {
        assert!(Instant::now() < deadline, "the removal never reported");
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        if let crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } = event {
            break result;
        }
    };
    assert!(nested.join("f.txt").exists(), "{result:?}");
    let message = result.expect_err("the worktree was kept");
    assert!(message.contains("project \"vendor\""), "{message}");
}

/// A keep-worktree delete ends the agent's running startup command too, and
/// the run then says, truthfully, that dux stopped it.
#[test]
fn a_keep_worktree_delete_ends_the_startup_command() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let session = engine.sessions[0].clone();
    let pidfile = tmp.path().join("cmd.pid");
    let run = crate::startup::StartupCommandRun {
        project: engine.projects[0].clone(),
        managed: session.workspace.as_managed().unwrap().clone(),
        session,
        command: format!("echo $$ > '{}'; sleep 30", pidfile.display()),
        terminal: crate::config::StartupCommandTerminalConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string()],
        },
        env: Vec::new(),
    };
    let paths = engine.paths.clone();
    let registry = engine.process_registry.clone();
    let handle =
        std::thread::spawn(move || crate::startup::run_startup_command(&paths, run, &registry));
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine
        .process_registry
        .startup_session_of("s-agent")
        .is_none()
    {
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    let _ = engine.begin_delete_session("s-agent", false, None);
    engine.finish_delete_session_memory("s-agent");
    let result = handle.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was ended, not waited out"
    );
    let err = result.status.expect_err("the run reports the deletion");
    assert!(err.contains("dux stopped the command"), "{err}");
    assert!(!err.contains("log:"), "{err}");
    assert!(worktree.exists(), "the worktree was kept, as asked");
}

/// A session whose leader dux spawned and has not reaped is led, whether or
/// not the process table shows the leader at all.
#[test]
fn a_leader_dux_has_not_reaped_leads_its_session_without_the_table() {
    let session = crate::process_sessions::ProcessSession {
        sid: 4_242_424,
        started_at: 1,
        boot: crate::process_sessions::current_boot(),
    };
    let member = crate::process_sessions::ProcRow {
        pid: 4_242_425,
        ppid: Some(1),
        sid: Some(4_242_424),
        start_time: 5,
        name: "job".to_string(),
        exited: false,
    };
    let table = vec![member];
    assert!(crate::process_sessions::members(&table, &[session], &[], 9).is_empty());
    crate::process_sessions::note_spawned(session);
    let found = crate::process_sessions::members(&table, &[session], &[], 9);
    crate::process_sessions::note_reaped(session);
    assert_eq!(found.len(), 1, "led while dux has not reaped its leader");
    assert!(crate::process_sessions::members(&table, &[session], &[], 9).is_empty());
}

/// Every change reaches the database through the writer, and `flush` says
/// when it has.
#[test]
fn the_writer_saves_every_change_and_flush_waits_for_it() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("sessions.sqlite3");
    let registry = crate::process_sessions::AgentProcessRegistry::default();
    registry.attach_store(&db);
    for n in 0..50u32 {
        registry.register(
            "agent",
            crate::process_sessions::ProcessSession {
                sid: 4_300_000 + n,
                started_at: 1,
                boot: crate::process_sessions::current_boot(),
            },
            tmp.path(),
        );
    }
    assert!(registry.flush(Duration::from_secs(10)));
    let stored = crate::storage::SessionStore::open(&db)
        .unwrap()
        .load_process_registry()
        .unwrap();
    assert!(stored.len() >= 50, "{}", stored.len());
}
