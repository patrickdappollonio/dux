//! Review 17: reproductions.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};
use crate::engine::{Command, Engine};
use crate::worker::{PullTarget, WorkerEvent};

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

fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--initial-branch=main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
}

/// A repository of its own, with history, at `folder`.
fn own_repository(folder: &Path) {
    init(folder);
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    git(folder, &["add", "."]);
    git(folder, &["commit", "-m", "mine"]);
}

fn wait_pull(engine: &mut Engine) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) {
            let done = matches!(event, WorkerEvent::PullCompleted { .. });
            let _ = engine.process_worker_event(event);
            if done {
                return;
            }
        }
    }
    panic!("the pull never completed");
}

/// A standalone agent lives in `scratch/`, a folder the managed agent's
/// worktree ignores (the occupancy rule treats this layout as the worktree's
/// occupant). Upstream adds a tracked FILE called `scratch` to the agent's
/// branch. The session Pull runs `git pull --ff-only`, which deletes the
/// ignored folder to make room, the standalone agent's repository and
/// history with it: no occupancy question is asked before the pull.
#[test]
fn review17_a_session_pull_never_deletes_a_standalone_agents_folder() {
    let (mut engine, tmp) = test_engine();
    let upstream = tmp.path().join("upstream");
    init(&upstream);
    std::fs::write(upstream.join(".gitignore"), "scratch/\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-m", "ignore scratch"]);
    git(&upstream, &["branch", "agent"]);
    let repo = tmp.path().join("repo");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = tmp.path().join("worktrees").join("p1-name").join("agent");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "agent",
            worktree.to_str().unwrap(),
            "origin/agent",
        ],
    );
    let mut session = sample_session("s-agent", "p1", "agent");
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine.sessions.push(session);
    let folder = worktree.join("scratch");
    own_repository(&folder);
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            folder.to_str().unwrap(),
        ));
    // Upstream: a tracked file where the standalone agent's folder is.
    git(&upstream, &["switch", "-q", "agent"]);
    std::fs::write(upstream.join("scratch"), "a file\n").unwrap();
    git(&upstream, &["add", "-f", "scratch"]);
    git(&upstream, &["commit", "-m", "scratch is a file now"]);

    let _ = engine.apply(Command::Pull {
        repo_path: worktree.clone(),
        target: PullTarget::Session,
        busy_message: crate::status_text::StatusText::from("Pulling"),
        already_running_message: crate::status_text::StatusText::from("already"),
    });
    wait_pull(&mut engine);
    assert!(
        folder.join(".git").exists() && folder.join("notes.txt").exists(),
        "the standalone agent's folder (a repository with history) was deleted by a session \
         pull; {} is now {:?}",
        folder.display(),
        std::fs::symlink_metadata(&folder).map(|m| m.file_type())
    );
}

/// The same for a project's repository: project p2 lives in `vendor/lib`, a
/// folder project p1's checkout ignores. Pull project on p1 deletes p2's
/// repository when the incoming commit tracks a file at `vendor`.
#[test]
fn review17_a_project_pull_never_deletes_another_projects_repository() {
    let (mut engine, tmp) = test_engine();
    let upstream = tmp.path().join("upstream");
    init(&upstream);
    std::fs::write(upstream.join(".gitignore"), "vendor/\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-m", "ignore vendor"]);
    let repo = tmp.path().join("repo");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let other = repo.join("vendor").join("lib");
    own_repository(&other);
    engine
        .projects
        .push(sample_project("p2", other.to_str().unwrap()));
    std::fs::write(upstream.join("vendor"), "a file\n").unwrap();
    git(&upstream, &["add", "-f", "vendor"]);
    git(&upstream, &["commit", "-m", "vendor is a file now"]);

    let _ = engine.apply(Command::Pull {
        repo_path: repo.clone(),
        target: PullTarget::Project {
            project_id: "p1".to_string(),
            project_name: "p1-name".to_string(),
            leading_branch: Some("main".to_string()),
        },
        busy_message: crate::status_text::StatusText::from("Pulling"),
        already_running_message: crate::status_text::StatusText::from("already"),
    });
    wait_pull(&mut engine);
    assert!(
        other.join(".git").exists(),
        "project p2's repository at {} was deleted by Pull project on p1",
        other.display()
    );
}

/// What [`wait_pull`] waits for, with the worker's answer.
fn pull_result(engine: &mut Engine) -> Result<crate::worker::PullOutcome, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) {
            let result = match &event {
                WorkerEvent::PullCompleted { result, .. } => Some(result.clone()),
                _ => None,
            };
            let _ = engine.process_worker_event(event);
            if let Some(result) = result {
                return result;
            }
        }
    }
    panic!("the pull never completed");
}

fn head(dir: &Path) -> String {
    crate::git::head_commit(dir).unwrap()
}

/// A clone of an upstream that ignores `vendor/`, registered as project p1.
fn project_checkout(tmp: &Path, engine: &mut Engine) -> (std::path::PathBuf, std::path::PathBuf) {
    let upstream = tmp.join("upstream");
    init(&upstream);
    std::fs::write(upstream.join(".gitignore"), "vendor/\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-m", "ignore vendor"]);
    let repo = tmp.join("repo");
    git(
        tmp,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    (upstream, repo)
}

fn session_pull(
    engine: &mut Engine,
    checkout: &Path,
) -> Result<crate::worker::PullOutcome, String> {
    let _ = engine.apply(Command::Pull {
        repo_path: checkout.to_path_buf(),
        target: PullTarget::Session,
        busy_message: crate::status_text::StatusText::from("Pulling"),
        already_running_message: crate::status_text::StatusText::from("already"),
    });
    pull_result(engine)
}

