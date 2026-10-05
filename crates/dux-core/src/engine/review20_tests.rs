//! Review 20: reproductions.

use std::path::Path;

use crate::engine::test_support::{sample_project, test_engine};

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::fixture_git()
        .args(["-c", "protocol.file.allow=always"])
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

/// A project checkout `repo` with an initialized submodule `sub` on `main`,
/// a local branch `nosub` without it, and a local branch `bumped` that moves
/// the submodule to a commit tracking a FILE at `scratch`. The checkout has
/// `submodule.recurse = true`, a setting git documents and users commonly set
/// globally.
fn fixture(tmp: &Path) -> std::path::PathBuf {
    let subup = tmp.join("subup");
    init(&subup);
    std::fs::write(subup.join("a"), "a\n").unwrap();
    git(&subup, &["add", "."]);
    git(&subup, &["commit", "-q", "-m", "s"]);
    git(&subup, &["switch", "-q", "-c", "v2"]);
    std::fs::write(subup.join("scratch"), "f\n").unwrap();
    git(&subup, &["add", "."]);
    git(&subup, &["commit", "-q", "-m", "v2"]);
    git(&subup, &["switch", "-q", "main"]);

    let repo = tmp.join("repo");
    init(&repo);
    std::fs::write(repo.join("x"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["branch", "nosub"]);
    git(
        &repo,
        &["submodule", "add", "-q", subup.to_str().unwrap(), "sub"],
    );
    git(&repo, &["commit", "-q", "-m", "add sub"]);
    git(&repo, &["switch", "-q", "-c", "bumped"]);
    git(&repo.join("sub"), &["checkout", "-q", "v2"]);
    git(&repo, &["add", "sub"]);
    git(&repo, &["commit", "-q", "-m", "bump"]);
    git(&repo, &["switch", "-q", "main"]);
    git(&repo, &["submodule", "update", "-q"]);
    git(&repo, &["config", "submodule.recurse", "true"]);
    repo
}

/// A standalone agent works in `repo/sub/scratch`, a folder the submodule
/// ignores. Switching the project to `bumped` moves the submodule (git
/// recurses because of `submodule.recurse`), and the submodule's new commit
/// tracks a file at `scratch`, so git deletes the agent's folder. The move
/// check never looks inside a submodule, so dux lets it happen.
#[test]
fn review20_recursing_switch_deletes_a_standalone_folder_inside_a_submodule() {
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let folder = repo.join("sub").join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    let exclude = repo.join(".git/modules/sub/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "scratch\n").unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-inner",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "bumped", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(
        folder.join("notes.txt").is_file(),
        "the standalone agent's folder {} was deleted by the switch (result {result:?})",
        folder.display()
    );
}

/// A standalone agent works in the submodule `repo/sub` itself (a repository
/// of its own). Switching the project to `nosub`, where the submodule does
/// not exist, makes git (recursing) delete the submodule's working tree, the
/// agent's whole folder with it. The move check skips deleted entries.
#[test]
fn review20_recursing_switch_deletes_a_standalone_folder_that_is_a_submodule() {
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    let folder = repo.join("sub");
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-sub",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "nosub", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(
        folder.join("a").is_file() && folder.join(".git").exists(),
        "the standalone agent's folder {} was emptied or removed by the switch (exists: {}, result {result:?})",
        folder.display(),
        folder.exists()
    );
}

/// Changing a project's base branch to one only origin has, when the guard
/// refuses the switch (the branch tracks a file where a standalone agent's
/// ignored folder stands), leaves behind the local branch dux created for it.
/// `switch_to_base_branch` creates the tracking branch itself before calling
/// `switch_branch`, which then sees an existing local branch and never removes
/// it on refusal.
#[test]
fn review20_refused_base_branch_switch_leaves_the_branch_it_created() {
    let (mut engine, tmp) = test_engine();
    let upstream = tmp.path().join("upstream");
    init(&upstream);
    std::fs::write(upstream.join("x"), "x\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "base"]);
    git(&upstream, &["switch", "-q", "-c", "feat"]);
    std::fs::write(upstream.join("scratch"), "a file\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "scratch is a file"]);
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
    std::fs::create_dir_all(repo.join(".git/info")).unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "scratch\n").unwrap();
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
        crate::base_branch::switch_to_base_branch(&r, "feat", &guard, &[]).map(|_| ())
    })
    .join()
    .unwrap();
    assert!(result.is_err(), "the guard refused the switch: {result:?}");
    assert!(folder.join("notes.txt").is_file());
    let left = crate::checkout_move::commit_id(&repo, "refs/heads/feat").unwrap();
    assert!(
        left.is_none(),
        "the refused base-branch switch left the local branch feat it created at {left:?}"
    );
}

