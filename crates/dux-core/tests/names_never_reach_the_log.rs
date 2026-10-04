//! A name the file writes that is not a setting name (a token pasted where a
//! key goes) never reaches dux.log when `dux server` loads the file, in any
//! structural position (see `NAME_POSITIONS`).
// The same positions and tokens as the printers' property test, from the one
// fixture file (an integration test does not see the crate's test items).
#[allow(dead_code)]
#[path = "../src/start_check_fixtures.rs"]
mod start_check_fixtures;
use start_check_fixtures::{NAME_POSITIONS, name_tokens};

#[test]
fn a_name_that_is_not_a_setting_name_never_reaches_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let paths = dux_core::config::DuxPaths {
        root: dir.path().to_path_buf(),
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
    };
    let logging = dux_core::config::LoggingConfig {
        path: dir.path().join("dux.log").to_string_lossy().into_owned(),
        ..Default::default()
    };
    dux_core::logger::init(&logging, &paths);
    let tokens = name_tokens();
    for token in &tokens {
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        for position in NAME_POSITIONS {
            std::fs::write(&paths.config_path, position.replace("{T}", bare)).unwrap();
            let _ = dux_core::config::load_config(&paths);
            let _ = dux_core::config::load_config_for_reload(&paths);
        }
    }
    let log = std::fs::read_to_string(dir.path().join("dux.log")).unwrap_or_default();
    // The loads did log what they recovered, so the check below means something.
    assert!(
        log.contains("is invalid; resetting"),
        "nothing was logged:\n{log}"
    );
    for token in &tokens {
        assert!(
            !log.contains(&token[..12]),
            "a pasted name reached dux.log:\n{log}"
        );
    }
}
