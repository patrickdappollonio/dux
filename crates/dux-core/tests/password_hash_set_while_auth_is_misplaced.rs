//! While any auth setting sits where dux does not read it, no password can
//! be set. `server.auth.password` refuses; does `server.auth.password_hash`?

use dux_core::config_keys::{lookup, set_plain};

#[test]
fn a_password_hash_cannot_be_set_while_an_auth_setting_is_misplaced() {
    let hash = dux_core::auth::hash_password(&dux_core::auth::Password::new(
        "correct horse battery staple veranda".to_string(),
    ))
    .unwrap();
    for misplaced in [
        "[server]\nminimum-password-length = 40\n",
        "[server]\nrequire = \"everywhere\"\n",
        "[server.authentication]\nminimum_password_length = 40\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, misplaced).unwrap();
        let result = set_plain(&path, &lookup("server.auth.password_hash").unwrap(), &hash);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            result.is_err() && !after.contains("argon2id"),
            "{misplaced:?}: a password was set while an auth setting is misplaced:\n{after}"
        );
    }
}
