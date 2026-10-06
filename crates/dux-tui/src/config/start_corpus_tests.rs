//! The terminal UI's start against the start corpus: for every case, it
//! starts exactly when the release before the config checks were rebuilt
//! did (or as the corpus says it changed on purpose), and the one list of
//! start checks agrees with it.
use super::*;

#[test]
fn the_terminal_ui_starts_exactly_as_the_start_corpus_says() {
    install_canonical_renderer();
    let mut wrong = Vec::new();
    for case in dux_core::start_check_fixtures::start_corpus() {
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
        fs::write(&paths.config_path, &case.text).expect("seed");
        // As the start goes on: the config read, its keys and its projects.
        let starts = match ensure_config(&paths) {
            Err(_) => false,
            Ok(config) => {
                validate_keys(&config.keys).is_ok()
                    && dux_core::config_sync::validate_project_records(
                        "config.toml",
                        &config.projects,
                    )
                    .is_ok()
            }
        };
        let listed =
            dux_core::config::start_refusal(&case.text, dux_core::config::Surface::TerminalUi)
                .is_none();
        if starts != case.terminal_ui_starts || listed != case.terminal_ui_starts {
            wrong.push(format!(
                "{:?}: starts {starts}, the list says {listed}, the corpus says {}",
                case.text, case.terminal_ui_starts
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
