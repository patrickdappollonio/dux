//! `--stdin` refuses what is too large in words that fit what was being read.
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

/// A pipe holding more than the largest value `--stdin` reads.
struct Oversized;
impl SecretSource for Oversized {
    fn read_stdin(&mut self) -> Result<Password> {
        read_secret("x".repeat(STDIN_LIMIT as usize + 1).as_bytes())
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

/// A value read whole from a pipe.
struct Piped(String);
impl SecretSource for Piped {
    fn read_stdin(&mut self) -> Result<Password> {
        read_secret(self.0.as_bytes())
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

fn set_stdin(key: &str, secrets: &mut dyn SecretSource) -> (Result<()>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, "[ui]\nleft_width_pct = 25\n").unwrap();
    let mut out = Vec::new();
    let result = run_set(
        &[key.to_string(), "--stdin".to_string()],
        &paths,
        secrets,
        &mut out,
    );
    (result, std::fs::read_to_string(&paths.config_path).unwrap())
}

#[test]
fn an_environment_value_up_to_one_mebibyte_is_read_and_stored() {
    crate::config::install_canonical_renderer();
    let value = "v".repeat(ENV_VALUE_STDIN_LIMIT as usize);
    let (result, after) = set_stdin("env.BIG_VALUE", &mut Piped(format!("{value}\n")));
    result.expect("a value of exactly 1 MiB is stored");
    assert!(after.contains(&value));
}

#[test]
fn an_environment_value_past_one_mebibyte_is_refused_as_one() {
    crate::config::install_canonical_renderer();
    let (result, after) = set_stdin("env.BIG_VALUE", &mut Oversized);
    let error = format!("{:#}", result.expect_err("refused"));
    assert!(
        error.contains("an environment value larger than 1 MiB"),
        "{error}"
    );
    assert!(!error.contains("password"), "{error}");
    assert!(!after.contains("BIG_VALUE"));
}

#[test]
fn a_password_past_any_limit_is_refused_as_a_password() {
    crate::config::install_canonical_renderer();
    let (result, after) = set_stdin("server.auth.password", &mut Oversized);
    let error = format!("{:#}", result.expect_err("refused"));
    assert!(
        error.contains("more than any password dux accepts"),
        "{error}"
    );
    assert!(!after.contains("password_hash"));
}

/// A password longer than `max_password_bytes` but within what `--stdin`
/// reads is refused by the password's own limit, never stored.
#[test]
fn a_password_past_max_password_bytes_keeps_its_own_limit() {
    crate::config::install_canonical_renderer();
    let long = "correct horse battery staple ".repeat(4000);
    let (result, after) = set_stdin("server.auth.password", &mut Piped(long));
    let error = format!("{:#}", result.expect_err("refused"));
    assert!(error.contains("max_password_bytes"), "{error}");
    assert!(!after.contains("password_hash"));
}
