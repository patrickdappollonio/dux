//! A password hash written where dux does not read it must stop the start
//! (fail closed), never let dux serve with no password. The branch already
//! refuses `[server.Auth]`, `[auht]` and a bare `password_hash` under
//! `[server]`, but not these equally plausible misspellings of the header.

const HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";

fn starts_with_no_password(body: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let paths = dux_core::config::DuxPaths {
        root: dir.path().to_path_buf(),
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
        socket_path: dir.path().join("dux.sock"),
    };
    std::fs::write(&paths.config_path, body).unwrap();
    let refused =
        dux_core::config::start_refusal(body, dux_core::config::Surface::DuxServer).is_some();
    match dux_core::config::load_config(&paths) {
        Ok(config) => !refused && config.server.auth.password_hash.is_empty(),
        Err(_) => false,
    }
}

#[test]
fn a_hash_under_a_capitalised_server_header_stops_dux_server() {
    let body =
        format!("[server]\nhost = \"0.0.0.0\"\n\n[Server.auth]\npassword_hash = \"{HASH}\"\n");
    assert!(
        !starts_with_no_password(&body),
        "dux server starts on 0.0.0.0 with NO password although the file sets one under [Server.auth]"
    );
}

#[test]
fn a_hash_under_a_quoted_server_auth_header_stops_dux_server() {
    let body =
        format!("[server]\nhost = \"0.0.0.0\"\n\n[\"server.auth\"]\npassword_hash = \"{HASH}\"\n");
    assert!(
        !starts_with_no_password(&body),
        "dux server starts on 0.0.0.0 with NO password although the file sets one under [\"server.auth\"]"
    );
}

#[test]
fn a_hash_under_a_quoted_dotted_key_in_server_stops_dux_server() {
    let body = format!("[server]\nhost = \"0.0.0.0\"\n\"auth.password_hash\" = \"{HASH}\"\n");
    assert!(
        !starts_with_no_password(&body),
        "dux server starts on 0.0.0.0 with NO password although [server] holds \"auth.password_hash\""
    );
}
