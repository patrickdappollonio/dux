//! The previews of `dux config regenerate` and `dux config restore-docs`
//! print the user's own file. A plaintext password the file holds must never
//! be printed by any printer, and nothing below a hidden key (`[env]`) may be
//! printed without `--show`.
use super::*;

const CHILD_ENV: &str = "DUX_PREVIEW_LEAK_CHILD";
const CHILD_TEST: &str = "cli::preview_leak_tests::child_runs_a_preview";

/// Not a test on its own: re-run as a child (so its stdout is real and can
/// be read by the parent), it runs one preview on the config dir it is given.
#[test]
fn child_runs_a_preview() {
    let Ok(spec) = std::env::var(CHILD_ENV) else {
        return;
    };
    let (which, root) = spec.split_once(':').unwrap();
    let root = std::path::PathBuf::from(root);
    let paths = DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        lock_path: root.join("dux.lock"),
        worktrees_root: root.join("worktrees"),
        root: root.clone(),
    };
    crate::config::install_canonical_renderer();
    let _ = match which {
        "regenerate" => run_regenerate(&paths, false, false),
        "restore-docs" => run_restore_docs(&paths, false, false),
        _ => unreachable!(),
    };
}

fn preview(which: &str, file: &str) -> String {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), file).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, format!("{which}:{}", dir.path().display()))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn regenerate_preview_never_prints_a_plaintext_password() {
    let out = preview(
        "regenerate",
        "[server.auth]\npassword = \"correct horse battery staple\"\n",
    );
    assert!(out.contains("--- current"), "the preview ran: {out}");
    assert!(
        !out.contains("correct horse battery staple"),
        "the plaintext password was printed:\n{out}"
    );
}

#[test]
fn regenerate_preview_never_prints_an_env_value() {
    let out = preview(
        "regenerate",
        "[env]\nGITHUB_TOKEN = \"ghp_reviewSecretToken\"\n",
    );
    assert!(out.contains("--- current"), "the preview ran: {out}");
    assert!(
        !out.contains("ghp_reviewSecretToken"),
        "an [env] value was printed without --show:\n{out}"
    );
}

#[test]
fn restore_docs_preview_never_prints_an_env_value() {
    let out = preview(
        "restore-docs",
        "[env]\nGITHUB_TOKEN = \"ghp_reviewSecretToken\"\n",
    );
    assert!(out.contains("--- current"), "the preview ran: {out}");
    assert!(
        !out.contains("ghp_reviewSecretToken"),
        "an [env] value was printed without --show:\n{out}"
    );
}
