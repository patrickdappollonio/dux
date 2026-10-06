//! What `dux server`'s load recovers is logged once per load: the start
//! checks, `get` and a save's base read the same text again and say nothing.

#[test]
fn a_load_logs_its_recovery_once_whatever_read_the_file_before() {
    let dir = tempfile::tempdir().unwrap();
    let paths = dux_core::config::DuxPaths {
        root: dir.path().to_path_buf(),
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
        socket_path: dir.path().join("dux.sock"),
    };
    let logging = dux_core::config::LoggingConfig {
        path: dir.path().join("dux.log").to_string_lossy().into_owned(),
        ..Default::default()
    };
    dux_core::logger::init(&logging, &paths);
    let body = "[ui]\nleft_width_pct = \"wide\"\n";
    std::fs::write(&paths.config_path, body).unwrap();

    // Everything that reads the file without loading it.
    let _ = dux_core::config::start_problems_of(body);
    let _ = dux_core::config::load_corrections_of(body);
    let _ = dux_core::config::config_from_text_as_loaded(body);
    let _ = dux_core::config::effective_config_from_text(body);
    // The one load.
    let _ = dux_core::config::load_config(&paths).expect("dux server starts");

    let log = std::fs::read_to_string(dir.path().join("dux.log")).unwrap_or_default();
    assert_eq!(
        log.matches("is invalid; resetting it to its default")
            .count(),
        1,
        "one load, one line:\n{log}"
    );
    assert!(
        log.contains("config [ui] left_width_pct is invalid"),
        "{log}"
    );
}
