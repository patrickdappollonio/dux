//! `get` marks only true defaults as defaults, and credits a value only to a
//! surface that starts with the file.
use super::*;

fn paths_with(body: &str) -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
        socket_path: tmp.path().join("dux.sock"),
    };
    std::fs::write(&paths.config_path, body).expect("seed");
    (tmp, paths)
}

fn get(paths: &DuxPaths, key: &str) -> (String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], paths, &mut out, &mut err).expect("get");
    (
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

/// `get server` on a file whose only server setting is the deprecated
/// `bind`: dux serves on 10.0.0.1:4000, carried over from it, yet the table
/// marks both values `# default` (the defaults are 127.0.0.1 and 3890).
#[test]
fn get_of_a_table_never_marks_a_value_carried_from_a_deprecated_key_as_a_default() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[server]\nbind = \"10.0.0.1:4000\"\n");
    let (out, err) = get(&paths, "server");
    for line in out.lines() {
        if line.starts_with("host =") || line.starts_with("port =") {
            assert!(
                !line.contains("# default"),
                "`get server` marks {line:?} as a default, but it is carried over from \
                 [server] bind and differs from the default:\n{out}\n{err}"
            );
        }
    }
}

/// The same for the terminal UI's own key resolution: a legacy
/// `exit_interactive` binding folded into `toggle_fullscreen` is printed as
/// `toggle_fullscreen = ["ctrl-x"] # default`, while its default is ctrl-g.
#[test]
fn get_keys_never_marks_a_binding_folded_from_a_legacy_action_as_a_default() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[keys]\nexit_interactive = [\"ctrl-x\"]\n");
    let (out, err) = get(&paths, "keys");
    let line = out
        .lines()
        .find(|line| line.starts_with("toggle_fullscreen ="))
        .expect("toggle_fullscreen is listed");
    assert!(
        line.contains("ctrl-x") && !line.contains("# default"),
        "`get keys` says {line:?}: ctrl-x comes from the file's legacy exit_interactive, the \
         default is ctrl-g:\n{err}"
    );
}

/// With a file the terminal UI refuses, `get keys.show_terminal_keys` says
/// both that the terminal UI uses the value and that it has no value in use.
#[test]
fn get_of_a_key_setting_never_says_a_refusing_terminal_ui_uses_a_value() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[env]\nFOO = \"a\\u0000b\"\n");
    for key in ["keys", "keys.show_terminal_keys"] {
        let (out, err) = get(&paths, key);
        assert!(
            err.contains("the terminal UI will not start with this file"),
            "precondition: {err}"
        );
        assert!(
            !err.contains("the terminal UI uses this value"),
            "`get {key}` says the terminal UI uses a value it will not start with:\nOUT {out}\nERR {err}"
        );
    }
}
