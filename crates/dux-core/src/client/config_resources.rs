//! `dux macros`, `providers`, `keys`, `themes` and `env`: the resources kept
//! in `config.toml`.
//!
//! On this machine they are read from the file itself, whether or not dux is
//! running. Through a remote they come from that dux's API: macros and the
//! environment from `GET /api/v1/bootstrap`, the rest from
//! `GET /api/v1/config/{providers,keys,themes}`. Every listing is built by
//! [`crate::config_resources`], the same code the API answers with.
//!
//! A change goes to a running dux through its one-entry routes, so it applies
//! at once; with no dux running it edits the file.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

use super::connect::Client;
use super::output::{self, Listing, Row, Shape};
use super::transport::Method;
use super::{CliError, Exit};
use crate::config::DuxPaths;
use crate::config_keys::NOT_SHOWN;
use crate::config_resources::{
    self as resources, EnvItem, KeyItem, MacroItem, ProviderItem, ThemeItem,
};

/// Where a listing is read from.
pub enum Source<'a> {
    /// This machine's `config.toml`.
    File(&'a DuxPaths),
    /// A dux's API.
    Dux(&'a Client),
}

fn failed(message: impl Into<String>) -> CliError {
    CliError::new(Exit::Failed, message)
}

fn file(paths: &DuxPaths) -> Result<resources::ConfigFile, CliError> {
    resources::read(&paths.config_path).map_err(failed)
}

#[derive(Deserialize)]
struct Bootstrap {
    macros: Vec<BootstrapMacro>,
    global_env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct BootstrapMacro {
    name: String,
    text: String,
    surface: String,
}

fn macro_items(source: &Source<'_>) -> Result<Vec<MacroItem>, CliError> {
    match source {
        Source::File(paths) => Ok(resources::macros_of(&file(paths)?)),
        Source::Dux(client) => {
            let boot: Bootstrap = client.get_json("/api/v1/bootstrap")?;
            Ok(resources::macros(
                "",
                boot.macros.into_iter().map(|m| (m.name, m.text, m.surface)),
            ))
        }
    }
}

fn env_items(source: &Source<'_>, show: bool) -> Result<Vec<EnvItem>, CliError> {
    match source {
        Source::File(paths) => {
            let file = file(paths)?;
            Ok(resources::env(&file.raw, &file.config.env, show))
        }
        Source::Dux(client) => {
            let boot: Bootstrap = client.get_json("/api/v1/bootstrap")?;
            Ok(resources::env("", &boot.global_env, show))
        }
    }
}

fn provider_items(source: &Source<'_>) -> Result<Vec<ProviderItem>, CliError> {
    match source {
        Source::File(paths) => Ok(resources::providers(&file(paths)?)),
        Source::Dux(client) => client.get_json("/api/v1/config/providers"),
    }
}

fn key_items(source: &Source<'_>) -> Result<Vec<KeyItem>, CliError> {
    match source {
        Source::File(paths) => resources::keys(&file(paths)?.raw).map_err(failed),
        Source::Dux(client) => client.get_json("/api/v1/config/keys"),
    }
}

fn theme_items(source: &Source<'_>) -> Result<Vec<ThemeItem>, CliError> {
    match source {
        Source::File(paths) => {
            let file = file(paths)?;
            Ok(resources::themes(paths, &file.config.ui.theme))
        }
        Source::Dux(client) => client.get_json("/api/v1/config/themes"),
    }
}

/// What an item is looked up by: the file's own name (`key`), or for one a
/// running dux listed, which carries none, the name it was listed under.
fn lookup<'a>(key: &'a str, listed: &'a str) -> &'a str {
    if key.is_empty() { listed } else { key }
}

fn macro_key(item: &MacroItem) -> &str {
    lookup(&item.key, &item.name)
}

fn provider_key(item: &ProviderItem) -> &str {
    lookup(&item.key, &item.name)
}

/// The most of a macro's first line a table shows.
const TEXT_CELL_CHARS: usize = 48;

/// A macro's text as its table cell: the first line, cut to
/// [`TEXT_CELL_CHARS`] characters, with `…` where anything was left out.
fn text_cell(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    let mut cell: String = first.chars().take(TEXT_CELL_CHARS).collect();
    let more_lines = !text[first.len()..].trim().is_empty();
    if first.chars().count() > TEXT_CELL_CHARS || more_lines {
        cell.push('…');
    }
    cell
}

fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// `dux macros ls`.
pub fn macros_ls(source: &Source<'_>, shape: Shape) -> Result<String, CliError> {
    let rows = macro_items(source)?
        .into_iter()
        .map(|item| Row {
            id: item.name.clone(),
            cells: vec![
                item.name.clone(),
                item.surface.clone(),
                item.text
                    .as_deref()
                    .map_or(NOT_SHOWN.to_string(), text_cell),
            ],
            json: to_json(&item),
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["NAME", "SURFACE", "TEXT"],
            rows,
        },
        shape,
    ))
}

/// `dux macros show <name>`.
pub fn macros_show(source: &Source<'_>, name: &str) -> Result<String, CliError> {
    let items = macro_items(source)?;
    let shown = resources::shown_name("", "macros", name).0;
    let item = output::select_shown("macro", name, &shown, &items, macro_key, macro_key)?;
    Ok(output::details(&to_json(item)))
}

/// `dux env ls`: names only, unless `show`.
pub fn env_ls(source: &Source<'_>, show: bool, shape: Shape) -> Result<String, CliError> {
    let rows = env_items(source, show)?
        .into_iter()
        .map(|item| {
            let mut cells = vec![item.name.clone()];
            cells.extend(item.value.clone());
            Row {
                id: item.name.clone(),
                cells,
                json: to_json(&item),
            }
        })
        .collect();
    let headers = if show {
        vec!["NAME", "VALUE"]
    } else {
        vec!["NAME"]
    };
    Ok(output::render(&Listing { headers, rows }, shape))
}

/// `dux providers ls`.
pub fn providers_ls(source: &Source<'_>, shape: Shape) -> Result<String, CliError> {
    let rows = provider_items(source)?
        .into_iter()
        .map(|item| Row {
            id: item.name.clone(),
            cells: vec![
                item.name.clone(),
                item.settings
                    .as_ref()
                    .map_or(NOT_SHOWN.to_string(), |s| s.command.clone()),
                item.source.clone(),
            ],
            json: provider_json(&item),
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["NAME", "COMMAND", "SOURCE"],
            rows,
        },
        shape,
    ))
}

