//! A token pasted where a name goes in `[env]` (a name that breaks the
//! variable-name rule) is never printed by the start checks, which place it
//! by its line. `dux server`'s load still writes it to dux.log verbatim when
//! it resets the entry ("config [env] <name> is invalid").

#[test]
fn a_token_pasted_as_an_env_name_is_not_written_to_dux_log() {
    let token = "sk-proj-AbCdEf0123456789 x";
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
    std::fs::write(&paths.config_path, format!("[env]\n\"{token}\" = 1\n")).unwrap();

    // The start checks never print it ...
    for problem in
        dux_core::config::start_problems_of(&std::fs::read_to_string(&paths.config_path).unwrap())
    {
        assert!(!problem.message.contains("sk-proj"), "{}", problem.message);
    }
    // ... and dux server starts with it, resetting the entry.
    let _ = dux_core::config::load_config(&paths).expect("dux server starts");

    let log = std::fs::read_to_string(dir.path().join("dux.log")).unwrap_or_default();
    assert!(
        !log.contains("sk-proj"),
        "the pasted token reached dux.log:\n{log}"
    );
}
