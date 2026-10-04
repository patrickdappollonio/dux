//! A value written as a `[keys]` binding is never printed.
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

fn paths(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
    }
}

fn set(p: &DuxPaths, args: &[&str]) -> (Result<()>, String) {
    let mut out = Vec::new();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_set(&args, p, &mut NoSecrets, &mut out);
    (r, String::from_utf8(out).unwrap())
}

fn get(p: &DuxPaths, args: &[&str]) -> (String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_get(&args, p, &mut out, &mut err);
    let mut e = String::from_utf8(err).unwrap();
    if let Err(x) = r {
        e.push_str(&format!("ERR: {x:#}"));
    }
    (String::from_utf8(out).unwrap(), e)
}

#[test]
fn a_token_pasted_as_a_key_binding_is_never_printed() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let token = "ghp_SeCrEtToKeN1234567890";
    std::fs::write(&p.config_path, format!("[keys]\nquit = [\"{token}\"]\n")).unwrap();
    let (r, out) = set(&p, &["server.port", "4000"]);
    r.expect("an unrelated set is accepted");
    let (get_out, get_err) = get(&p, &["server.port"]);
    let refusal = format!(
        "{:#}",
        crate::config::ensure_config(&p).expect_err("refused")
    );
    for (what, text) in [
        ("set", &out),
        ("get", &format!("{get_out}{get_err}")),
        ("start", &refusal),
    ] {
        assert!(
            !text.contains(token),
            "{what} repeats the value pasted into [keys]:\n{text}"
        );
    }
}
