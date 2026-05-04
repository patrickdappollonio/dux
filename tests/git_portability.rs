//! audit02 Phase 21 (P2-11) — git wrapper portability tests.
//!
//! Verifies that `src/git.rs` invokes git with `&Path`/`&OsStr` arguments
//! so non-UTF-8 worktree paths survive the round-trip. This is not
//! theoretical: Linux ext4 allows arbitrary byte sequences in filenames
//! (anything except `/` and `\0`), and `to_string_lossy()` would silently
//! replace any such byte with U+FFFD before git ever saw the path —
//! making `git -C` look up the wrong directory and fail.
//!
//! Gated to `#[cfg(target_os = "linux")]` rather than `#[cfg(unix)]`:
//! macOS APFS rejects non-UTF-8 filenames at the VFS layer with
//! `EILSEQ` (errno 92), so `std::fs::create_dir` on a non-UTF-8 path
//! aborts the test before it can exercise the wrapper. Only Linux
//! filesystems (ext4, xfs, btrfs) accept arbitrary byte sequences.

#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

/// `git init` inside a worktree whose path contains non-UTF-8 bytes.
///
/// The byte sequence `0xff 0xfe` is a deliberately invalid UTF-8 lead —
/// `to_string_lossy()` collapses both bytes to a single U+FFFD char,
/// changing the path's identity. With the Phase 21 fix, `is_git_repo`
/// passes the `Path` straight through `Command::arg` (which accepts
/// `AsRef<OsStr>`), so the kernel sees the exact bytes that exist on
/// disk.
#[test]
fn git_handles_non_utf8_worktree_path() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let bad_name = OsString::from_vec(vec![0xff, 0xfe, b'd', b'i', b'r']);
    let bad = tmp.path().join(bad_name);
    std::fs::create_dir(&bad).expect("create non-UTF-8 dir");

    // Skip if `git` isn't on PATH (CI sandboxes that strip git would
    // otherwise hard-fail; macOS runners install it as part of the
    // Xcode CLT shipped with the runner image, so this is mainly a
    // safeguard for unusual local environments).
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not on PATH — skipping git_handles_non_utf8_worktree_path");
        return;
    }

    // Initialise an empty repo inside the non-UTF-8 directory.
    let init = Command::new("git")
        .arg("init")
        .current_dir(&bad)
        .output()
        .expect("git init runs");
    assert!(
        init.status.success(),
        "git init failed in non-UTF-8 dir: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    // The wrapper must report `true` for the directory we just `git
    // init`-ed. If `is_git_repo` had used `to_string_lossy()`, git
    // would have been pointed at a U+FFFD-replacement path that does
    // not exist, returning `false`.
    assert!(
        dux::git::is_git_repo(&bad),
        "is_git_repo() must recognise repo at non-UTF-8 path"
    );

    // A sibling directory that is *not* a git repo must still report
    // false — guards against the wrapper accidentally short-circuiting
    // the path check on non-UTF-8 inputs.
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert!(
        !dux::git::is_git_repo(&plain),
        "non-repo directory must not be reported as a git repo"
    );
}

/// Round-trip a worktree creation through `create_worktree_existing_branch`
/// when the *parent* directory has a non-UTF-8 component. This exercises
/// the heavier wrapper path (`git worktree add`) where two `&Path` args
/// flow into the same command.
#[test]
fn create_worktree_existing_branch_handles_non_utf8_parent() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let bad_name = OsString::from_vec(vec![0xc3, 0x28, b'p', b'r', b'o', b'j']);
    let project_root = tmp.path().join(&bad_name);
    std::fs::create_dir(&project_root).expect("create non-UTF-8 project root");

    if Command::new("git").arg("--version").output().is_err() {
        eprintln!(
            "git not on PATH — skipping create_worktree_existing_branch_handles_non_utf8_parent"
        );
        return;
    }

    // Bootstrap a real repo inside the non-UTF-8 directory with one
    // commit on `main` so a worktree can be carved off.
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&project_root)
            .output()
            .expect("git ran");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            project_root.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "test"]);
    run(&["config", "user.email", "t@t"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
    // Create a second branch so the worktree can check it out without
    // conflicting with `main` (which is already checked out at
    // `project_root`).
    run(&["branch", "feature"]);

    // Create a worktree under a *separate* non-UTF-8 worktrees root —
    // doubles the surface area that must round-trip cleanly.
    let worktrees_root = tmp.path().join(OsString::from_vec(vec![0xff, b'w', b't']));
    let result = dux::git::create_worktree_existing_branch(
        &project_root,
        &worktrees_root,
        "demo",
        "feature",
    );
    assert!(
        result.is_ok(),
        "create_worktree_existing_branch should succeed with non-UTF-8 paths: {result:?}"
    );

    let (branch, worktree) = result.unwrap();
    assert_eq!(branch, "feature");
    assert!(
        worktree.exists(),
        "worktree directory must exist on disk at {}",
        worktree.display()
    );
}
