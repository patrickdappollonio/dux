//! A plaintext password in the file is never printed, and `set` refuses a
//! theme the terminal UI would replace with its default.
use super::*;

const S: &str = "PLAINSECRETzz9q";

fn paths_in(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
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
fn get_all(body: &str, args: &[&str]) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, body).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_get(&args, &paths, &mut out, &mut err);
    format!(
        "{r:?}\nOUT:{}\nERR:{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}
/// A plaintext password typed into config.toml is never printed by `get`
/// without --show, wherever it sits.
#[test]
fn get_never_prints_a_plaintext_password_without_show() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for (body, key) in [
        (
            format!("[server.auth]\npassword = \"{S}\"\n"),
            "server.auth",
        ),
        (format!("[server.auth]\npassword = \"{S}\"\n"), "server"),
        (
            format!("[server]\nauth = {{ password = \"{S}\" }}\n"),
            "server",
        ),
        (format!("server.auth.password = \"{S}\"\n"), "server.auth"),
        (
            format!("[server.auth]\nPassword = \"{S}\"\n"),
            "server.auth",
        ),
        (
            format!("[server.auth]\npass-word = \"{S}\"\n"),
            "server.auth",
        ),
        (format!("[server]\npassword = \"{S}\"\n"), "server"),
        (
            format!("[auth]\nusername = \"u\"\npassword = \"{S}\"\n"),
            "server",
        ),
    ] {
        let said = get_all(&body, &[key]);
        if said.contains(S) {
            leaks.push(format!("get {key} on {body:?}:\n{said}"));
        }
    }
    assert!(
        leaks.is_empty(),
        "plaintext password printed without --show:\n{}",
        leaks.join("\n----\n")
    );
}

/// `get server.auth.password` on a file that has one says it is in the file.
#[test]
fn get_the_password_does_not_deny_the_line_the_file_has() {
    crate::config::install_canonical_renderer();
    let said = get_all(
        &format!("[server.auth]\npassword = \"{S}\"\n"),
        &["server.auth.password"],
    );
    assert!(
        !said.contains("server.auth.password is not in config.toml"),
        "{said}"
    );
}

/// `set` refuses a value dux resets at use time; a theme that does not exist
/// is replaced by the default theme the moment the terminal UI loads it.
#[test]
fn set_refuses_a_theme_the_terminal_ui_replaces_with_its_default() {
    crate::config::install_canonical_renderer();
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, "[ui]\nleft_width_pct = 25\n").unwrap();
    let mut out = Vec::new();
    let result = run_set(
        &["ui.theme".into(), "no-such-theme-exists".into()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    let config = crate::config::ensure_config(&paths).expect("starts");
    let (_theme, warning) = crate::theme::load_or_fallback(&config.ui.theme, &paths);
    assert!(
        result.is_err() || warning.is_none(),
        "set accepted a theme the terminal UI does not use ({:?}):\n{}",
        warning,
        String::from_utf8_lossy(&out)
    );
}

/// `get ui.theme` reports the theme the terminal UI draws with: its default
/// for a name it has no theme by, with why; a built-in name as written.
#[test]
fn get_reports_the_theme_the_terminal_ui_draws_with() {
    crate::config::install_canonical_renderer();
    let said = get_all("[ui]\ntheme = \"no-such-theme-exists\"\n", &["ui.theme"]);
    assert!(said.contains("OUT:dux_dark"), "{said}");
    assert!(
        said.contains("the terminal UI has no theme by this name"),
        "{said}"
    );
    let said = get_all("[ui]\ntheme = \"nord\"\n", &["ui.theme"]);
    assert!(said.contains("OUT:nord"), "{said}");
}
