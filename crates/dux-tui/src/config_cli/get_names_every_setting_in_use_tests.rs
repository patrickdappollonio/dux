//! `get` on a table, or on a setting the file holds, reports what dux uses.
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

fn get(paths: &DuxPaths, key: &str) -> Result<(String, String)> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], paths, &mut out, &mut err)?;
    Ok((
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    ))
}

/// With no `[providers]` in the file, dux runs its stock providers, and
/// `get providers.claude.command` says so ("claude ... dux uses this
/// value"); `get providers.claude` and `get providers` say nothing is set
/// and print nothing.
#[test]
fn get_of_a_stock_provider_table_reports_the_provider_dux_uses() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("");
    let (out, _) = get(&paths, "providers.claude.command").unwrap();
    assert_eq!(out, "claude\n", "precondition: the stock command is in use");
    let (out, err) = get(&paths, "providers.claude").unwrap();
    assert!(
        out.contains("claude"),
        "get providers.claude prints nothing although dux uses the stock claude provider: out={out:?} err={err:?}"
    );
}

/// A keybinding the file sets is a setting; `get` says it does not exist.
#[test]
fn get_of_a_keybinding_the_file_sets_reports_it() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[keys]\nquit = [\"ctrl-q\"]\n");
    let result = get(&paths, "keys.quit");
    assert!(result.is_ok(), "get keys.quit: {:#}", result.unwrap_err());
}

/// A macro the file defines is a setting; `get` says it does not exist.
#[test]
fn get_of_a_macro_the_file_defines_reports_it() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[macros.greet]\ntext = \"hello\"\nsurface = \"agent\"\n");
    let result = get(&paths, "macros.greet");
    assert!(
        result.is_ok(),
        "get macros.greet: {:#}",
        result.unwrap_err()
    );
}