/// A provider as one flat object: its name and source, then its settings.
fn provider_json(item: &ProviderItem) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    object.insert("name".into(), item.name.clone().into());
    object.insert("source".into(), item.source.clone().into());
    if let Some(serde_json::Value::Object(settings)) = item.settings.as_ref().map(to_json) {
        object.extend(settings);
    }
    serde_json::Value::Object(object)
}

/// `dux providers show <name>`.
pub fn providers_show(source: &Source<'_>, name: &str) -> Result<String, CliError> {
    let items = provider_items(source)?;
    let shown = resources::shown_name("", "providers", name).0;
    let item = output::select_shown("provider", name, &shown, &items, provider_key, provider_key)?;
    Ok(output::details(&provider_json(item)))
}

/// `dux keys ls`.
pub fn keys_ls(source: &Source<'_>, shape: Shape) -> Result<String, CliError> {
    let rows = key_items(source)?
        .into_iter()
        .map(|item| Row {
            id: item.action.clone(),
            cells: vec![
                item.action.clone(),
                item.keys.join(", "),
                item.source.clone(),
            ],
            json: to_json(&item),
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["ACTION", "KEYS", "SOURCE"],
            rows,
        },
        shape,
    ))
}

/// `dux themes ls`.
pub fn themes_ls(source: &Source<'_>, shape: Shape) -> Result<String, CliError> {
    let rows = theme_items(source)?
        .into_iter()
        .map(|item| Row {
            id: item.name.clone(),
            cells: vec![
                item.name.clone(),
                item.source.clone(),
                if item.current { "yes" } else { "" }.to_string(),
            ],
            json: to_json(&item),
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["NAME", "SOURCE", "CURRENT"],
            rows,
        },
        shape,
    ))
}

// ---------------------------------------------------------------------------
// Changes
// ---------------------------------------------------------------------------

/// Where a change goes, and how long it waits for a running dux.
pub enum Writer<'a> {
    /// No dux is running on this machine: the change edits `config.toml`.
    File(&'a DuxPaths),
    /// A running dux applies it. `wait` is how long to wait for its outcome;
    /// `None` returns the operation's id at once.
    Dux {
        client: &'a Client,
        wait: Option<Duration>,
    },
}

/// What segment escapes in a path: everything but the URL's unreserved marks.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn entry_path(collection: &str, name: &str) -> String {
    format!(
        "/api/v1/{collection}/{}",
        percent_encoding::utf8_percent_encode(name, SEGMENT)
    )
}

