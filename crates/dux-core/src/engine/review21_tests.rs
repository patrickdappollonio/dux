//! Review 21: reproductions.

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

fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "--initial-branch=main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
}

/// A project checkout `repo` on `main` ignoring `scratch/`, and a local branch
/// `feat` whose commit tracks a FILE at `scratch/notes.txt`.
fn fixture(tmp: &Path) -> std::path::PathBuf {
    let repo = tmp.join("repo");
    init(&repo);
    std::fs::write(repo.join(".gitignore"), "scratch/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    std::fs::create_dir_all(repo.join("scratch")).unwrap();
    std::fs::write(repo.join("scratch/notes.txt"), "from the branch\n").unwrap();
    git(&repo, &["add", "-f", "scratch/notes.txt"]);
    git(&repo, &["commit", "-q", "-m", "track scratch/notes.txt"]);
    git(&repo, &["switch", "-q", "main"]);
    assert!(!repo.join("scratch").exists());
    repo
}

/// A standalone agent works in `repo/scratch` (ignored by the project). A
/// branch switch whose commit tracks `scratch/notes.txt` makes git OVERWRITE
/// the agent's own ignored `notes.txt` inside the folder: the move check only
/// asks about folders git would remove, never about files it would replace
/// inside a folder something lives in.
#[test]
fn review21_switch_overwrites_a_file_inside_a_standalone_agents_folder() {
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let folder = repo.join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-scratch",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "irreplaceable\n",
        "the switch replaced the standalone agent's file (result {result:?})"
    );
}

/// Another project's repository lives at `repo/scratch` with uncommitted
/// work in `notes.txt`. A switch whose commit tracks `scratch/notes.txt`
/// makes git overwrite that file, and the other project's uncommitted work is
/// gone.
#[test]
fn review21_switch_overwrites_uncommitted_work_in_a_nested_project() {
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let other = repo.join("scratch");
    init(&other);
    std::fs::write(other.join("notes.txt"), "committed\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "mine"]);
    std::fs::write(other.join("notes.txt"), "uncommitted work\n").unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .projects
        .push(sample_project("other", other.to_str().unwrap()));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(other.join("notes.txt")).unwrap(),
        "uncommitted work\n",
        "the switch replaced the other project's uncommitted file (result {result:?})"
    );
}

/// Switching a project to a branch only origin has, when git itself refuses
/// the switch (a local change to a file the branch changes), leaves behind
/// the local branch dux created for it: only a refusal by the move check
/// removes it.
#[test]
fn review21_switch_git_refuses_leaves_the_branch_it_created() {
    let (mut engine, tmp) = test_engine();
    let upstream = tmp.path().join("upstream");
    init(&upstream);
    std::fs::write(upstream.join("x"), "x\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "base"]);
    git(&upstream, &["switch", "-q", "-c", "feat"]);
    std::fs::write(upstream.join("x"), "feat\n").unwrap();
    git(&upstream, &["commit", "-q", "-am", "feat changes x"]);
    git(&upstream, &["switch", "-q", "main"]);
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
    std::fs::write(repo.join("x"), "local edit\n").unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::base_branch::switch_to_base_branch(&r, "feat", &guard, &Default::default())
            .map(|_| ())
    })
    .join()
    .unwrap();
    assert!(result.is_err(), "git refused the switch: {result:?}");
    let left = crate::checkout_move::commit_id(&repo, "refs/heads/feat").unwrap();
    assert!(
        left.is_none(),
        "the refused switch left the local branch feat it created at {left:?}"
    );
}

// ---- pulls, and what does not count ----

