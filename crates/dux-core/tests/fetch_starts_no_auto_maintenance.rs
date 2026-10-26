//! A fetch dux runs before `git worktree add` must not leave git's background
//! auto-maintenance running, because since git 2.54 that maintenance prunes
//! worktrees and can delete the one being created. This is its own test binary
//! because it traces git through process environment variables, which every
//! git the process starts inherits.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_pull_request_fetch_starts_no_auto_maintenance_even_where_the_repository_wants_it() {
    let tmp = tempfile::tempdir().unwrap();
    let trace = tmp.path().join("trace2.json");
    // SAFETY: this binary holds this one test, so no other thread reads the
    // environment while it changes.
    unsafe { std::env::set_var("GIT_TRACE2_EVENT", &trace) };
    // SAFETY: as above.
    unsafe { std::env::set_var("GIT_CONFIG_NOSYSTEM", "1") };
    // SAFETY: as above.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null") };

    let origin = tmp.path().join("origin");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    git(
        &origin,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "pr",
        ],
    );
    git(&origin, &["update-ref", "refs/pull/7/head", "HEAD"]);
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    // The repository asks for auto-maintenance, so only dux's own invocation
    // can turn it off.
    git(&repo, &["config", "maintenance.auto", "true"]);
    std::fs::write(&trace, "").unwrap();

    dux_core::git::fetch_pull_request_head(&repo, 7, "pr-7").unwrap();

    let events = std::fs::read_to_string(&trace).unwrap();
    assert!(
        events.contains(r#""child_start""#),
        "the trace recorded no child processes at all, so it proves nothing: {events}"
    );
    assert!(
        !events.contains(r#""maintenance","run""#),
        "the fetch started git maintenance: {events}"
    );
}
