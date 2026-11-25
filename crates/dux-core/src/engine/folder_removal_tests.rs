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

/// Begin the delete and, when it runs inline, delete the record, as both
/// surfaces do.
fn delete(engine: &mut Engine, id: &str, folder: FolderOnDelete) -> BeginDeleteSessionOutcome {
    let outcome = engine.begin_delete_session_full(
        id,
        WorktreeOnDelete::Keep,
        &BranchDeleteRequest::from_legacy(None),
        folder,
    );
    if let BeginDeleteSessionOutcome::Inline { .. } = outcome {
        engine
            .finish_delete_session(id)
            .expect("the record is deleted");
    }
    outcome
}

/// A program in its own session whose current folder is `cwd`, killed when
/// dropped.
struct Sleeper(std::process::Child);

impl Sleeper {
    fn start(cwd: &Path) -> Self {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 30"]).current_dir(cwd);
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        Self(command.spawn().expect("spawn sleep"))
    }

    fn session(&self) -> crate::process_sessions::ProcessSession {
        crate::process_sessions::ProcessSession::started_now(self.0.id())
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn pending_rows(engine: &Engine) -> Vec<String> {
    engine
        .session_store
        .load_pending_folder_removals()
        .unwrap()
        .into_iter()
        .map(|row| row.session_id)
        .collect()
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

fn marker_is_there(folder: &Path) -> bool {
    folder.join(crate::agent_folders::MARKER_FILE_NAME).exists()
}

fn wait_for_marker_to_go(folder: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while marker_is_there(folder) {
        assert!(
            Instant::now() < deadline,
            "the marker file was never removed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A folder the delete keeps is the person's from then on, so nothing in it
/// says dux may delete it.
#[test]
fn keeping_the_folder_dux_created_lets_go_of_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::write(folder.join("a.md"), "x").unwrap();
    let _ = delete(&mut engine, "otter", FolderOnDelete::Keep);
    wait_for_marker_to_go(&folder);
    assert!(folder.join("a.md").exists());
}

#[test]
fn a_folder_kept_because_a_program_works_in_it_is_let_go_of() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let _program = Sleeper::start(&folder);
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("Kept the folder"), "{text}");
    wait_for_marker_to_go(&folder);
    assert!(folder.is_dir());
}

/// A folder another agent works in is kept on the engine's own look, and let
/// go of the same.
#[test]
fn a_folder_kept_because_another_agent_works_in_it_is_let_go_of() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    engine.sessions.push(sample_standalone_session(
        "other",
        &folder.to_string_lossy(),
    ));
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("runs in"), "{text}");
    wait_for_marker_to_go(&folder);
}

/// A deletion that cannot go on names where it stopped, keeps the rest, and
/// is not tried again.
#[test]
fn a_deletion_that_stops_says_where_and_is_not_tried_again() {
    use std::os::unix::fs::PermissionsExt as _;
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let locked = folder.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("inner.txt"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        text.contains(
            "dux could not delete \\\"locked\\\", because dux does not have permission to \
             delete it, so nothing in it was deleted. Delete it yourself if you no longer \
             need it."
        ),
        "{text}"
    );
    assert!(locked.join("inner.txt").exists());
    assert_eq!(pending_rows(&engine), Vec::<String>::new());
    wait_for_marker_to_go(&folder);
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
    assert!(
        text.contains(crate::agent_folders::MARKER_FILE_NAME),
        "{text}"
    );
    assert!(folder.join("theirs.txt").exists());
}

#[test]
fn a_folder_already_gone_says_nothing_was_deleted() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::remove_dir_all(&folder).unwrap();
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("so nothing was deleted"), "{text}");
}

/// dux cannot know where a moved agents folder went, so it claims nothing
/// about the folder beyond its recorded path.
#[test]
fn a_folder_moved_away_is_not_called_gone() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::rename(tmp.path().join("agents"), tmp.path().join("moved")).unwrap();
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(!text.contains("gone"), "{text}");
    assert!(
        text.contains("Nothing is at the path of the folder"),
        "{text}"
    );
    assert!(
        text.contains(
            "so nothing was deleted. A folder that was moved is still wherever it was moved to."
        ),
        "{text}"
    );
    assert!(!folder.exists());
    assert!(tmp.path().join("moved/otter").exists());
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
    assert!(text.contains("runs in"), "{text}");
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
        delete_folder: Some(true),
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
            delete_folder: Some(true),
        })
        .unwrap();
    let _ = folder_final(&engine);
    assert!(!folder.exists());
}

