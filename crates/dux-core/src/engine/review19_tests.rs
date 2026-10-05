//! Review 19: reproductions.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, test_engine};
use crate::engine::{Command, Engine};
use crate::worker::{PullTarget, WorkerEvent};
use std::fs;

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

fn head(dir: &Path) -> String {
    crate::git::head_commit(dir).unwrap()
}

fn pull(engine: &mut Engine, checkout: &Path) -> Result<crate::worker::PullOutcome, String> {
    let _ = engine.apply(Command::Pull {
        repo_path: checkout.to_path_buf(),
        target: PullTarget::Session,
        busy_message: crate::status_text::StatusText::from("Pulling"),
        already_running_message: crate::status_text::StatusText::from("already"),
    });
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

/// Upstream's `.gitmodules` declares `alone` with `ignore = all` (a common
/// setting for a submodule whose dirty state should not show). The checkout
/// has an ignored link `alone` pointing at a standalone agent's folder, and
/// the agent is recorded at the link's path, exactly the review-18 layout.
fn checkout(tmp: &Path, engine: &mut Engine) -> (PathBuf, PathBuf) {
    let upstream = tmp.join("upstream");
    init(&upstream);
    std::fs::write(
        upstream.join(".gitmodules"),
        "[submodule \"alone\"]\n\tpath = alone\n\turl = ./alone\n\tignore = all\n",
    )
    .unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-m", "declare the submodule"]);
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
    std::fs::create_dir_all(repo.join(".git").join("info")).unwrap();
    std::fs::write(repo.join(".git").join("info").join("exclude"), "alone\n").unwrap();
    let outside = tmp.join("outside");
    init(&outside);
    std::fs::write(outside.join("notes.txt"), "irreplaceable\n").unwrap();
    git(&outside, &["add", "."]);
    git(&outside, &["commit", "-m", "mine"]);
    std::os::unix::fs::symlink(&outside, repo.join("alone")).unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            repo.join("alone").to_str().unwrap(),
        ));
    (upstream, repo)
}

/// A local branch `feat` whose commit adds a gitlink at `alone`, checked out
/// with a guarded switch from `main`, where `alone` is an ignored link to a
/// standalone agent's folder and is recorded as that agent's folder.
fn switch_over_link(ignore_all: bool) -> (Result<(), String>, std::fs::FileType) {
    let (mut engine, tmp) = test_engine();
    let repo = tmp.path().join("repo");
    init(&repo);
    let gitmodules = if ignore_all {
        "[submodule \"alone\"]\n\tpath = alone\n\turl = ./alone\n\tignore = all\n"
    } else {
        "[submodule \"alone\"]\n\tpath = alone\n\turl = ./alone\n"
    };
    std::fs::write(repo.join(".gitmodules"), gitmodules).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "declare the submodule"]);
    let id = head(&repo);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{id},alone"),
        ],
    );
    git(&repo, &["commit", "-m", "alone is a submodule"]);
    git(&repo, &["switch", "-q", "main"]);
    std::fs::create_dir_all(repo.join(".git").join("info")).unwrap();
    std::fs::write(repo.join(".git").join("info").join("exclude"), "alone\n").unwrap();
    let outside = tmp.path().join("outside");
    init(&outside);
    std::fs::write(outside.join("notes.txt"), "irreplaceable\n").unwrap();
    git(&outside, &["add", "."]);
    git(&outside, &["commit", "-m", "mine"]);
    std::os::unix::fs::symlink(&outside, repo.join("alone")).unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-alone",
            repo.join("alone").to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    let kind = std::fs::symlink_metadata(repo.join("alone"))
        .unwrap()
        .file_type();
    (result, kind)
}

/// Control: the guard refuses the switch and the link stays (review 18's fix).
#[test]
fn review19_control_a_guarded_switch_keeps_a_linked_standalone_folder() {
    let (result, kind) = switch_over_link(false);
    assert!(result.is_err(), "{result:?}");
    assert!(kind.is_symlink());
}

/// With `ignore = all` for that path in the checkout's `.gitmodules`,
/// `git diff-tree` leaves the gitlink out, the move check asks about nothing,
/// and `git switch` replaces the standalone agent's folder link with an
/// empty directory.
#[test]
fn review19_ignore_all_hides_a_submodule_over_a_linked_standalone_folder() {
    let (result, kind) = switch_over_link(true);
    assert!(
        kind.is_symlink(),
        "the standalone agent's folder link was replaced by {kind:?} (switch result {result:?})"
    );
}

/// The same for a pull: upstream's `.gitmodules` marks `alone` with
/// `ignore = all` and a later commit adds the gitlink there, over the
/// checkout's link to a standalone agent's folder. The pull is refused and
/// the link stays.
#[test]
fn ignore_all_hides_nothing_from_a_guarded_pull() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = checkout(tmp.path(), &mut engine);
    let id = head(&upstream);
    git(
        &upstream,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{id},alone"),
        ],
    );
    git(&upstream, &["commit", "-m", "alone is a submodule"]);
    let before = head(&repo);

    let result = pull(&mut engine, &repo);

    let kind = fs::symlink_metadata(repo.join("alone"))
        .unwrap()
        .file_type();
    assert!(
        kind.is_symlink(),
        "the link was replaced by {kind:?} ({result:?})"
    );
    assert!(result.is_err(), "{result:?}");
    assert_eq!(head(&repo), before);
}
