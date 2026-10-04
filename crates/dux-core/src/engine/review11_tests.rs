//! Review 11: reproductions.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{BeginDeleteSessionOutcome, Engine};

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

fn pump_until_removed(engine: &mut Engine) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        for removal in engine.reap_terminating_ptys().removals {
            let _ = engine.dispatch_deferred_worktree_removal(removal);
        }
        let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        let outcome = match &event {
            crate::worker::WorkerEvent::WorktreeRemoveCompleted { result, .. } => {
                Some(result.clone().map(|_| ()))
            }
            _ => None,
        };
        engine.process_worker_event(event);
        if let Some(outcome) = outcome {
            return outcome;
        }
    }
    panic!("the removal never reported");
}

/// A shell dux started (a project terminal, say) in its own session.
struct Shell(u32);
impl Drop for Shell {
    fn drop(&mut self) {
        let _ = rustix::process::kill_process_group(
            rustix::process::Pid::from_raw(self.0 as i32).unwrap(),
            rustix::process::Signal::KILL,
        );
    }
}

fn spawn_shell(cwd: &Path, script: &str) -> (Shell, std::process::Child) {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new("sh");
    command.args(["-c", script]).current_dir(cwd);
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    (Shell(child.id()), child)
}

/// The removal's last look at where dux's processes stand (the cwd rule:
/// a tracked process standing in the folder keeps it) runs BEFORE it waits
/// for an older overlapping claim. A terminal shell `cd`'d into the
/// worktree during that wait is never seen, and git deletes the folder
/// from under it.
#[test]
fn review11_a_shell_that_enters_the_worktree_while_the_removal_waits_for_an_overlap_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let sub = worktree.join("node_modules");
    std::fs::create_dir_all(&sub).unwrap();

    // A project terminal: its shell starts in the repository and, a moment
    // later, the user `cd`s it into the agent's worktree.
    let script = format!("sleep 2; cd '{}' && exec sleep 60", worktree.display());
    let (_shell, mut child) = spawn_shell(&repo, &script);
    let session = crate::process_sessions::ProcessSession::started_now(child.id());
    crate::process_sessions::note_spawned(session);
    engine
        .process_registry
        .register(crate::process_sessions::UNOWNED_PTYS, session, &repo);
    engine.process_registry.label(session, "a project terminal");

    // A changes-pane delete of an untracked folder inside the worktree is
    // running (its claim is older than the removal's).
    let claim = engine
        .worktree_ops()
        .claim_for_destructive_within(&sub, Duration::ZERO)
        .expect("the claim");

    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");

    // The changes-pane delete finishes four seconds later; by then the
    // shell is standing in the worktree.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        drop(claim);
    });
    let outcome = pump_until_removed(&mut engine);
    releaser.join().unwrap();
    let still_running = child.try_wait().unwrap().is_none();
    assert!(still_running, "the shell was ended");
    assert!(
        worktree.exists(),
        "the worktree was removed from under a shell standing in it (outcome {outcome:?})"
    );
}

/// Control: the same shell standing in the worktree BEFORE the delete keeps it.
#[test]
fn review11_control_a_shell_already_in_the_worktree_keeps_it() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let script = format!("cd '{}' && exec sleep 60", worktree.display());
    let (_shell, child) = spawn_shell(&repo, &script);
    let session = crate::process_sessions::ProcessSession::started_now(child.id());
    crate::process_sessions::note_spawned(session);
    engine
        .process_registry
        .register(crate::process_sessions::UNOWNED_PTYS, session, &repo);
    std::thread::sleep(Duration::from_millis(500));
    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    let outcome = pump_until_removed(&mut engine);
    assert!(worktree.exists(), "control: removed ({outcome:?})");
}

/// A branch rename checks `is_being_removed` and then takes its hold with
/// `let _ =`, on the comment's word that the check on the same thread means
/// it "cannot be refused". A destructive claim is taken off the engine thread
/// (the editor and changes-pane routes), so one landing in between refuses
/// the hold, the refusal is dropped, and the rename is dispatched holding
/// nothing: a removal of the worktree then neither waits for it nor follows
/// its new name.
#[test]
fn review11_a_rename_the_plan_accepts_always_holds_its_worktree() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let ops = engine.worktree_ops().clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let claimer = {
        let ops = ops.clone();
        let stop = stop.clone();
        let worktree = worktree.clone();
        std::thread::spawn(move || {
            // An editor rooted at a folder around the worktree asks to delete
            // it, again and again; each claim is refused at its check and let
            // go (the agent lives there), but it exists for a moment.
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if std::env::var("R11_NO_CLAIMS").is_ok() {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if let Ok(claim) = ops.claim_for_destructive_within(&worktree, Duration::ZERO) {
                    std::thread::sleep(Duration::from_micros(200));
                    drop(claim);
                }
                std::thread::sleep(Duration::from_micros(50));
            }
        })
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut unheld = None;
    let mut n = 0;
    while Instant::now() < deadline {
        n += 1;
        if let crate::engine::BranchRenamePlan::RenameBranch(dispatch) =
            engine.prepare_branch_rename("s-agent", &format!("renamed-{n}"), true)
        {
            let held = ops
                .holders(&worktree)
                .contains(&crate::worktree_ops::WorktreeOpKind::BranchRename);
            engine.revert_optimistic_rename("s-agent", dispatch.previous_title);
            if !held {
                unheld = Some(n);
                break;
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    claimer.join().unwrap();
    assert!(
        unheld.is_none(),
        "rename {unheld:?} was accepted for dispatch without a hold on its worktree"
    );
}

// Tests added with the fixes for the eleventh review.

/// A link is judged at its own path: one that IS an agent's recorded folder
/// is refused, one that merely points at an agent's folder is not (deleting
/// it leaves the folder where it is).
#[test]
fn a_link_is_judged_at_its_own_path_never_followed() {
    let (mut engine, tmp) = test_engine();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let at_link = tmp.path().join("at-link");
    let to_agent = tmp.path().join("to-agent");
    std::os::unix::fs::symlink(&real, &at_link).unwrap();
    std::os::unix::fs::symlink(&real, &to_agent).unwrap();
    // One standalone agent recorded AT a link, another at the real folder.
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-link",
            at_link.to_str().unwrap(),
        ));
    let ops = engine.worktree_ops().clone();
    let claim = ops.claim_for_destructive(&at_link).unwrap();
    let refused = engine
        .destructive_check(&at_link)
        .clear(&[&claim], "delete")
        .expect_err("the agent lives at the link");
    assert!(refused.0.contains("standalone agent"), "{refused}");
    drop(claim);

    engine.sessions.clear();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-real",
            real.to_str().unwrap(),
        ));
    let claim = ops.claim_for_destructive(&to_agent).unwrap();
    assert!(
        engine
            .destructive_check(&to_agent)
            .clear(&[&claim], "delete")
            .is_ok(),
        "a link that only points at an agent's folder can go: the folder stays"
    );
}

/// The changes pane's delete of an untracked link that is a standalone
/// agent's folder is refused.
#[test]
fn the_changes_pane_does_not_delete_an_untracked_link_that_is_an_agents_folder() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = worktree.join("work");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            link.to_str().unwrap(),
        ));
    let refused = engine.apply(crate::engine::Command::DiscardFile {
        worktree_path: worktree.clone(),
        path: "work".to_string(),
        is_untracked: true,
        confirmed: crate::git::ConfirmedEntry::File,
    });
    assert!(refused.is_err(), "the delete was refused");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the link is still there"
    );
}
