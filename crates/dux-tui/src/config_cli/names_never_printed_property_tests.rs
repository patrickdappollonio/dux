//! A name the file writes that is not a setting name (a token pasted where a
//! key goes) never reaches any printer without `--show`: the start checks,
//! both surfaces' start errors, `get` of every table (body and corrections),
//! `set`'s messages and `dux config diff`. Generated tokens are placed in
//! every structural position: each fixed table, each map of user-chosen
//! names, under values that are not tables, and under misplaced password
//! hashes.
use super::*;
use dux_core::start_check_fixtures::{
    BINDING_VALUE_POSITIONS, BROKEN_PLAINTEXT_PASSWORD_FILES, NAME_POSITIONS as POSITIONS,
    PLAINTEXT_PASSWORD_POSITIONS, PROJECT_VALUE_POSITIONS, VALUE_POSITIONS, name_tokens as tokens,
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
        socket_path: dir.join("dux.sock"),
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
    said.extend(previews(body, false));
    said.extend(listings(&paths, body, false));
    said
}

/// What `dux env ls`, `dux macros ls` and `dux providers ls` print about
/// `body` with no dux running, `show` passed to `env ls`, in every shape.
fn listings(paths: &DuxPaths, body: &str, show: bool) -> Vec<(String, String)> {
    use dux_core::client::config_resources::{Source, env_ls, macros_ls, providers_ls};
    use dux_core::client::output::Shape;
    let mut said = Vec::new();
    for shape in [Shape::Table, Shape::Json, Shape::Ids] {
        std::fs::write(&paths.config_path, body).expect("seed");
        let source = Source::File(paths);
        for (printer, result) in [
            (
                format!("env ls (show {show})"),
                env_ls(&source, show, shape),
            ),
            ("macros ls".to_string(), macros_ls(&source, shape)),
            ("providers ls".to_string(), providers_ls(&source, shape)),
        ] {
            let text = match result {
                Ok(text) => text,
                Err(error) => error.message,
            };
            said.push((format!("{printer} {shape:?}"), text));
        }
    }
    said
}

/// What the command line prints about `name` when it is typed as a provider,
/// a macro or an environment variable with no dux running: `providers show`,
/// `macros show`, the questions `env set` and `env rm` ask, `env set`'s name
/// check, and both changes on the file.
fn named(paths: &DuxPaths, body: &str, name: &str) -> Vec<(String, String)> {
    use dux_core::client::config_resources::{
        Source, Writer, check_env_name, macros_show, providers_show, remove_env,
        remove_env_question, set_env, set_env_question,
    };
    let text = |result: Result<String, dux_core::client::CliError>| match result {
        Ok(text) => text,
        Err(error) => error.message,
    };
    let mut said = vec![
        ("env set question".to_string(), set_env_question(name)),
        ("env rm question".to_string(), remove_env_question(name)),
        (
            "env set name check".to_string(),
            text(check_env_name(name).map(|()| String::new())),
        ),
    ];
    std::fs::write(&paths.config_path, body).expect("seed");
    said.push((
        "providers show".to_string(),
        text(providers_show(&Source::File(paths), name)),
    ));
    std::fs::write(&paths.config_path, body).expect("seed");
    said.push((
        "macros show".to_string(),
        text(macros_show(&Source::File(paths), name)),
    ));
    std::fs::write(&paths.config_path, body).expect("seed");
    said.push((
        "env set".to_string(),
        text(set_env(Writer::File(paths), name, "v")),
    ));
    std::fs::write(&paths.config_path, body).expect("seed");
    said.push((
        "env rm".to_string(),
        text(remove_env(Writer::File(paths), name)),
    ));
    said
}

/// `dux env ls` prints an environment value only with `--show`.
#[test]
fn env_ls_prints_a_value_only_with_show() {
    crate::config::install_canonical_renderer();
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    let body = "[env]\nAPI_TOKEN = \"zzSECRETvalue\"\n";
    for (printer, text) in listings(&paths, body, false) {
        assert!(!text.contains("zzSECRETvalue"), "{printer}: {text}");
    }
    let shown: Vec<String> = listings(&paths, body, true)
        .into_iter()
        .filter(|(printer, _)| printer.starts_with("env ls") && !printer.ends_with("Ids"))
        .map(|(_, text)| text)
        .collect();
    assert_eq!(shown.len(), 2);
    assert!(
        shown.iter().all(|text| text.contains("zzSECRETvalue")),
        "{shown:?}"
    );
}

/// dux's own documented file with `line` added where a user adds one: at
/// its very end, and first inside `[macros]` (which it used to end in) and
/// `[keys]`, wherever those now sit.
fn documented_positions(line: &str) -> Vec<String> {
    crate::config::install_canonical_renderer();
    let base = crate::config::render_default_config();
    let mut positions = vec![format!("{base}\n{line}\n")];
    for section in ["macros", "keys"] {
        let header = format!("\n[{section}]\n");
        let at = base
            .find(&header)
            .expect("the documented file has the section")
            + header.len();
        positions.push(format!("{}{line}\n{}", &base[..at], &base[at..]));
    }
    positions
}

