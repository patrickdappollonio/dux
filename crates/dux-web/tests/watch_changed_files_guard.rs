//! Nothing web-side may ask the engine to move its watched worktree.
//!
//! `Command::WatchChangedFiles` points the engine's ONE watched worktree at a
//! session, and the terminal UI's changed-files pane is what reads it. The web
//! UI does not need it: its own changes pane rides the per-session
//! `ChangesService`, which is insulated from that selection entirely.
//!
//! That distinction only became load-bearing when the web server learned to serve
//! behind a live terminal UI. In that mode one engine backs both surfaces, so a
//! browser sending this command would silently retarget the watch and blank the
//! changes pane the person at the keyboard is looking at.
//!
//! Measured rather than assumed: at the time of writing, no route, handler or
//! frontend module sends it. This test is what keeps that true, because a future
//! sender would be a correct-looking three-line change with a consequence
//! nowhere near it.

use std::path::{Path, PathBuf};

/// The wire name a client would have to use, and the Rust variant a route would
/// have to name. Either one appearing web-side is the regression.
const FORBIDDEN: [&str; 2] = ["WatchChangedFiles", "watch_changed_files"];

/// Files that are allowed to contain the name: this test, which has to spell it
/// out to look for it.
fn is_exempt(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "watch_changed_files_guard.rs")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `node_modules` is somebody else's code and enormous.
            if path.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            collect(&path, out);
            continue;
        }
        let interesting = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "rs" | "ts" | "tsx"));
        if interesting && !is_exempt(&path) {
            out.push(path);
        }
    }
}

#[test]
fn no_web_surface_sends_the_changed_files_watch_command() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&crate_root.join("src"), &mut files);
    collect(&crate_root.join("tests"), &mut files);
    collect(&crate_root.join("web").join("src"), &mut files);
    assert!(
        !files.is_empty(),
        "the scan found no source files, so it would pass no matter what"
    );

    let mut offenders = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if FORBIDDEN.iter().any(|needle| line.contains(needle)) {
                offenders.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a web surface sends the changed-files watch command, which retargets the engine's one \
         watched worktree and blanks the terminal UI's changes pane while the background server \
         is on. The web UI's own changes pane rides the per-session ChangesService and does not \
         need this. Found:\n{}",
        offenders.join("\n")
    );
}
