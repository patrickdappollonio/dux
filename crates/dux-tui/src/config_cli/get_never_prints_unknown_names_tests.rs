//! `get` of a table without `--show` must not print a key name that is not a
//! setting name (a token pasted where a key goes).
use super::*;

const TOKEN: &str = "sk-proj-AbCdEf0123456789";

fn get_all(body: &str, key: &str) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
        socket_path: tmp.path().join("dux.sock"),
    };
    std::fs::write(&paths.config_path, body).expect("seed");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], &paths, &mut out, &mut err).expect("get");
    format!(
        "{}{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}

#[test]
fn get_ui_without_show_does_not_print_an_unknown_key_name() {
    crate::config::install_canonical_renderer();
    let said = get_all(&format!("[ui]\n\"{TOKEN}\" = 5\n"), "ui");
    assert!(
        !said.contains(TOKEN),
        "get ui printed the pasted name:\n{said}"
    );
}

#[test]
fn get_server_auth_without_show_does_not_print_an_unknown_key_name() {
    crate::config::install_canonical_renderer();
    let said = get_all(&format!("[server.auth]\n\"{TOKEN}\" = 5\n"), "server.auth");
    assert!(
        !said.contains(TOKEN),
        "get server.auth printed the pasted name:\n{said}"
    );
}

#[test]
fn get_a_stock_provider_without_show_does_not_print_an_unknown_key_name() {
    crate::config::install_canonical_renderer();
    let said = get_all(
        &format!("[providers.claude]\n\"{TOKEN}\" = 5\n"),
        "providers.claude",
    );
    assert!(
        !said.contains(TOKEN),
        "get providers.claude printed the pasted name:\n{said}"
    );
}
