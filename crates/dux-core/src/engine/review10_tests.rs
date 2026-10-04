//! Review 10: reproductions.

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

/// The changes pane's "delete" of an untracked nested repository (web route
/// and TUI worker alike) builds its check on the engine, then runs the delete
/// off it; unlike the editor's delete it takes no claim on the folder. A
/// project added at that repository in between is not seen: `clear` re-reads
/// only the processes, and the occupant sentence was decided before the
/// project existed. The project's repository is deleted whole.
#[test]
fn review10_a_project_added_between_the_discard_check_and_the_delete_is_not_deleted() {
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

    // The route, following the one destructive protocol: the folder is
    // claimed first, then the check is built under the claim.
    let claim = engine
        .worktree_ops()
        .claim_for_destructive_within(&nested, Duration::ZERO)
        .expect("the claim");
    let check = engine.destructive_check(&nested);

    // Another request reaches the engine in between: the nested repository
    // is added as a project. The claim keeps it out.
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
    assert!(
        engine.projects.iter().all(|project| project.id != "p2"),
        "the claim on the folder kept a project add out of it"
    );
    // And a project row that reached the database by any other path is seen
    // when clearing: the whole occupancy rule is asked again.
    engine
        .session_store
        .upsert_project(&crate::config::ProjectConfig {
            id: "p3".to_string(),
            path: nested.to_string_lossy().into_owned(),
            name: Some("vendor".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        })
        .unwrap();

    // The route's worker: the confirmed delete of the repository.
    let outcome = crate::git::discard_confirmed(
        &worktree,
        "vendor-lib",
        true,
        Some(crate::git::ConfirmedEntry::Repository),
        || check.clear(&[&claim], "delete"),
    );
    assert!(
        nested.join("precious.txt").exists(),
        "project p2's repository at {} was deleted by the changes pane's delete \
         (outcome {outcome:?})",
        nested.display()
    );
}

/// An agent delete announced while an editor's destructive claim on the very
/// same folder is held (the editor's delete of it is about to be REFUSED,
/// because the agent lives there) joins that claim, and takes the refused
/// claim's outcome as its own: the agent is gone and its worktree is kept,
/// with an error, though nothing was in the worktree's way.
#[test]
fn review10_an_agent_delete_does_not_fail_because_a_refused_editor_claim_was_held() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");

    // A terminal-rooted editor at a folder containing the worktree claims it
    // for a delete (the engine is about to refuse it: an agent lives there).
    let lease = engine
        .worktree_ops()
        .claim_for_destructive_within(&worktree, Duration::from_millis(10))
        .expect("the claim");
    assert!(
        engine
            .destructive_check(&worktree)
            .clear(&[&lease], "delete")
            .is_err()
    );

    // The user deletes the agent with its worktree at that moment.
    assert!(matches!(
        engine.begin_delete_session("s-agent", true, None),
        BeginDeleteSessionOutcome::AsyncStarted { .. }
    ));
    engine.finish_delete_session_memory("s-agent");
    // The editor's refusal drops its claim.
    drop(lease);

    let outcome = pump_until_removed(&mut engine);
    assert!(
        outcome.is_ok() && !worktree.exists(),
        "the agent was deleted, but its worktree was kept: {outcome:?}"
    );
}