/// An upstream on `main` ignoring `scratch/`, a clone of it registered as
/// project p1, and a new upstream commit that tracks `scratch/notes.txt`.
fn pull_fixture(
    tmp: &Path,
    engine: &mut crate::engine::Engine,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let upstream = tmp.join("upstream");
    init(&upstream);
    std::fs::write(upstream.join(".gitignore"), "scratch/\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "base"]);
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
    std::fs::create_dir_all(upstream.join("scratch")).unwrap();
    std::fs::write(upstream.join("scratch/notes.txt"), "from upstream\n").unwrap();
    git(&upstream, &["add", "-f", "scratch/notes.txt"]);
    git(
        &upstream,
        &["commit", "-q", "-m", "track scratch/notes.txt"],
    );
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
fn a_pull_never_overwrites_a_file_inside_a_standalone_agents_folder() {
    let (mut engine, tmp) = test_engine();
    let (_upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    let folder = repo.join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-scratch",
            folder.to_str().unwrap(),
        ));
    let before = crate::git::head_commit(&repo).unwrap();

    let error = guarded_pull(&engine, &repo).expect_err("the pull is refused");

    assert_eq!(
        std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "irreplaceable\n"
    );
    assert_eq!(crate::git::head_commit(&repo).unwrap(), before);
    assert!(
        error.contains("standalone agent") && error.contains("overwrite"),
        "{error}"
    );
}

#[test]
fn a_pull_never_overwrites_uncommitted_work_in_a_nested_project() {
    let (mut engine, tmp) = test_engine();
    let (_upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    let other = repo.join("scratch");
    init(&other);
    std::fs::write(other.join("notes.txt"), "committed\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "mine"]);
    std::fs::write(other.join("notes.txt"), "uncommitted work\n").unwrap();
    engine
        .projects
        .push(sample_project("other", other.to_str().unwrap()));

    let error = guarded_pull(&engine, &repo).expect_err("the pull is refused");

    assert_eq!(
        std::fs::read_to_string(other.join("notes.txt")).unwrap(),
        "uncommitted work\n"
    );
    assert!(error.contains("project \"other-name\""), "{error}");
}

/// A file standing where the incoming commit needs a folder, inside a
/// standalone agent's folder, is just as much the agent's.
#[test]
fn a_pull_never_replaces_a_standalone_agents_file_with_a_folder() {
    let (mut engine, tmp) = test_engine();
    let upstream = tmp.path().join("upstream");
    init(&upstream);
    std::fs::write(upstream.join(".gitignore"), "scratch/\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "base"]);
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
    std::fs::create_dir_all(upstream.join("scratch/data")).unwrap();
    std::fs::write(upstream.join("scratch/data/x"), "x\n").unwrap();
    git(&upstream, &["add", "-f", "scratch/data/x"]);
    git(
        &upstream,
        &["commit", "-q", "-m", "a folder at scratch/data"],
    );
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let folder = repo.join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("data"), "the agent's own file\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-scratch",
            folder.to_str().unwrap(),
        ));

    let error = guarded_pull(&engine, &repo).expect_err("the pull is refused");

    assert_eq!(
        std::fs::read_to_string(folder.join("data")).unwrap(),
        "the agent's own file\n"
    );
    assert!(error.contains("needs a folder"), "{error}");
}

/// The checkout's own project (and agent) contain every path in it and do not
/// count: an ignored file nothing else lives around is overwritten as before.
#[test]
fn a_pull_still_overwrites_an_ignored_file_only_the_checkout_itself_holds() {
    let (mut engine, tmp) = test_engine();
    let (upstream, repo) = pull_fixture(tmp.path(), &mut engine);
    std::fs::create_dir_all(repo.join("scratch")).unwrap();
    std::fs::write(repo.join("scratch/notes.txt"), "throwaway\n").unwrap();
    engine
        .sessions
        .push(crate::engine::test_support::sample_session(
            "s-own", "p1", "main",
        ));
    if let Some(managed) = engine
        .sessions
        .last_mut()
        .and_then(|session| session.workspace.as_managed_mut())
    {
        managed.worktree_path = repo.to_string_lossy().into_owned();
    }

    guarded_pull(&engine, &repo).expect("the pull goes ahead");

    assert_eq!(
        crate::git::head_commit(&repo).unwrap(),
        crate::git::head_commit(&upstream).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("scratch/notes.txt")).unwrap(),
        "from upstream\n"
    );
}
