//! A name the file writes that is not a setting name (a token pasted where a
//! key goes) never reaches any printer without `--show`: the start checks,
//! both surfaces' start errors, `get` of every table (body and corrections),
//! `set`'s messages and `dux config diff`. Generated tokens are placed in
//! every structural position: each fixed table, each map of user-chosen
//! names, under values that are not tables, and under misplaced password
//! hashes.
use super::*;
use dux_core::start_check_fixtures::{
    NAME_POSITIONS as POSITIONS, VALUE_POSITIONS, name_tokens as tokens,
};

/// Every table `get` is asked for.
const TABLES: &[&str] = &[
    "ui",
    "defaults",
    "logging",
    "capabilities",
    "editor",
    "terminal",
    "startup_command_terminal",
    "server",
    "server.auth",
    "env",
    "providers",
    "providers.claude",
    "providers.mytool",
    "macros",
    "keys",
    "projects",
];

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

/// Everything dux prints about `body`, printer by printer.
fn printed(body: &str) -> Vec<(String, String)> {
    let mut said = Vec::new();
    for problem in dux_core::config::start_problems_of(body) {
        said.push(("start problem".to_string(), problem.message));
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, body).expect("seed");
    if let Err(error) = crate::config::ensure_config(&paths) {
        said.push(("terminal UI start".to_string(), format!("{error:#}")));
    }
    std::fs::write(&paths.config_path, body).expect("seed");
    if let Err(error) = dux_core::config::load_config(&paths) {
        said.push(("dux server start".to_string(), format!("{error:#}")));
    }
    for table in TABLES {
        std::fs::write(&paths.config_path, body).expect("seed");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let result = run_get(&[table.to_string()], &paths, &mut out, &mut err);
        let mut text = String::from_utf8_lossy(&out).into_owned();
        text.push_str(&String::from_utf8_lossy(&err));
        if let Err(error) = result {
            text.push_str(&format!("{error:#}"));
        }
        said.push((format!("get {table}"), text));
    }
    std::fs::write(&paths.config_path, body).expect("seed");
    let mut out = Vec::new();
    let result = run_set(
        &["ui.left_width_pct".to_string(), "30".to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if let Err(error) = result {
        text.push_str(&format!("{error:#}"));
    }
    said.push(("set".to_string(), text));
    // A set over a setting the file may have written as a table, or with a
    // table in it, reports what it replaced.
    std::fs::write(&paths.config_path, body).expect("seed");
    let mut out = Vec::new();
    let result = run_set(
        &["ui.theme".to_string(), "dux_dark".to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if let Err(error) = result {
        text.push_str(&format!("{error:#}"));
    }
    said.push(("set ui.theme".to_string(), text));
    if let Ok(mut config) = toml::from_str::<dux_core::config::Config>(body) {
        config.source_text = dux_core::config::SourceText::of(body);
        said.push((
            "config diff".to_string(),
            crate::cli::collect_config_changes(&config).join("\n"),
        ));
    }
    said
}

#[test]
fn a_name_that_is_not_a_setting_name_never_reaches_a_printer() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        for position in POSITIONS {
            let body = position.replace("{T}", bare);
            assert!(toml::from_str::<toml::Table>(&body).is_ok(), "{body}");
            for (printer, text) in printed(&body) {
                if text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}

/// A VALUE below a key the formatter does not name is never printed either:
/// nothing at or below a hidden key reaches a printer without `--show`.
#[test]
fn a_value_below_a_hidden_key_never_reaches_a_printer() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        for position in VALUE_POSITIONS {
            let body = position.replace("{V}", bare);
            assert!(toml::from_str::<toml::Table>(&body).is_ok(), "{body}");
            for (printer, text) in printed(&body) {
                if text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}