/// Send one change to a running dux and say how it ended.
fn through_dux(
    client: &Client,
    wait: Option<Duration>,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<String, CliError> {
    let record = client.change(method, path, body)?;
    let Some(timeout) = wait else {
        return Ok(format!("{}\n", record.id));
    };
    let record = client.wait(record, timeout)?;
    Ok(format!("{}\n", super::wait::outcome(&record)?))
}

/// What a file edit adds about when it takes effect.
const APPLIES_AT_START: &str = "dux is not running, so it applies the next time dux starts.";

fn edit_failed(error: anyhow::Error) -> CliError {
    failed(format!("{error:#}"))
}

/// What `dux macros add` asks before it changes anything.
pub fn set_macro_question(name: &str) -> String {
    format!("Save macro {}", resources::macro_label(name))
}

/// What `dux macros rm` asks before it changes anything.
pub fn remove_macro_question(name: &str) -> String {
    format!("Remove macro {}", resources::macro_label(name))
}

/// `dux macros add`: add the macro, or replace the one with that name.
pub fn set_macro(
    writer: Writer<'_>,
    name: &str,
    text: String,
    surface: &str,
) -> Result<String, CliError> {
    let label = resources::macro_label(name.trim());
    let result = match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Put,
            &entry_path("macros", name),
            Some(serde_json::json!({ "text": text, "surface": surface })),
        )
        .map(|said| own_outcome(said, wait, format!("Saved macro {label}.\n"))),
        Writer::File(paths) => resources::set_macro_in_file(paths, name, text, surface)
            .map_err(edit_failed)
            .map(|_| {
                format!(
                    "Saved macro {label} in {}. {APPLIES_AT_START}\n",
                    paths.config_path.display()
                )
            }),
    };
    result.map_err(|error| macro_named_by_label(name, error))
}

/// `dux macros rm`.
pub fn remove_macro(writer: Writer<'_>, name: &str) -> Result<String, CliError> {
    let label = resources::macro_label(name);
    let result = match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Delete,
            &entry_path("macros", name),
            None,
        )
        .map(|said| own_outcome(said, wait, format!("Removed macro {label}.\n"))),
        Writer::File(paths) => resources::remove_macro_in_file(paths, name)
            .map_err(edit_failed)
            .map(|()| {
                format!(
                    "Removed macro {label} from {}. {APPLIES_AT_START}\n",
                    paths.config_path.display()
                )
            }),
    };
    result.map_err(|error| macro_named_by_label(name, error))
}

/// A macro change through a running dux prints its operation's id when it did not wait, else
/// `done`, never the record's sentence, which names the macro as given whatever its name.
fn own_outcome(said: String, wait: Option<Duration>, done: String) -> String {
    if wait.is_none() { said } else { done }
}

/// `error` with the macro `name` replaced by its label when the name is outside the `[macros]`
/// rule: a running dux and the shared checks name a macro as given.
fn macro_named_by_label(name: &str, mut error: CliError) -> CliError {
    let (label, hidden) = resources::shown_name("", "macros", name.trim());
    if hidden {
        for given in [name, name.trim()] {
            if !given.is_empty() {
                error.message = error.message.replace(given, &label);
            }
        }
    }
    error
}

/// What `dux env set` asks before it changes anything.
pub fn set_env_question(name: &str) -> String {
    format!(
        "Set {} in the global environment",
        resources::env_label("", name)
    )
}

/// What `dux env rm` asks before it changes anything. A remote's file is not visible here, so
/// the label is judged on the name alone.
pub fn remove_env_question(name: &str) -> String {
    format!(
        "Remove {} from the global environment",
        resources::env_label("", name)
    )
}

/// This machine's config file as it is, empty when it cannot be read.
fn file_text(paths: &DuxPaths) -> String {
    std::fs::read_to_string(&paths.config_path).unwrap_or_default()
}

/// `dux env set` refuses a name that is no variable name before it asks or
/// sends anything, without repeating it.
pub fn check_env_name(name: &str) -> Result<(), CliError> {
    // An empty value passes every value check, so only the name is judged.
    crate::config::check_global_env_var(name, "").map_err(edit_failed)
}

