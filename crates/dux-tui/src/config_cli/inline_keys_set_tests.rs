//! `dux config set` on a file whose `[keys]` is written inline.
use super::*;

struct NoSecrets;
impl SecretSource for NoSecrets {
    fn read_stdin(&mut self) -> Result<Password> {
        bail!("no stdin")
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

/// The terminal UI will not start with this file (its start refuses it),
/// yet `set` says nothing of it, and that the change applies at the next start.
#[test]
fn set_says_the_terminal_ui_will_not_start_with_a_file_it_refuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let paths = DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    };
    let text = "keys = { generate_commit_message = \"ctrl-y\" }\n";
    std::fs::write(&paths.config_path, text).expect("seed");
    let mut out = Vec::new();
    run_set(
        &["server.port".to_string(), "4000".to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    )
    .expect("the set is accepted");
    let said = String::from_utf8(out).expect("utf8");
    assert!(
        crate::config::ensure_config(&paths).is_err()
            || crate::config::ensure_config(&paths)
                .ok()
                .is_some_and(|c| crate::config::validate_keys(&c.keys).is_err()),
        "precondition: the terminal UI refuses the file"
    );
    assert!(
        said.contains("the terminal UI will not start"),
        "set never says the terminal UI will not start with this file, and says the change \
         applies the next time dux starts, but the terminal UI refuses it:\n{said}"
    );
}
