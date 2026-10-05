//! `get` without `--show` never prints a name nested under a key it hides.
use super::*;

const TOKEN: &str = "sk-proj-AbCdEf0123456789";

fn get_all(body: &str, key: &str) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
    };
    std::fs::write(&paths.config_path, body).expect("seed");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = run_get(&[key.to_string()], &paths, &mut out, &mut err);
    format!(
        "{}{}{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
        result.err().map(|e| format!("{e:#}")).unwrap_or_default()
    )
}

#[test]
fn get_server_does_not_print_a_name_inside_an_unknown_server_auth_value() {
    crate::config::install_canonical_renderer();
    for body in [
        format!("[server.auth]\nextra = {{ \"{TOKEN}\" = 1 }}\n"),
        format!("[server.auth]\nextra = [{{ \"{TOKEN}\" = 1 }}]\n"),
    ] {
        let said = get_all(&body, "server");
        assert!(
            !said.contains(TOKEN),
            "get server printed a name from inside an unknown [server.auth] key:\n{said}"
        );
    }
}

#[test]
fn get_macros_does_not_print_a_name_inside_an_unknown_macro_field() {
    crate::config::install_canonical_renderer();
    for (body, key) in [
        (
            format!("[macros.m]\nextra = {{ \"{TOKEN}\" = 1 }}\n"),
            "macros",
        ),
        (
            format!("[macros.m]\nextra = [{{ \"{TOKEN}\" = 1 }}]\n"),
            "macros.m",
        ),
    ] {
        let said = get_all(&body, key);
        assert!(
            !said.contains(TOKEN),
            "get {key} printed a name from inside an unknown macro field:\n{said}"
        );
    }
}

/// When no surface starts with the file, `get` prints what the file says:
/// the unknown key's own name is a marker, but every name inside its table
/// is printed as written.
#[test]
fn get_ui_on_a_file_no_surface_starts_does_not_print_a_name_inside_an_unknown_key() {
    crate::config::install_canonical_renderer();
    let body = format!(
        "[ui]\nextra = {{ \"{TOKEN}\" = 1 }}\n\n[[projects]]\nid = \"a\"\npath = \"/a\"\n\
         [[projects]]\nid = \"a\"\npath = \"/b\"\n"
    );
    let said = get_all(&body, "ui");
    assert!(
        !said.contains(TOKEN),
        "get ui printed a name from inside an unknown [ui] key:\n{said}"
    );
}
