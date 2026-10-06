//! Adversarial review probes.
use super::*;

struct Canned {
    stdin: Option<&'static str>,
}

impl SecretSource for Canned {
    fn read_stdin(&mut self) -> Result<Password> {
        read_secret(self.stdin.expect("stdin was not expected").as_bytes())
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

fn setup(body: Option<&str>) -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
        socket_path: tmp.path().join("dux.sock"),
    };
    if let Some(body) = body {
        std::fs::write(&paths.config_path, body).expect("seed");
    }
    (tmp, paths)
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn get(paths: &DuxPaths, list: &[&str]) -> (String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&args(list), paths, &mut out, &mut err).expect("get");
    (
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn set(paths: &DuxPaths, list: &[&str], stdin: Option<&'static str>) -> Result<String> {
    let mut out = Vec::new();
    run_set(&args(list), paths, &mut Canned { stdin }, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

/// An `[env]` entry whose name is not a variable name (a token pasted where
/// the name goes) is identified by its line everywhere else; `get env`
/// without `--show` must not print that name either.
#[test]
fn get_env_without_show_never_prints_an_invalid_entry_name() {
    let (_t, paths) = setup(Some("[env]\n\"sk-live-SECRETNAME\" = 5\n"));
    let (out, err) = get(&paths, &["env"]);
    assert!(
        !out.contains("SECRETNAME") && !err.contains("SECRETNAME"),
        "the invalid name was printed:\nstdout: {out}\nstderr: {err}"
    );
}

/// config.toml is a symlink whose target is gone: the terminal UI and
/// `dux server` both refuse to start with it, so `get` must not report a
/// value "dux uses".
#[test]
fn get_through_a_dangling_symlink_does_not_claim_a_value_in_use() {
    let (tmp, paths) = setup(None);
    std::os::unix::fs::symlink(tmp.path().join("gone.toml"), &paths.config_path).unwrap();
    // The surfaces' own verdicts.
    assert!(
        crate::config::ensure_config(&paths).is_err(),
        "the terminal UI refuses"
    );
    assert!(
        dux_core::config::load_config(&paths).is_err(),
        "dux server refuses"
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = run_get(&args(&["server.port"]), &paths, &mut out, &mut err);
    let (out, err) = (
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    );
    assert!(
        result.is_err() || !err.contains("uses this value"),
        "get claimed a value in use for a file no surface starts with:\nstdout: {out}\nstderr: {err}"
    );
}

/// A dux holds the lock but cannot be asked to reload (no control socket
/// answers), so the new password applies only when it restarts. The message
/// must not say that browsers are signed out now.
#[test]
fn a_password_set_beside_a_dux_that_cannot_be_asked_does_not_claim_browsers_were_signed_out() {
    let (_tmp, paths) = setup(Some("[server]\nport = 3890\n"));
    let _running = dux_core::lockfile::SingleInstanceLock::acquire(&paths.lock_path).expect("lock");
    let out = set(
        &paths,
        &["server.auth.password", "--stdin"],
        Some("Tr0ub4dor&3-correct-horse-battery\n"),
    )
    .expect("set");
    assert!(
        out.contains("does not answer on its control socket"),
        "not asked:\n{out}"
    );
    assert!(
        !out.contains("is signed out and logs in with the new password"),
        "claims browsers were signed out although nothing reloaded:\n{out}"
    );
}

/// The file sets the deprecated `[server] bind`, which every surface carries
/// over into host and port. `get server.host` must not present the address
/// it came from as a value the file leaves out.
#[test]
fn get_says_where_a_value_carried_over_from_a_deprecated_key_comes_from() {
    let (_t, paths) = setup(Some("[server]\nbind = \"0.0.0.0:4000\"\n"));
    let (out, err) = get(&paths, &["server.host"]);
    assert_eq!(out, "0.0.0.0\n");
    assert!(
        err.contains("bind"),
        "get presents a public bind address as an unset setting, with no word of where it \
         comes from:\n{err}"
    );
}
