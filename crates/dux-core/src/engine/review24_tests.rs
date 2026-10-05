//! Review 24: reproductions.

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

/// A project checkout tracks a link `work` pointing at ANOTHER project's
/// repository. A branch deletes the link. Removing the link leaves the other
/// project's repository where it is, so the switch must go through; the move
/// check judges the link's target instead (the containment test's canonical
/// spelling follows the link) and refuses the switch.
#[test]
fn review24_switch_deleting_a_tracked_link_to_another_project_is_refused() {
    let (mut engine, tmp) = test_engine();
    let tmp = tmp.path();
    let shared = tmp.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("notes.md"), "mine\n").unwrap();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README"), "r\n").unwrap();
    std::os::unix::fs::symlink(&shared, repo.join("work")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    git(&repo, &["rm", "-q", "work"]);
    git(&repo, &["commit", "-q", "-m", "drop the link"]);
    git(&repo, &["switch", "-q", "main"]);

    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    engine
        .projects
        .push(sample_project("p2", shared.to_str().unwrap()));
    let guard = engine.checkout_move_guard();
    let r = repo.clone();
    let result = std::thread::spawn(move || {
        crate::git::switch_branch(&r, "feat", &guard).map_err(|e| format!("{e:#}"))
    })
    .join()
    .unwrap();
    assert!(
        result.is_ok(),
        "a switch that only deletes a tracked link (its target, another project's \
         repository, stays) was refused: {result:?}"
    );
    assert!(shared.join("notes.md").exists());
}