/// A delete that asks nobody about the folder leaves it to who made it: a
/// folder dux created goes with the agent.
#[test]
fn an_unasked_delete_deletes_the_folder_dux_created() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::write(folder.join("a.md"), "x").unwrap();
    let outcome = delete(&mut engine, "otter", FolderOnDelete::Unasked);
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

/// A delete that asks nobody keeps a folder the user chose, and is not refused.
#[test]
fn an_unasked_delete_keeps_a_folder_the_user_chose() {
    let (mut engine, _tmp) = test_engine();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("mine.txt"), "mine").unwrap();
    let session = sample_standalone_session("notes", &folder.path().to_string_lossy());
    engine.session_store.upsert_session(&session).unwrap();
    engine.sessions.push(session);
    let outcome = delete(&mut engine, "notes", FolderOnDelete::Unasked);
    assert!(
        matches!(
            outcome,
            BeginDeleteSessionOutcome::Inline {
                removal: WorktreeRemoval::NothingToRemove {
                    folder: FolderFate::KeptChosen,
                    ..
                }
            }
        ),
        "{outcome:?}"
    );
    assert!(engine.pending_folder_removals.is_empty());
    assert!(folder.path().join("mine.txt").exists());
}

/// The wire with no folder answer, as the command line sends it without a
/// folder flag, deletes the folder dux created.
#[test]
fn the_wire_with_no_folder_answer_follows_who_made_the_folder() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let _ = engine
        .apply_wire(crate::wire::WireCommand::DeleteSession {
            session_id: "otter".to_string(),
            delete_worktree: false,
            delete_branch: None,
            branches: None,
            delete_folder: None,
        })
        .unwrap();
    let _ = folder_final(&engine);
    assert!(!folder.exists());
}

/// Another agent whose recorded folder reaches the deleted one through a
/// symbolic link still keeps it: the paths are compared resolved.
#[test]
fn a_folder_another_agent_reaches_through_a_link_is_kept() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let link = tmp.path().join("shortcut");
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    std::fs::write(folder.join("theirs.txt"), "keep").unwrap();
    let other: AgentSession = sample_standalone_session("other", &link.to_string_lossy());
    engine.sessions.push(other);

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("runs in"), "{text}");
    assert!(folder.join("theirs.txt").exists());
}

/// Renaming an agent whose folder dux created leaves the folder where it is,
/// and the record still says dux created it, also once read back from disk.
#[test]
fn renaming_keeps_the_folder_and_that_dux_created_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let _ = engine
        .apply_wire(crate::wire::WireCommand::RenameSession {
            session_id: "otter".to_string(),
            title: "badger".to_string(),
            rename_branch: false,
        })
        .unwrap();
    let session = engine.sessions.iter().find(|s| s.id == "otter").unwrap();
    assert_eq!(session.title.as_deref(), Some("badger"));
    assert_eq!(session.folder_path(), Some(folder.to_str().unwrap()));
    assert!(session.workspace.folder_created_by_dux());
    assert!(folder.is_dir());
    let stored = engine.session_store.load_sessions().unwrap();
    let stored = stored.iter().find(|s| s.id == "otter").unwrap();
    assert_eq!(stored.title.as_deref(), Some("badger"));
    assert!(stored.workspace.folder_created_by_dux());
}

/// A shell dux started elsewhere and `cd`'d into the folder keeps it: the
/// same working-directory check a worktree removal makes.
#[test]
fn a_process_dux_started_standing_in_the_folder_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    std::fs::write(folder.join("work.txt"), "keep").unwrap();
    let shell = Sleeper::start(&folder);
    engine
        .process_registry
        .register("a-terminal", shell.session(), tmp.path());

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("Kept the folder"), "{text}");
    assert!(text.contains("working in it"), "{text}");
    assert!(folder.join("work.txt").exists());
}

/// Any program working in the folder keeps it, whoever started it.
#[test]
fn a_program_dux_did_not_start_working_in_the_folder_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let _program = Sleeper::start(&folder);

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("is working in it"), "{text}");
    assert!(folder.is_dir());
}

