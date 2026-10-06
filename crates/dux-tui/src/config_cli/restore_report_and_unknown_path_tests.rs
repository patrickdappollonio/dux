//! The restore report and an unknown setting never repeat a name or value
//! the file or the command line holds where a setting name goes.
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

#[test]
fn restore_docs_never_prints_a_token_pasted_as_a_key_under_an_array_of_tables() {
    crate::config::install_canonical_renderer();
    const TOKEN: &str = "sk-proj-AbCdEf0123456789";
    let mut leaked = Vec::new();
    for text in [
        format!("[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n\"{TOKEN} x\" = \"1\"\n"),
        format!("[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n{TOKEN} = \"1\"\n"),
        format!("[ui]\n\"{TOKEN} x\" = 1\n"),
        format!("[[extra]]\n\"{TOKEN} x\" = 1\n"),
    ] {
        let restored = crate::config::restore_documentation(&text).expect("restores");
        let all = [
            restored.dropped.clone(),
            restored.preserved.clone(),
            restored.unplaceable.clone(),
        ]
        .concat();
        if all.iter().any(|p| p.contains(TOKEN)) {
            leaked.push(format!("{text:?} -> {all:?}"));
        }
    }
    assert!(leaked.is_empty(), "{}", leaked.join("\n"));
}

#[test]
fn a_password_typed_after_a_dot_is_never_repeated() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, "").unwrap();
    let mut leaks = Vec::new();
    for path in [
        "server.auth.password.hunter2",
        "server.auth.password_hash.hunter2",
        "env.TOKEN.hunter2",
    ] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let e = run_get(&[path.to_string()], &paths, &mut out, &mut err)
            .map_err(|e| format!("{e:#}"))
            .err()
            .unwrap_or_default();
        if e.contains("hunter2") {
            leaks.push(format!("get {path}: {e}"));
        }
        let mut said = Vec::new();
        let e = run_set(&[path.to_string()], &paths, &mut NoSecrets, &mut said)
            .map_err(|e| format!("{e:#}"))
            .err()
            .unwrap_or_default();
        if e.contains("hunter2") {
            leaks.push(format!("set {path}: {e}"));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}
