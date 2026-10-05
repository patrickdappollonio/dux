//! A table names where each entry the file leaves out came from, and a path
//! that holds anything a setting name cannot is never repeated.
use super::*;

fn paths_with(body: &str) -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
    };
    std::fs::write(&paths.config_path, body).expect("seed");
    (tmp, paths)
}

fn get_out(paths: &DuxPaths, key: &str) -> String {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], paths, &mut out, &mut err).expect("get");
    String::from_utf8(out).unwrap()
}

/// The marker names the setting the value came from, the same one stderr
/// names; a true default is still `# default`.
#[test]
fn a_tables_markers_name_where_each_value_came_from() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[server]\nbind = \"10.0.0.1:4000\"\n");
    let out = get_out(&paths, "server");
    let line = |name: &str| {
        out.lines()
            .find(|line| line.starts_with(&format!("{name} =")))
            .unwrap_or_else(|| panic!("{name} missing:\n{out}"))
            .to_string()
    };
    assert!(line("host").ends_with("# from server.bind"), "{out}");
    assert!(line("port").ends_with("# from server.bind"), "{out}");
    assert!(line("color").ends_with("# default"), "{out}");

    let (_tmp, paths) = paths_with("[keys]\nexit_interactive = [\"ctrl-x\"]\n");
    let out = get_out(&paths, "keys");
    let toggle = out
        .lines()
        .find(|line| line.starts_with("toggle_fullscreen ="))
        .expect("listed");
    assert!(toggle.ends_with("# from keys.exit_interactive"), "{out}");
}

/// A value typed into the path (after `:`, a space, or before `=`) is never
/// repeated, by `get` or by `set`.
#[test]
fn a_path_holding_more_than_a_setting_name_is_never_repeated() {
    let (_tmp, paths) = paths_with("");
    for path in [
        "server.auth.password:hunter2",
        "server.auth.password hunter2",
        "server.auth.password:hunter2=x",
    ] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let error =
            run_get(&[path.to_string()], &paths, &mut out, &mut err).expect_err("not a setting");
        let message = format!("{error:#}");
        assert!(!message.contains("hunter2"), "{message}");
        assert!(message.contains("not a setting name"), "{message}");

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
        let mut said = Vec::new();
        let error = run_set(&[path.to_string()], &paths, &mut NoSecrets, &mut said)
            .expect_err("not a setting");
        assert!(!format!("{error:#}").contains("hunter2"), "{error:#}");
    }
}
