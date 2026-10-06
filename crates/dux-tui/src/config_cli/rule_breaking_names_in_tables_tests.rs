use super::*;

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

fn get(paths: &DuxPaths, path: &str) -> String {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let r = run_get(&[path.to_string()], paths, &mut out, &mut err);
    format!(
        "r={:?}\nOUT:{}ERR:{}",
        r.err().map(|e| format!("{e:#}")),
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}

/// A provider or macro name that breaks its map's naming rule may be a
/// token pasted in the wrong place, so it is never printed, only placed by
/// its line. `get` of the table that holds it prints it anyway, on stdout.
#[test]
fn get_of_a_table_never_prints_a_name_that_breaks_its_rule() {
    for (body, path) in [
        (
            "[providers.\"ghp_SECRETTOKEN 123\"]\ncommand = \"x\"\n",
            "providers",
        ),
        (
            "[providers.\"ghp_SECRETTOKEN 123\"]\nargs = 5\n",
            "providers",
        ),
        ("[macros]\n\"ghp_SECRETTOKEN 123\" = 5\n", "macros"),
    ] {
        let (_t, paths) = setup(Some(body));
        let printed = get(&paths, path);
        assert!(
            !printed.contains("SECRETTOKEN"),
            "`dux config get {path}` printed a name that breaks its rule:\n{printed}"
        );
    }
}

/// Without `--show` the name is a marker naming its line; with it, the
/// table is printed as the file names its entries.
#[test]
fn a_rule_breaking_name_is_a_line_marker_unless_shown() {
    let (_t, paths) = setup(Some(
        "[providers.\"ghp_SECRETTOKEN 123\"]\ncommand = \"x\"\n",
    ));
    let printed = get(&paths, "providers");
    assert!(
        printed.contains("<the entry on line 1 of [providers]>"),
        "{printed}"
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(
        &["providers".to_string(), "--show".to_string()],
        &paths,
        &mut out,
        &mut err,
    )
    .expect("get --show");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("ghp_SECRETTOKEN 123"), "{out}");
}
