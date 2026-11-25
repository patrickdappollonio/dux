//! Deleting a standalone agent together with its folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::branch_history::BranchDeleteRequest;
use crate::engine::test_support::{
    sample_project, sample_session, sample_standalone_session, test_engine,
};
use crate::engine::{
    BeginDeleteSessionOutcome, Engine, FolderFate, FolderOnDelete, WorktreeOnDelete,
    WorktreeRemoval,
};
use crate::ids::TabId;
use crate::model::{AgentSession, AgentWorkspace, FolderOrigin, FolderWorkspace};
use crate::pty::PtyClient;
use crate::worker::WorkerEvent;

/// A standalone agent whose folder dux created under `root`.
fn created_agent(engine: &mut Engine, id: &str, root: &Path) -> PathBuf {
    let created = crate::agent_folders::create_agent_folder(root, id).unwrap();
    let mut session = sample_standalone_session(id, &created.path.to_string_lossy());
    session.workspace = AgentWorkspace::Folder(FolderWorkspace {
        folder_path: created.path.to_string_lossy().into_owned(),
        origin: FolderOrigin::CreatedByDux(created.identity),
    });
    engine.session_store.upsert_session(&session).unwrap();
    engine.sessions.push(session);
    created.path
}

fn delete(engine: &mut Engine, id: &str, folder: FolderOnDelete) -> BeginDeleteSessionOutcome {
    engine.begin_delete_session_full(
        id,
        WorktreeOnDelete::Keep,
        &BranchDeleteRequest::from_legacy(None),
        folder,
    )
}

/// The final the folder removal ends on, as text.
fn folder_final(engine: &Engine) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match engine.worker_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(WorkerEvent::StatusOpCompleted { resolved }) => return format!("{resolved:?}"),
            Ok(WorkerEvent::PollerStatus(status))
                if status.tone != crate::statusline::StatusTone::Busy =>
            {
                return format!("{status:?}");
            }
            Ok(_) | Err(_) => {}
        }
        assert!(Instant::now() < deadline, "the folder removal never ended");
    }
}

fn spawn_cat(cwd: &Path) -> PtyClient {
    PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
}

#[test]
fn deleting_an_agent_with_the_folder_dux_created_deletes_the_folder() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("notes/a.md"), "x").unwrap();

    let outcome = delete(&mut engine, "otter", FolderOnDelete::Remove);
    assert!(
        matches!(
            outcome,
            BeginDeleteSessionOutcome::Inline {
                removal: WorktreeRemoval::NothingToRemove {
                    folder: FolderFate::Deleting,
                    ..
                }
            }
        ),
        "{outcome:?}"
    );
    let text = folder_final(&engine);
    assert!(text.contains("Deleted the folder"), "{text}");
    assert!(!folder.exists());
}

#[test]
fn keeping_the_folder_dux_created_leaves_it_with_everything_in_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::write(folder.join("a.md"), "x").unwrap();
    let outcome = delete(&mut engine, "otter", FolderOnDelete::Keep);
    assert!(matches!(
        outcome,
        BeginDeleteSessionOutcome::Inline {
            removal: WorktreeRemoval::NothingToRemove {
                folder: FolderFate::KeptCreated,
                ..
            }
        }
    ));
    assert!(folder.join("a.md").exists());
}

/// The record decides, never the request: asking for the folder of an agent
/// whose folder the user chose refuses the whole delete and touches nothing.
#[test]
fn asking_to_delete_a_folder_the_user_chose_is_refused_and_nothing_is_deleted() {
    let (mut engine, _tmp) = test_engine();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("mine.txt"), "mine").unwrap();
    let session = sample_standalone_session("notes", &folder.path().to_string_lossy());
    engine.session_store.upsert_session(&session).unwrap();
    engine.sessions.push(session);

    let outcome = delete(&mut engine, "notes", FolderOnDelete::Remove);
    let BeginDeleteSessionOutcome::Refused { message } = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert!(
        message
            .to_string()
            .contains("dux only deletes folders it created"),
        "{message}"
    );
    assert!(
        engine.sessions.iter().any(|s| s.id == "notes"),
        "the agent stays"
    );
    assert!(folder.path().join("mine.txt").exists());
}

#[test]
fn asking_to_delete_the_folder_of_an_agent_in_a_worktree_is_refused() {
    let (mut engine, _tmp) = test_engine();
    engine.projects.push(sample_project("p1", "/tmp/p1"));
    let session = sample_session("s1", "p1", "feat");
    engine.session_store.upsert_session(&session).unwrap();
    engine.sessions.push(session);
    let outcome = delete(&mut engine, "s1", FolderOnDelete::Remove);
    assert!(matches!(outcome, BeginDeleteSessionOutcome::Refused { .. }));
    assert!(engine.sessions.iter().any(|s| s.id == "s1"));
}

/// The folder waits for the agent's processes, and goes once the reaper has
/// seen the last of them exit.
#[test]
fn the_folder_waits_for_the_agents_process_to_exit() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    engine
        .providers
        .insert(TabId::new("otter-slot"), spawn_cat(&folder));

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    assert_eq!(engine.pending_folder_removals.len(), 1);
    assert!(folder.exists(), "nothing is deleted while the agent runs");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine.pending_folder_removals.is_empty() {
        let _ = engine.reap_terminating_ptys();
        assert!(Instant::now() < deadline, "the agent never reaped");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = folder_final(&engine);
    assert!(text.contains("Deleted the folder"), "{text}");
    assert!(!folder.exists());
}

#[test]
fn a_folder_replaced_since_dux_created_it_is_kept_and_said_so() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::rename(&folder, tmp.path().join("moved")).unwrap();
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("theirs.txt"), "keep").unwrap();

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("Kept the folder"), "{text}");
    assert!(text.contains("no longer the one dux created"), "{text}");
    assert!(folder.join("theirs.txt").exists());
}

#[test]
fn a_folder_already_gone_is_said_to_be_gone() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::remove_dir(&folder).unwrap();
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("was already gone"), "{text}");
}

/// Another agent now working in the folder keeps it.
#[test]
fn a_folder_another_agent_works_in_is_kept() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let inside = folder.join("sub");
    std::fs::create_dir(&inside).unwrap();
    let other: AgentSession = sample_standalone_session("other", &inside.to_string_lossy());
    engine.sessions.push(other);

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("now works in it"), "{text}");
    assert!(inside.exists());
}

/// The wire's `delete_folder` reaches the engine, and a request that claims
/// a folder the record says the user chose is refused through the wire too.
#[test]
fn the_wire_flag_is_judged_by_the_record() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let chosen = tempfile::tempdir().unwrap();
    let session = sample_standalone_session("notes", &chosen.path().to_string_lossy());
    engine.session_store.upsert_session(&session).unwrap();
    engine.sessions.push(session);

    let refused = engine.apply_wire(crate::wire::WireCommand::DeleteSession {
        session_id: "notes".to_string(),
        delete_worktree: false,
        delete_branch: None,
        branches: None,
        delete_folder: true,
    });
    let refused_text = format!("{refused:?}");
    assert!(
        refused_text.contains("dux only deletes folders it created"),
        "{refused_text}"
    );
    assert!(chosen.path().exists());

    let _ = engine
        .apply_wire(crate::wire::WireCommand::DeleteSession {
            session_id: "otter".to_string(),
            delete_worktree: false,
            delete_branch: None,
            branches: None,
            delete_folder: true,
        })
        .unwrap();
    let _ = folder_final(&engine);
    assert!(!folder.exists());
}