// ---- `submodule.recurse = true` in the user's GLOBAL config ----
//
// Each scenario runs in a child test process whose HOME and global git config
// are a scratch folder holding `submodule.recurse = true`, so the setting is
// the user's own and reaches every git dux runs, without touching the parent
// process or the machine's real config.

const GLOBAL_DIR: &str = "DUX_REVIEW20_GLOBAL_DIR";

/// Run the ignored test `helper` in a child process with a global git config
/// that sets `submodule.recurse = true`, and fail if it fails.
fn run_with_global_recurse(helper: &str) {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("gitconfig");
    std::fs::write(
        &global,
        "[user]\n\tname = t\n\temail = t@example.com\n\
         [protocol \"file\"]\n\tallow = always\n\
         [submodule]\n\trecurse = true\n",
    )
    .unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("engine::review20_tests::{helper}"),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("GIT_CONFIG_GLOBAL", &global)
        .env(GLOBAL_DIR, dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{helper} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Whether this process is the child [`run_with_global_recurse`] started.
fn in_child() -> bool {
    std::env::var_os(GLOBAL_DIR).is_some()
}

#[test]
fn a_switch_never_recurses_into_submodules_under_a_global_setting() {
    run_with_global_recurse("global_recurse_switch");
}

#[test]
#[ignore]
fn global_recurse_switch() {
    if !in_child() {
        return;
    }
    let (mut engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    git(&repo, &["config", "--unset", "submodule.recurse"]);
    let folder = repo.join("sub").join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    let exclude = repo.join(".git/modules/sub/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "scratch\n").unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-inner",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "bumped", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(crate::git::current_branch(&repo).unwrap(), "bumped");
    assert!(
        folder.join("notes.txt").is_file(),
        "the folder inside the submodule is kept: the submodule did not move"
    );
}

#[test]
fn a_pull_never_recurses_into_submodules_under_a_global_setting() {
    run_with_global_recurse("global_recurse_pull");
}

#[test]
#[ignore]
fn global_recurse_pull() {
    if !in_child() {
        return;
    }
    let (mut engine, tmp) = test_engine();
    let upstream = fixture(tmp.path());
    git(&upstream, &["config", "--unset", "submodule.recurse"]);
    let repo = tmp.path().join("clone");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["submodule", "update", "-q", "--init"]);
    let folder = repo.join("sub").join("scratch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "irreplaceable\n").unwrap();
    let exclude = repo.join(".git/modules/sub/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "scratch\n").unwrap();
    // Upstream's main moves the submodule to the commit with a file at
    // `scratch`.
    git(&upstream, &["merge", "-q", "--ff-only", "bumped"]);
    let before_sub = crate::git::head_commit(&repo.join("sub")).unwrap();
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .sessions
        .push(crate::engine::test_support::sample_standalone_session(
            "s-inner",
            folder.to_str().unwrap(),
        ));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::pull_current_branch(&r, &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        crate::git::head_commit(&repo).unwrap(),
        crate::git::head_commit(&upstream).unwrap(),
        "the checkout fast-forwarded"
    );
    assert_eq!(
        crate::git::head_commit(&repo.join("sub")).unwrap(),
        before_sub,
        "the submodule's own checkout did not move"
    );
    assert!(folder.join("notes.txt").is_file());
}

#[test]
fn a_worktree_add_never_recurses_into_submodules_under_a_global_setting() {
    run_with_global_recurse("global_recurse_worktree_add");
}

#[test]
#[ignore]
fn global_recurse_worktree_add() {
    if !in_child() {
        return;
    }
    let (_engine, tmp) = test_engine();
    let repo = fixture(tmp.path());
    git(&repo, &["config", "--unset", "submodule.recurse"]);
    let fresh = tmp.path().join("fresh");
    crate::git::add_worktree_new_branch_at(&repo, &fresh, "fresh-branch", None).unwrap();
    let existing = tmp.path().join("existing");
    crate::git::add_worktree_existing_branch_at(&repo, &existing, "bumped").unwrap();
    for worktree in [&fresh, &existing] {
        let populated = std::fs::read_dir(worktree.join("sub"))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(
            !populated,
            "the new worktree's submodule was left unpopulated: {}",
            worktree.display()
        );
    }
}
