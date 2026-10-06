//! An auth setting written one level too high, or a password hash under
//! a spelling with dashes or capitals, stops the start rather than letting
//! dux serve without what the user meant to set.
use dux_core::auth::{Password, hash_password};
use dux_core::config::{DuxPaths, load_config};

fn paths(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}

fn hash() -> String {
    hash_password(&Password::new(
        "correct horse battery staple veranda".into(),
    ))
    .unwrap()
}

/// `require = "everywhere"` written under `[server]` instead of `[server.auth]`
/// (beside a real password): dux starts and asks for the password only off this
/// machine, so behind a same-machine reverse proxy nobody is asked at all.
#[test]
fn require_written_under_server_is_not_silently_ignored() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = paths(dir.path());
    let text = format!(
        "[server]\nrequire = \"everywhere\"\n\n[server.auth]\npassword_hash = \"{}\"\n",
        hash()
    );
    std::fs::write(&p.config_path, &text).unwrap();
    let loaded = load_config(&p);
    let problems = dux_core::config::start_problems_of(&text);
    assert!(
        loaded.is_err() || !problems.is_empty(),
        "dux server starts with require = {:?} although the file asks for everywhere",
        loaded.unwrap().server.auth.require
    );
}

/// The same table rule already refuses `[server.auht] require = "everywhere"`.
#[test]
fn require_in_a_near_miss_table_is_refused() {
    let text = "[server.auht]\nrequire = \"everywhere\"\n";
    assert!(!dux_core::config::start_problems_of(text).is_empty());
}

/// A password hash under `password-hash` (kebab case, as many TOML files
/// spell keys) directly under `[server]`: dux starts with no password, while
/// `PASSWORD_HASH` or `Password_Hash` in the same place stop the start.
#[test]
fn a_dashed_password_hash_under_server_is_not_silently_ignored() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = paths(dir.path());
    let h = hash();
    let upper = format!("[server]\nPASSWORD_HASH = \"{h}\"\n");
    assert!(
        !dux_core::config::start_problems_of(&upper).is_empty(),
        "precondition: a case variant is refused"
    );
    let text = format!("[server]\npassword-hash = \"{h}\"\n");
    std::fs::write(&p.config_path, &text).unwrap();
    let loaded = load_config(&p);
    assert!(
        loaded.is_err() || !dux_core::config::start_problems_of(&text).is_empty(),
        "dux server starts with no password (has_password = {})",
        loaded.unwrap().server.auth.has_password()
    );
}
