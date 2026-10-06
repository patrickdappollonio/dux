//! Inline-table sections are judged by the start list exactly as the terminal
//! UI reads them.
use super::*;

fn paths_with(text: &str) -> (tempfile::TempDir, DuxPaths) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path().to_path_buf();
    let paths = DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
        worktrees_root: root.join("worktrees"),
        root,
    };
    fs::write(&paths.config_path, text).expect("seed");
    (dir, paths)
}

/// The terminal UI's start, as the start corpus test runs it.
fn terminal_ui_starts(paths: &DuxPaths) -> bool {
    match ensure_config(paths) {
        Err(_) => false,
        Ok(config) => {
            validate_keys(&config.keys).is_ok()
                && dux_core::config_sync::validate_project_records("config.toml", &config.projects)
                    .is_ok()
        }
    }
}

/// The terminal UI refuses this file (it did at 7437238c too), but the one
/// list of start checks, which `dux config get`/`set` report from, says the
/// terminal UI starts with it.
#[test]
fn the_list_agrees_with_the_terminal_ui_on_an_inline_keys_table() {
    install_canonical_renderer();
    let text = "keys = { generate_commit_message = \"ctrl-y\" }\n";
    let (_dir, paths) = paths_with(text);
    let starts = terminal_ui_starts(&paths);
    let listed =
        dux_core::config::start_refusal(text, dux_core::config::Surface::TerminalUi).is_none();
    assert_eq!(
        starts, listed,
        "the terminal UI starts: {starts}; the one list says it starts: {listed}"
    );
}

/// 7437238c's terminal UI started with each of these files (its migrations
/// never read a deprecated key inside an inline table, and nothing else
/// reads it: `dux server` uses its defaults), and nothing about auth
/// changed. Now the terminal UI refuses them.
#[test]
fn an_inline_deprecated_key_nothing_reads_still_lets_the_terminal_ui_start() {
    install_canonical_renderer();
    for text in [
        "server = { bind = 5 }\n",
        "defaults = { prompt_for_name = \"yes\" }\n",
    ] {
        let (_dir, paths) = paths_with(text);
        assert!(
            terminal_ui_starts(&paths),
            "the terminal UI started with {text:?} at 7437238c; now: {:?}",
            ensure_config(&paths).err().map(|e| e.to_string())
        );
    }
}
