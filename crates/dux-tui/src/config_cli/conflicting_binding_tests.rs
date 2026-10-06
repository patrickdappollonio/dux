//! A binding that conflicts with another is never repeated in an error.
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
        socket_path: dir.join("dux.sock"),
    }
}

fn set(p: &DuxPaths, args: &[&str]) -> (Result<()>, String) {
    let mut out = Vec::new();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_set(&args, p, &mut NoSecrets, &mut out);
    (r, String::from_utf8(out).unwrap())
}

/// Two actions bound to the same key: the start refusal, and `set`'s list of
/// what still stops a start, repeat the binding's value.
#[test]
fn a_conflicting_binding_value_is_never_printed() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let value = "ctrl-alt-y";
    std::fs::write(
        &p.config_path,
        format!("[keys]\ntoggle_project = [\"{value}\"]\nnew_agent = [\"{value}\"]\n"),
    )
    .unwrap();
    let (r, out) = set(&p, &["server.port", "4000"]);
    r.expect("an unrelated set is accepted");
    let refusal = format!(
        "{:#}",
        crate::config::ensure_config(&p).expect_err("refused")
    );
    for (what, text) in [("set", &out), ("start", &refusal)] {
        assert!(
            !text.contains(value),
            "{what} repeats a [keys] binding value:\n{text}"
        );
    }
}
