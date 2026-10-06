//! Names with dots, key migrations in `get`, a `[server]` that is not a
//! table, and providers already without a command.
use super::*;

fn temp_paths(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}

fn get_all(body: &str, path: &str) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let paths = temp_paths(tmp.path());
    std::fs::write(&paths.config_path, body).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let r = run_get(&[path.to_string()], &paths, &mut out, &mut err);
    format!(
        "r={:?}\nOUT:{}ERR:{}",
        r.err().map(|e| format!("{e:#}")),
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}

/// A rule-breaking name that contains a dot is printed in full, without
/// `--show`, on the correction line of `get` for the table holding it: the
/// correction's dotted path is split on dots again before the formatter
/// sees it, so the formatter judges the wrong segment as the name.
#[test]
fn a_rule_breaking_name_with_a_dot_is_never_printed_by_get() {
    for (body, path) in [
        (
            "[providers.\"ghp.SECRETTOKEN 123\"]\nargs = 5\n",
            "providers",
        ),
        ("[env]\n\"ghp.SECRETTOKEN 123\" = 5\n", "env"),
        ("[macros]\n\"ghp.SECRETTOKEN 123\" = 5\n", "macros"),
    ] {
        let printed = get_all(body, path);
        assert!(
            !printed.contains("SECRETTOKEN"),
            "`dux config get {path}` printed a name that breaks its rule:\n{printed}"
        );
    }
}

/// The terminal UI's start folds the retired `[keys] exit_interactive` into
/// `toggle_fullscreen` (and prunes retired actions), so the bindings it uses
/// differ from what the file says; `get keys` prints the file's table and
/// says nothing about the difference.
#[test]
fn get_keys_says_what_the_terminal_ui_uses_after_its_key_migrations() {
    let body = "[keys]\nexit_interactive = [\"ctrl-x\"]\n";
    // Precondition: the terminal UI starts with this file and uses
    // toggle_fullscreen = ["ctrl-x"], with no exit_interactive.
    let tmp = tempfile::tempdir().unwrap();
    let paths = temp_paths(tmp.path());
    std::fs::write(&paths.config_path, body).unwrap();
    let config = crate::config::ensure_config(&paths).expect("the terminal UI starts");
    crate::config::validate_keys(&config.keys).expect("its keys validate");
    let used = serde_json::to_value(&config.keys).unwrap();
    assert_eq!(used["toggle_fullscreen"], serde_json::json!(["ctrl-x"]));
    assert!(used.get("exit_interactive").is_none());

    let printed = get_all(body, "keys");
    assert!(
        printed.contains("toggle_fullscreen"),
        "`dux config get keys` does not say the terminal UI uses toggle_fullscreen = \
         [\"ctrl-x\"] in place of the file's exit_interactive:\n{printed}"
    );
}

/// `server = []` (an empty array where `[server]` goes) cannot hold a
/// password, and at 7437238c both the terminal UI and `dux server` started
/// with it (each reading it as an empty `[server]`). Both now refuse it,
/// and the start corpus neither covers it nor lists it as a deliberate
/// change. `server = 5` / `server = "x"` likewise started `dux server` then
/// (on the defaults) and stop it now, also unlisted.
#[test]
fn a_non_table_server_starts_as_it_did_at_7437238c() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = temp_paths(tmp.path());
    std::fs::write(&paths.config_path, "server = []\n").unwrap();
    let tui = crate::config::ensure_config(&paths)
        .map(|_| ())
        .map_err(|e| format!("{e:#}"));
    assert!(
        tui.is_ok(),
        "the terminal UI started with `server = []` at 7437238c: {tui:?}"
    );
    for body in ["server = []\n", "server = 5\n", "server = \"x\"\n"] {
        std::fs::write(&paths.config_path, body).unwrap();
        let server = dux_core::config::load_config(&paths)
            .map(|_| ())
            .map_err(|e| e.to_string());
        assert!(
            server.is_ok(),
            "dux server started with {body:?} at 7437238c: {server:?}"
        );
    }
}

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

fn set_on(body: &str, key: &str, value: &str) -> (Result<()>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = temp_paths(tmp.path());
    std::fs::write(&paths.config_path, body).unwrap();
    let mut out = Vec::new();
    let r = run_set(
        &[key.to_string(), value.to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    (r, std::fs::read_to_string(&paths.config_path).unwrap())
}

/// A provider the file lists without a command (which both surfaces start
/// with, or whose other field is broken) cannot have any other field set:
/// the provider-command check refuses every set of it, saying the set
/// "would leave" the provider with no command, when the file already had
/// none. So a broken field cannot be repaired with `set`, and a problem
/// already in the file blocks a set the set is not answerable for.
#[test]
fn a_provider_already_without_a_command_does_not_block_its_other_fields() {
    for (body, key, value) in [
        // Both surfaces start with this file (start corpus), and with the result.
        (
            "[providers.claude]\ninstall_hint = \"x\"\n",
            "providers.claude.install_hint",
            "y",
        ),
        // Repairing the one broken field.
        (
            "[providers.claude]\nargs = 5\n",
            "providers.claude.args",
            "[\"--a\"]",
        ),
        (
            "[providers.claude]\nforward_scroll = \"x\"\n",
            "providers.claude.forward_scroll",
            "true",
        ),
    ] {
        let (r, after) = set_on(body, key, value);
        assert!(
            r.is_ok(),
            "{body:?}: `dux config set {key} {value}` was refused: {:#}\n(file after: {after:?})",
            r.unwrap_err()
        );
    }
}