/// A refused pull moves nothing at all, and says which folder, what lives
/// in it, and that the pull was not done.
#[test]
fn a_refused_pull_moves_nothing_and_names_the_folder_and_its_occupant() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = project_checkout(tmp.path(), &mut engine);
    let other = repo.join("vendor").join("lib");
    own_repository(&other);
    engine
        .projects
        .push(sample_project("p2", other.to_str().unwrap()));
    std::fs::write(upstream.join("vendor"), "a file\n").unwrap();
    git(&upstream, &["add", "-f", "vendor"]);
    git(&upstream, &["commit", "-m", "vendor is a file now"]);
    let before = head(&repo);

    let error = session_pull(&mut engine, &repo).expect_err("the pull is refused");

    assert_eq!(head(&repo), before, "the checkout did not move");
    assert!(other.join("notes.txt").exists());
    assert!(
        error.contains("vendor")
            && error.contains("project \"p2-name\"")
            && error.contains("did not pull"),
        "the refusal names the folder, its occupant and that nothing was pulled: {error}"
    );
}

/// The check is about the folders git would remove, not every folder the
/// incoming commit writes into: a new file inside an ignored folder that
/// another project lives under removes nothing, and the pull goes ahead.
#[test]
fn a_pull_that_only_writes_inside_an_occupied_folder_goes_ahead() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = project_checkout(tmp.path(), &mut engine);
    let other = repo.join("vendor").join("lib");
    own_repository(&other);
    engine
        .projects
        .push(sample_project("p2", other.to_str().unwrap()));
    std::fs::create_dir_all(upstream.join("vendor")).unwrap();
    std::fs::write(upstream.join("vendor").join("README"), "kept\n").unwrap();
    git(&upstream, &["add", "-f", "vendor/README"]);
    git(&upstream, &["commit", "-m", "a readme in vendor"]);

    session_pull(&mut engine, &repo).expect("the pull goes ahead");

    assert_eq!(head(&repo), head(&upstream), "the checkout fast-forwarded");
    assert!(repo.join("vendor").join("README").is_file());
    assert!(other.join(".git").exists());
}

/// A folder dux knows nothing about is git's to remove, as before (the docs
/// say so): only what dux must never remove is protected.
#[test]
fn a_pull_still_replaces_an_ignored_folder_nothing_lives_in() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = project_checkout(tmp.path(), &mut engine);
    std::fs::create_dir_all(repo.join("vendor")).unwrap();
    std::fs::write(repo.join("vendor").join("cache"), "throwaway\n").unwrap();
    std::fs::write(upstream.join("vendor"), "a file\n").unwrap();
    git(&upstream, &["add", "-f", "vendor"]);
    git(&upstream, &["commit", "-m", "vendor is a file now"]);

    session_pull(&mut engine, &repo).expect("the pull goes ahead");

    assert!(repo.join("vendor").is_file());
}

/// A terminal dux runs whose shell stands in the folder is an occupant too.
#[test]
fn a_pull_never_deletes_a_folder_a_process_dux_started_is_working_in() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = project_checkout(tmp.path(), &mut engine);
    let folder = repo.join("vendor");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(upstream.join("vendor"), "a file\n").unwrap();
    git(&upstream, &["add", "-f", "vendor"]);
    git(&upstream, &["commit", "-m", "vendor is a file now"]);
    // A process dux registered, started in the checkout, standing in the
    // ignored folder as its current folder.
    let mut sleeper = std::process::Command::new("sh");
    sleeper.args(["-c", "sleep 30"]).current_dir(&folder);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        use std::os::unix::process::CommandExt;
        sleeper.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let sleeper = Kill(sleeper.spawn().expect("spawn"));
    let session = crate::process_sessions::ProcessSession::started_now(sleeper.0.id());
    engine
        .process_registry
        .register("a-terminal", session, &repo);

    let error = session_pull(&mut engine, &repo).expect_err("the pull is refused");

    assert!(folder.is_dir(), "the folder the process works in is kept");
    assert!(error.contains("working in it"), "{error}");
}

/// A branch switch asks the same question before git moves the tree.
#[test]
fn a_base_branch_switch_never_deletes_another_projects_repository() {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init(&repo);
    std::fs::write(repo.join(".gitignore"), "vendor/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "ignore vendor"]);
    git(&repo, &["switch", "-q", "-c", "other"]);
    std::fs::write(repo.join("vendor"), "a file\n").unwrap();
    git(&repo, &["add", "-f", "vendor"]);
    git(&repo, &["commit", "-m", "vendor is a file on other"]);
    git(&repo, &["switch", "-q", "main"]);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let other = repo.join("vendor").join("lib");
    own_repository(&other);
    engine
        .projects
        .push(sample_project("p2", other.to_str().unwrap()));

    let _ = engine.change_project_base_branch("p1", "other").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let result = loop {
        assert!(Instant::now() < deadline, "the switch never completed");
        if let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) {
            let result = match &event {
                WorkerEvent::ProjectBaseBranchChanged { result, .. } => Some(result.clone()),
                _ => None,
            };
            let _ = engine.process_worker_event(event);
            if let Some(result) = result {
                break result;
            }
        }
    };

    let Err(crate::base_branch::BaseBranchChangeFailure::SwitchFailed(reason)) = result else {
        panic!("the switch is refused: {result:?}");
    };
    assert!(other.join(".git").exists());
    assert_eq!(
        crate::git::current_branch(&repo).unwrap(),
        "main",
        "the checkout stayed where it was"
    );
    assert!(reason.contains("project \"p2-name\""), "{reason}");
}
