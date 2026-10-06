//! `get` on a keybinding reports the binding the terminal UI uses.
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

/// The terminal UI folds a legacy `exit_interactive` binding into
/// `toggle_fullscreen` when it loads the file. `get keys.toggle_fullscreen`
/// must report the merged binding the terminal UI then uses (or say it uses
/// another), not the file's own list as if nothing changed.
#[test]
fn get_of_a_binding_the_terminal_ui_merges_a_legacy_one_into_reports_the_merge() {
    crate::config::install_canonical_renderer();
    let body = "[keys]\nexit_interactive = [\"ctrl-x\"]\ntoggle_fullscreen = [\"ctrl-g\"]\n";
    // Precondition: the terminal UI starts with the file and binds both keys.
    {
        let (_t, p) = paths_with(body);
        let config = crate::config::ensure_config(&p).expect("terminal UI starts");
        crate::config::validate_keys(&config.keys).expect("keys valid");
        assert_eq!(
            config.keys.bindings.get("toggle_fullscreen").cloned(),
            Some(vec!["ctrl-g".to_string(), "ctrl-x".to_string()]),
            "precondition: the terminal UI uses the merged binding"
        );
    }
    let (_tmp, paths) = paths_with(body);
    let (out, err) = get(&paths, "keys.toggle_fullscreen");
    assert!(
        out.contains("ctrl-x") || err.contains("ctrl-x"),
        "get keys.toggle_fullscreen says the file's [\"ctrl-g\"] with no word that the terminal \
         UI uses [\"ctrl-g\", \"ctrl-x\"]: out={out:?} err={err:?}"
    );
}

/// With only the legacy row, `get keys.toggle_fullscreen` says the binding
/// "is not set", while the terminal UI binds ctrl-x to it.
#[test]
fn get_of_a_binding_renamed_from_a_legacy_one_is_not_reported_unset() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[keys]\nexit_interactive = [\"ctrl-x\"]\n");
    let (out, err) = get(&paths, "keys.toggle_fullscreen");
    assert!(
        !err.contains("is not set"),
        "get keys.toggle_fullscreen says it is not set, but the terminal UI binds ctrl-x to it: \
         out={out:?} err={err:?}"
    );
}

/// With no `[keys]` in the file the terminal UI runs every action on its
/// default binding; `get keys.quit` must report that default, not "not set".
#[test]
fn get_of_a_keybinding_left_out_reports_the_default_the_terminal_ui_uses() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("");
    let bindings = crate::keybindings::RuntimeBindings::from_keys_config(
        &dux_core::config::KeysConfig::default(),
    );
    let used = bindings.labels_for(crate::keybindings::Action::Quit);
    assert!(!used.is_empty(), "precondition: quit has a default binding");
    let (out, err) = get(&paths, "keys.quit");
    assert!(
        !out.trim().is_empty(),
        "get keys.quit prints nothing (stderr {err:?}) while the terminal UI quits on {used}"
    );
}

/// A file that lists one provider of its own: dux runs the stock providers
/// beside it (the load fills them in), and `get providers.claude.command`
/// says so. `get providers` must report the providers in use, the stock ones
/// included, not only the one the file writes.
#[test]
fn get_of_the_providers_table_reports_the_stock_providers_in_use_beside_a_custom_one() {
    crate::config::install_canonical_renderer();
    let (_tmp, paths) = paths_with("[providers.mytool]\ncommand = \"mytool\"\n");
    let (leaf, _) = get(&paths, "providers.claude.command");
    assert_eq!(
        leaf, "claude\n",
        "precondition: dux uses the stock claude provider"
    );
    let (out, err) = get(&paths, "providers");
    assert!(
        out.contains("claude"),
        "get providers prints only the file's own provider while dux runs claude too: \
         out={out:?} err={err:?}"
    );
}

/// The same for a table of plain settings: with one `[ui]` setting in the
/// file, `get ui` must still report the defaults dux uses for the rest, as
/// it does when `[ui]` is absent.
#[test]
fn get_of_a_partly_written_table_reports_the_defaults_in_use_for_the_rest() {
    crate::config::install_canonical_renderer();
    let (_tmp, empty) = paths_with("");
    let (whole, _) = get(&empty, "ui");
    assert!(
        whole.contains("terminal_font_size"),
        "precondition: {whole:?}"
    );
    let (_tmp2, paths) = paths_with("[ui]\nleft_width_pct = 30\n");
    let (out, err) = get(&paths, "ui");
    assert!(
        out.contains("terminal_font_size"),
        "get ui prints only left_width_pct and none of the defaults dux uses for the rest of \
         [ui]: out={out:?} err={err:?}"
    );
}