/// `dux env set`. The value is never printed.
pub fn set_env(writer: Writer<'_>, name: &str, value: &str) -> Result<String, CliError> {
    check_env_name(name)?;
    match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Put,
            &entry_path("global-env", name),
            Some(serde_json::json!({ "value": value })),
        ),
        Writer::File(paths) => {
            resources::set_env_in_file(paths, name, value).map_err(edit_failed)?;
            Ok(format!(
                "Saved global environment variable {name} in {} (the value is not shown). \
                 {APPLIES_AT_START}\n",
                paths.config_path.display()
            ))
        }
    }
}

/// `dux env rm`.
pub fn remove_env(writer: Writer<'_>, name: &str) -> Result<String, CliError> {
    match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Delete,
            &entry_path("global-env", name),
            None,
        ),
        Writer::File(paths) => {
            let label = resources::env_label(&file_text(paths), name);
            resources::remove_env_in_file(paths, name).map_err(edit_failed)?;
            Ok(format!(
                "Removed {label} from the global environment in {}. {APPLIES_AT_START}\n",
                paths.config_path.display()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_macros_table_cell_is_its_first_line_cut_short() {
        let long = "é".repeat(60);
        for (text, cell) in [
            ("ls -la", "ls -la".to_string()),
            ("git status\n", "git status".to_string()),
            ("first\nsecond", "first…".to_string()),
            (long.as_str(), format!("{}…", "é".repeat(48))),
        ] {
            assert_eq!(text_cell(text), cell, "{text:?}");
        }
    }

    /// A stand-in local dux answering `/api/v1/build` and `bootstrap`, and a
    /// client of it.
    fn local_dux(
        dir: &std::path::Path,
        bootstrap: &'static str,
    ) -> (
        crate::client::test_server::FakeDux,
        crate::lockfile::SingleInstanceLock,
        Client,
    ) {
        use crate::client::test_server::{FakeDux, Reply};
        let socket = dir.join("dux.sock");
        let fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, r#"{"version":"v1","process":"p","api":1}"#),
            "/api/v1/bootstrap" => Reply::json(200, bootstrap),
            _ => Reply::json(404, "{}"),
        });
        let lock_path = dir.join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client =
            crate::client::connect::connect(&crate::client::connect::Target::Local, &lock_path)
                .unwrap();
        (fake, lock, client)
    }

    #[test]
    fn env_set_refuses_a_name_that_is_no_variable_name_without_sending_or_repeating_it() {
        let dir = crate::client::test_server::private_dir();
        let (fake, _lock, client) = local_dux(dir.path(), "{}");
        let writer = Writer::Dux {
            client: &client,
            wait: Some(Duration::from_secs(1)),
        };
        let refused = set_env(writer, "zz LEAK.x", "v").unwrap_err();
        assert_eq!(refused.exit, Exit::Failed);
        assert!(!refused.message.contains("LEAK"), "{}", refused.message);
        assert!(
            fake.seen()
                .iter()
                .all(|seen| !seen.path.contains("global-env")),
            "{:?}",
            fake.seen()
        );
    }

    #[test]
    fn a_macro_is_found_by_the_name_it_has_here_and_through_a_running_dux() {
        let dir = crate::client::test_server::private_dir();
        let paths = DuxPaths {
            root: dir.path().to_path_buf(),
            config_path: dir.path().join("config.toml"),
            sessions_db_path: dir.path().join("sessions.sqlite3"),
            worktrees_root: dir.path().join("worktrees"),
            lock_path: dir.path().join("dux.lock"),
            socket_path: dir.path().join("dux.sock"),
        };
        std::fs::write(
            &paths.config_path,
            "[macros]\n\"Review changes\" = { text = \"review\", surface = \"agent\" }\n\n\
             [providers.\"my tool\"]\ncommand = \"x\"\n",
        )
        .unwrap();
        // The file's own name finds a provider whose name is not shown.
        let shown = providers_show(&Source::File(&paths), "my tool").expect("found");
        assert_eq!(
            shown,
            "name: the entry on line 4 of [providers]\nsource: yours\n"
        );
        let (_fake, _lock, client) = local_dux(
            dir.path(),
            r#"{"macros":[{"name":"Review changes","text":"review","surface":"agent"}],"global_env":{}}"#,
        );
        for source in [Source::File(&paths), Source::Dux(&client)] {
            let shown = macros_show(&source, "Review changes").expect("found by its name");
            assert!(shown.contains("surface: agent"), "{shown}");
            let missing = macros_show(&source, "Review it").unwrap_err();
            assert_eq!(missing.exit, Exit::Failed);
        }
    }
}
