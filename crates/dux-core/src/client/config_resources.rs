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
    let item = output::select("macro", name, &items, |m| &m.name, |m| &m.name)?;
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
    let item = output::select("provider", name, &items, |p| &p.name, |p| &p.name)?;
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

/// `dux macros add`: add the macro, or replace the one with that name.
pub fn set_macro(
    writer: Writer<'_>,
    name: &str,
    text: String,
    surface: &str,
) -> Result<String, CliError> {
    match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Put,
            &entry_path("macros", name),
            Some(serde_json::json!({ "text": text, "surface": surface })),
        ),
        Writer::File(paths) => {
            let saved =
                resources::set_macro_in_file(paths, name, text, surface).map_err(edit_failed)?;
            Ok(format!(
                "Saved macro {saved} in {}. {APPLIES_AT_START}\n",
                paths.config_path.display()
            ))
        }
    }
}

/// `dux macros rm`.
pub fn remove_macro(writer: Writer<'_>, name: &str) -> Result<String, CliError> {
    match writer {
        Writer::Dux { client, wait } => through_dux(
            client,
            wait,
            Method::Delete,
            &entry_path("macros", name),
            None,
        ),
        Writer::File(paths) => {
            resources::remove_macro_in_file(paths, name).map_err(edit_failed)?;
            Ok(format!(
                "Removed macro {name} from {}. {APPLIES_AT_START}\n",
                paths.config_path.display()
            ))
        }
    }
}

/// `dux env set`. The value is never printed.
pub fn set_env(writer: Writer<'_>, name: &str, value: &str) -> Result<String, CliError> {
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
            resources::remove_env_in_file(paths, name).map_err(edit_failed)?;
            Ok(format!(
                "Removed global environment variable {name} from {}. {APPLIES_AT_START}\n",
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
}
