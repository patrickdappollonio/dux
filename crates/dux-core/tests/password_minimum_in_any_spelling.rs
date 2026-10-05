//! A password set while the file's minimum is written in a spelling dux
//! does not read is checked against the defaults, not refused.
use dux_core::auth::Password;

fn try_set(body: &str) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, body).unwrap();
    // 16 characters, strong, well under the 40 the file asks for.
    let pw = Password::new("Tr0ub4dor&3-zqx!".to_string());
    dux_core::config_keys::set_password(&path, &pw, &[])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn a_misspelled_minimum_never_lets_a_shorter_password_through() {
    let mut wrong = Vec::new();
    for body in [
        "[server.auth]\nminimum-password-length = 40\n",
        "[server.auth]\nMinimum_Password_Length = 40\n",
        "[server]\nminimum_password_length = 40\n",
        "[Server.auth]\nminimum_password_length = 40\n",
    ] {
        // sanity: the right spelling refuses it
        assert!(
            try_set(
                &body
                    .replace("minimum-password-length", "minimum_password_length")
                    .replace("Minimum_Password_Length", "minimum_password_length")
                    .replace("[server]\n", "[server.auth]\n")
                    .replace("[Server.auth]", "[server.auth]")
            )
            .is_err()
        );
        let result = try_set(body);
        if result.is_ok() {
            wrong.push(format!("{body:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a 16-character password was set although the file asks for 40: {}",
        wrong.join(", ")
    );
}
