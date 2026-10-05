//! Review 22: reproductions.

use std::path::Path;

use crate::engine::test_support::{sample_project, test_engine};

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

/// A project checkout on `main` tracking `docs/guide.md`, and a local branch
/// `feat` that deletes `docs/`.
fn fixture(tmp: &Path) -> std::path::PathBuf {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README"), "r\n").unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/guide.md"), "guide\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    git(&repo, &["rm", "-q", "-r", "docs"]);
    git(&repo, &["commit", "-q", "-m", "drop docs"]);
    git(&repo, &["switch", "-q", "main"]);
    assert!(repo.join("docs/guide.md").exists());
    repo
}

/// A standalone agent works in `repo/docs`, a tracked folder of the project.
/// Switching the project to a branch that deletes `docs/` makes git delete the
/// tracked files and then the now-empty folder itself: the move check skips
/// every deleted path (`--diff-filter=d`), so the standalone agent's folder is
/// removed from under it.
#[test]
fn review22_switch_removes_a_standalone_agents_tracked_folder() {
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let folder = repo.join("docs");
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-docs",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(
        folder.is_dir(),
        "the switch removed the standalone agent's folder {} (result {result:?})",
        folder.display()
    );
}

/// A process dux started (a terminal's shell, recorded in the registry) works
/// in `repo/docs`. The switch deletes the folder it is working in.
#[test]
fn review22_switch_removes_the_folder_a_dux_process_works_in() {
    use std::os::unix::process::CommandExt;
    let (engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let folder = repo.join("docs");
    let mut command = std::process::Command::new("sleep");
    command.arg("60").current_dir(&folder);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
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
    let job = Kill(command.spawn().unwrap());
    let session = crate::process_sessions::ProcessSession::started_now(job.0.id());
    engine
        .process_registry
        .register(crate::process_sessions::UNOWNED_PTYS, session, &folder);
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(
        folder.is_dir(),
        "the switch removed {} while a process dux started works in it (result {result:?})",
        folder.display()
    );
    drop(job);
}

// ---- pulls, and what stays ----

/// An upstream tracking `docs/guide.md`, a clone registered as project p1,
/// and a new upstream commit that deletes `docs/`.
fn pull_fixture(
    tmp: &Path,
    engine: &mut crate::engine::Engine,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let upstream = fixture(tmp);
    git(&upstream, &["merge", "-q", "--ff-only", "feat"]);
    git(&upstream, &["reset", "-q", "--hard", "HEAD~1"]);
    let repo = tmp.join("clone");
    git(
        tmp,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&upstream, &["merge", "-q", "--ff-only", "feat"]);
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    (upstream, repo)
}

fn guarded_pull(engine: &crate::engine::Engine, repo: &Path) -> Result<(), String> {
    let guard = engine.checkout_move_guard();
    let r = repo.to_path_buf();
    std::thread::spawn(move || {
        crate::git::pull_current_branch(&r, &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap()
}

#[test]
fn a_pull_never_removes_a_standalone_agents_tracked_folder() {
    let (mut engine, tmp) = test_engine();
    let (_upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    let folder = repo.join("docs");
    assert!(folder.join("guide.md").exists());
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-docs",
            folder.to_str().unwrap(),
        ));
    let before = crate::git::head_commit(&repo).unwrap();

    let error = guarded_pull(&engine, &repo).expect_err("the pull is refused");

    assert!(folder.join("guide.md").is_file());
    assert_eq!(crate::git::head_commit(&repo).unwrap(), before);
    assert!(
        error.contains("deletes everything in docs") && error.contains("standalone agent"),
        "{error}"
    );
}

#[test]
fn a_pull_never_removes_the_folder_a_session_works_in() {
    use std::os::unix::process::CommandExt;
    let (mut engine, tmp) = test_engine();
    let (_upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    let folder = repo.join("docs");
    let mut command = std::process::Command::new("sleep");
    command.arg("60").current_dir(&folder);
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
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
    let job = Kill(command.spawn().unwrap());
    // Started at the checkout's root, as an agent's own terminal is, and
    // `cd`'d into the folder since.
    let session = crate::process_sessions::ProcessSession::started_now(job.0.id());
    engine
        .process_registry
        .register("p1-terminal", session, &repo);

    let error = guarded_pull(&engine, &repo).expect_err("the pull is refused");

    assert!(folder.is_dir());
    assert!(error.contains("working in it"), "{error}");
    drop(job);
}

/// A folder that keeps something the move does not delete (an ignored file)
/// is not removed, so nothing about it is asked and the pull goes ahead.
#[test]
fn a_pull_that_leaves_a_folder_non_empty_goes_ahead() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    let folder = repo.join("docs");
    std::fs::write(folder.join("notes.local"), "mine\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-docs",
            folder.to_str().unwrap(),
        ));

    guarded_pull(&engine, &repo).expect("the pull goes ahead");

    assert_eq!(
        crate::git::head_commit(&repo).unwrap(),
        crate::git::head_commit(&upstream).unwrap()
    );
    assert!(folder.join("notes.local").is_file());
    assert!(!folder.join("guide.md").exists());
}
