//! A password no browser password field can type is never stored.
use super::*;

fn paths_in(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}

struct Stdin(String);
impl SecretSource for Stdin {
    fn read_stdin(&mut self) -> Result<Password> {
        Ok(Password::new(self.0.clone()))
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

/// A password read from a file that ends in a blank line keeps a line break,
/// which no browser password field can type: the web login can never match it.
#[test]
fn a_password_holding_a_line_break_is_refused() {
    crate::config::install_canonical_renderer();
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, "[ui]\nleft_width_pct = 25\n").unwrap();
    let mut out = Vec::new();
    // `printf 'correct horse battery staple veranda\n\n'` piped in.
    let r = read_secret(&b"correct horse battery staple veranda\n\n"[..]).unwrap();
    assert!(
        r.expose().contains('\n'),
        "precondition: a line break survives the read"
    );
    let result = run_set(
        &["server.auth.password".to_string(), "--stdin".to_string()],
        &paths,
        &mut Stdin(r.expose().to_string()),
        &mut out,
    );
    let after = std::fs::read_to_string(&paths.config_path).unwrap();
    assert!(
        result.is_err() && !after.contains("password_hash = \"$argon2id"),
        "a password no login form can type was stored:\n{}",
        String::from_utf8_lossy(&out)
    );
}

/// The refusal says why in the decided words, repeats nothing of the
/// password, and leaves the file as it was.
#[test]
fn the_refusal_names_the_rule_and_never_the_password() {
    crate::config::install_canonical_renderer();
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    let before = "[ui]\nleft_width_pct = 25\n";
    std::fs::write(&paths.config_path, before).unwrap();
    let mut out = Vec::new();
    let error = run_set(
        &["server.auth.password".to_string(), "--stdin".to_string()],
        &paths,
        &mut Stdin("correct horse\tbattery staple veranda".to_string()),
        &mut out,
    )
    .expect_err("refused");
    let said = format!("{error:#}{}", String::from_utf8_lossy(&out));
    assert!(
        said.contains(
            "the password cannot contain line breaks, tabs or other control characters, \
             because a browser's password field cannot type them. Nothing was changed."
        ),
        "{said}"
    );
    assert!(
        !said.contains("horse") && !said.contains("staple"),
        "{said}"
    );
    assert_eq!(std::fs::read_to_string(&paths.config_path).unwrap(), before);
}
