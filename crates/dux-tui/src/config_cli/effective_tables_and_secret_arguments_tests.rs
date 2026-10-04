//! `get` of a table marks what comes from defaults; a secret setting never
//! repeats an argument it was not meant to take.
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

/// Each entry of a table the file leaves to its default is marked so; one
/// the file writes is not.
#[test]
fn a_tables_defaults_are_marked_and_the_files_own_entries_are_not() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[ui]\nleft_width_pct = 30\n");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&["ui".to_string()], &paths, &mut out, &mut err).expect("get");
    let out = String::from_utf8(out).unwrap();
    let line = |name: &str| {
        out.lines()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("{name} missing:\n{out}"))
            .to_string()
    };
    assert!(!line("left_width_pct").contains("# default"), "{out}");
    assert!(line("terminal_font_size").contains("# default"), "{out}");
}

/// A secret setting given an argument it does not take (a flag, a value)
/// says so without repeating it, whatever it was.
#[test]
fn a_secret_setting_never_repeats_an_unexpected_argument() {
    let (_tmp, paths) = paths_with("");
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
    for args in [
        vec!["server.auth.password", "--hunter2"],
        vec!["server.auth.password", "--stdin", "--hunter2=x"],
        vec!["env.TOKEN", "--hunter2"],
    ] {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let mut out = Vec::new();
        let error = run_set(&args, &paths, &mut NoSecrets, &mut out).expect_err("refused");
        let message = format!("{error:#}");
        assert!(!message.contains("hunter2"), "{message}");
        assert!(message.contains("unexpected argument"), "{message}");
    }
}