/// The previews of `dux config regenerate` and `dux config restore-docs`,
/// with `show` as given, as each prints them.
fn previews(body: &str, show: bool) -> Vec<(String, String)> {
    let mut said = vec![(
        format!("regenerate preview (show {show})"),
        crate::cli::regenerate_preview(body, &crate::config::render_default_config(), show),
    )];
    if let Ok(restored) = crate::config::restore_documentation(body) {
        said.push((
            format!("restore-docs preview (show {show})"),
            crate::cli::restore_docs_preview(body, &restored.text, show),
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
            // `env ls --show` prints values, and still no name it hides.
            let tmp = tempfile::tempdir().expect("tempdir");
            let paths = paths_in(tmp.path());
            let mut shown = listings(&paths, &body, true);
            // The name typed on the command line, as the file has it.
            shown.extend(named(&paths, &body, &token));
            for (printer, text) in printed(&body).into_iter().chain(shown) {
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

/// A `[keys]` binding's value is printed where a setting's value is the
/// point (`get keys` and `dux config diff`, which print what the file sets,
/// as they print any setting's value) and nowhere else: a binding that is not
/// a key dux understands (a token pasted there) is named by its action and
/// line in every problem, refusal and `set` message.
#[test]
fn a_binding_value_reaches_no_printer_but_the_value_printers() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        for position in BINDING_VALUE_POSITIONS {
            let body = position.replace("{V}", bare);
            assert!(toml::from_str::<toml::Table>(&body).is_ok(), "{body}");
            for (printer, text) in printed(&body) {
                let prints_values = printer == "get keys" || printer == "config diff";
                if !prints_values && text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}

/// A project's path, name and id reach no printer without `--show`: every
/// problem names a project by its line, and `get projects` and `dux config
/// diff` summarize projects.
#[test]
fn a_project_value_reaches_no_printer() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        // A path and an id can hold the token's characters but a quote.
        let plain: String = token
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        for position in PROJECT_VALUE_POSITIONS {
            let body = position.replace("{V}", &plain);
            assert!(toml::from_str::<toml::Table>(&body).is_ok(), "{body}");
            for (printer, text) in printed(&body) {
                if text.contains(&plain[..12]) || text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}

/// Everything dux prints about `body` with `--show`, where a printer takes
/// it: every table `get` is asked for, the password itself, both previews,
/// and `dux config diff --raw`.
fn printed_with_show(body: &str) -> Vec<(String, String)> {
    let mut said = previews(body, true);
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    said.extend(listings(&paths, body, true));
    let mut asks: Vec<Vec<&str>> = TABLES.iter().map(|table| vec![*table, "--show"]).collect();
    asks.push(vec!["server.auth.password"]);
    asks.push(vec!["server.auth.password", "--show"]);
    for args in asks {
        std::fs::write(&paths.config_path, body).expect("seed");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let result = run_get(&args, &paths, &mut out, &mut err);
        let mut text = String::from_utf8_lossy(&out).into_owned();
        text.push_str(&String::from_utf8_lossy(&err));
        if let Err(error) = result {
            text.push_str(&format!("{error:#}"));
        }
        said.push((format!("get {}", args.join(" ")), text));
    }
    if let Ok(config) = toml::from_str::<dux_core::config::Config>(body) {
        said.push((
            "config diff --raw".to_string(),
            crate::cli::raw_diff_text(&config),
        ));
    }
    said
}

/// A plaintext password written in the file reaches no printer in any shape
/// dux's own start check calls one, and `--show` makes no exception for it.
#[test]
fn a_plaintext_password_reaches_no_printer_even_with_show() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        let positions = PLAINTEXT_PASSWORD_POSITIONS
            .iter()
            .map(|position| (*position).to_string())
            .chain(documented_positions("password = \"{V}\""));
        for position in positions {
            let body = position.replace("{V}", bare);
            assert!(toml::from_str::<toml::Table>(&body).is_ok(), "{body}");
            assert!(
                !dux_core::config::plaintext_password_problems(&body).is_empty(),
                "the start check does not call {body:?} a plaintext password"
            );
            let mut said = printed(&body);
            said.extend(printed_with_show(&body));
            for (printer, text) in said {
                if text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}

/// A file that is not TOML prints none of its text anywhere, `--show`
/// included: dux cannot tell where a secret sits in it.
#[test]
fn a_file_that_is_not_toml_reaches_no_printer_even_with_show() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for token in tokens() {
        let fragment = &token[..12];
        let quoted = toml::Value::String(token.clone()).to_string();
        let bare = &quoted[1..quoted.len() - 1];
        for broken in BROKEN_PLAINTEXT_PASSWORD_FILES {
            let body = broken.replace("{V}", bare);
            assert!(toml::from_str::<toml::Table>(&body).is_err(), "{body}");
            let mut said = printed(&body);
            said.extend(printed_with_show(&body));
            for (printer, text) in said {
                if text.contains(fragment) {
                    leaks.push(format!("{printer} on {body:?}:\n{text}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n---\n"));
}