/// An agent being created on the folder keeps it.
#[test]
fn an_agent_being_created_on_the_folder_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    engine
        .worktree_ops()
        .hold_as(
            crate::worktree_ops::HoldOwner::CreateOp("op-1".to_string()),
            &folder,
            crate::worktree_ops::WorktreeOpKind::CreateAgent,
        )
        .expect("the create holds the folder");

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("an agent is being created in it"), "{text}");
    assert!(folder.is_dir());
}

/// A process the deleted agent left running in the background keeps the
/// folder: dux never ends a standalone agent's processes.
#[test]
fn a_process_the_agent_left_running_keeps_the_folder() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    let server = Sleeper::start(&folder);
    engine
        .process_registry
        .register_standalone("otter", server.session(), &folder);

    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("still running"), "{text}");
    assert!(folder.is_dir());
}

/// A record that cannot be deleted keeps the folder, and the agent stays
/// recorded with the folder it runs in.
#[test]
fn a_record_that_cannot_be_deleted_keeps_the_folder() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    rusqlite::Connection::open(&engine.paths.sessions_db_path)
        .unwrap()
        .execute_batch(
            "create trigger refuse_session_delete before delete on agent_sessions \
             begin select raise(abort, 'disk is full'); end",
        )
        .unwrap();

    let outcome = engine.begin_delete_session_full(
        "otter",
        WorktreeOnDelete::Keep,
        &BranchDeleteRequest::from_legacy(None),
        FolderOnDelete::Remove,
    );
    assert!(matches!(outcome, BeginDeleteSessionOutcome::Inline { .. }));
    assert!(engine.finish_delete_session("otter").is_err());
    let _ = engine.reap_terminating_ptys();

    assert!(engine.pending_folder_removals.is_empty());
    assert_eq!(pending_rows(&engine), Vec::<String>::new());
    assert!(engine.sessions.iter().any(|s| s.id == "otter"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(folder.is_dir(), "the folder stays with its agent");
    assert!(
        !engine.worktree_ops().is_being_removed(&folder),
        "the folder is free for the agent again"
    );
}

/// The removal is written down with the record's deletion, so a quit while
/// it waits for the agent's processes leaves it for the next start, which
/// finishes it and says so.
#[test]
fn a_removal_left_waiting_at_a_quit_is_finished_at_the_next_start() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    engine
        .providers
        .insert(TabId::new("otter-slot"), spawn_cat(&folder));
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    assert_eq!(pending_rows(&engine), vec!["otter".to_string()]);
    assert!(folder.is_dir());

    // dux quits: what it held in memory is gone, and the agent's process
    // ends with it.
    engine.pending_folder_removals.clear();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine.terminating_ptys.is_empty() {
        let _ = engine.reap_terminating_ptys();
        assert!(Instant::now() < deadline, "the agent never reaped");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(folder.is_dir());

    // The next start, on the same database.
    let (mut next, _next_tmp) = test_engine();
    next.session_store =
        crate::storage::SessionStore::open(&engine.paths.sessions_db_path).unwrap();
    next.paths.sessions_db_path = engine.paths.sessions_db_path.clone();
    next.resume_pending_folder_removals();
    let text = folder_final(&next);
    assert!(text.contains("Deleted the folder"), "{text}");
    assert!(
        text.contains("had not finished when it last quit"),
        "{text}"
    );
    assert!(!folder.exists());
    assert_eq!(pending_rows(&next), Vec::<String>::new());
}

/// A folder recorded with no mark is kept, with the reason.
#[test]
fn a_folder_recorded_without_a_mark_is_kept_and_said_so() {
    let (mut engine, tmp) = test_engine();
    let folder = created_agent(&mut engine, "otter", &tmp.path().join("agents"));
    if let AgentWorkspace::Folder(workspace) = &mut engine.sessions[0].workspace
        && let FolderOrigin::CreatedByDux(identity) = &mut workspace.origin
    {
        identity.mark = None;
    }
    let _ = delete(&mut engine, "otter", FolderOnDelete::Remove);
    let text = folder_final(&engine);
    assert!(text.contains("no mark on record"), "{text}");
    assert!(folder.is_dir());
}
