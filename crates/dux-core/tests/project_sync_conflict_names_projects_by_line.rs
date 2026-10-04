//! Projects are always named by their line in problem messages, never by
//! path, name or id (what a user may keep private in a pasted bug report).
//! A `[[projects]]` entry whose path was edited by hand no longer matches
//! the path SQLite holds for the same id; the error that stops `dux server`
//! (and the terminal UI's start and every reload) names the id and both
//! paths.

const ID: &str = "client-acme-secret-id";
const OLD_PATH: &str = "/home/u/clients/acme-merger-2026";
const NEW_PATH: &str = "/home/u/clients/acme-merger-renamed";

#[test]
fn a_project_path_edited_by_hand_is_named_by_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let paths = dux_core::config::DuxPaths {
        root: dir.path().to_path_buf(),
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
    };
    let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
    store
        .upsert_project_at(
            &dux_core::config::ProjectConfig {
                id: ID.to_string(),
                path: OLD_PATH.to_string(),
                name: None,
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            },
            0,
        )
        .unwrap();
    drop(store);
    std::fs::write(
        &paths.config_path,
        format!("[[projects]]\nid = \"{ID}\"\npath = \"{NEW_PATH}\"\n"),
    )
    .unwrap();
    let mut config = dux_core::config::load_config(&paths).expect("dux server reads it");
    let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
    let error = dux_core::config_sync::reconcile_config_projects(&mut config, &store, |_| Ok(()))
        .expect_err("the stores disagree");
    let message = format!("{error:#}");
    for private in [ID, OLD_PATH, NEW_PATH] {
        assert!(
            !message.contains(private),
            "the project is named by {private:?}: {message}"
        );
    }
}
