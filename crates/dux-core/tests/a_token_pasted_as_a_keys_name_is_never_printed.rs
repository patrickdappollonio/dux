//! `[keys]` is a map whose keys the branch itself counts as names the user
//! chose (see `stray_password_hashes`, which exempts it with `[env]`,
//! providers, projects and macros). A token pasted there as a name breaks
//! the map's rule (it is no action), yet it is quoted by the start checks
//! and written to dux.log by `dux server`'s load.

const TOKEN: &str = "sk-proj-AbCdEf0123456789";

#[test]
fn a_token_pasted_as_a_keys_name_is_not_quoted_by_the_start_checks() {
    let raw = format!("[keys]\n\"{TOKEN}\" = 5\n");
    let problems = dux_core::config::start_problems_of(&raw);
    assert!(
        !problems.is_empty(),
        "precondition: it stops the terminal UI"
    );
    let leaked: Vec<&String> = problems
        .iter()
        .map(|p| &p.message)
        .filter(|m| m.contains(TOKEN))
        .collect();
    assert!(
        leaked.is_empty(),
        "the start checks quote the pasted token: {leaked:?}"
    );
}

#[test]
fn a_token_pasted_as_a_keys_name_is_not_written_to_dux_log() {
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
    std::fs::write(&paths.config_path, format!("[keys]\n\"{TOKEN}\" = 5\n")).unwrap();
    // dux server starts with it, resetting the entry.
    let _ = dux_core::config::load_config(&paths).expect("dux server starts");
    let log = std::fs::read_to_string(dir.path().join("dux.log")).unwrap_or_default();
    assert!(
        !log.contains(TOKEN),
        "the pasted token reached dux.log:\n{log}"
    );
}
