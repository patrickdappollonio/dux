//! The resources kept in `config.toml` (macros, providers, keybindings,
//! themes and the global environment) as the command line lists them, and
//! the edits it makes to the file when no dux is running.
//!
//! The web API's `GET /api/v1/config/providers`, `/keys` and `/themes` answer
//! with the same entries built here from the server's own file, so a listing
//! reads the same through a remote as it does on this machine.
//!
//! A name the file chose is printed only where the config formatter prints
//! it ([`crate::config::shown_path`]): a name that breaks its table's rule may
//! be a token pasted where a name goes, so it is shown as the line it sits on,
//! and nothing under it is printed.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{Config, DuxPaths, ProviderCommandConfig};
use crate::config_keys::{NOT_SHOWN, ValueForm};

/// A config file as read for a listing: its text (empty when there is no
/// file yet) and what dux loads from it.
pub struct ConfigFile {
    pub raw: String,
    pub config: Config,
}

/// Read `config_path` as every surface loads it; a missing file is the defaults,
/// and the error never quotes a value from the file.
pub fn read(config_path: &Path) -> Result<ConfigFile, String> {
    let fail = |problem| {
        crate::config::ConfigLoadError {
            path: config_path.to_path_buf(),
            problem,
        }
        .to_string()
    };
    match crate::config::read_config_text(config_path).map_err(|error| error.to_string())? {
        Some(raw) => {
            let config = crate::config::config_from_text_as_loaded(&raw).map_err(fail)?;
            Ok(ConfigFile { raw, config })
        }
        None => {
            let mut config = Config::default();
            config.providers.ensure_defaults();
            Ok(ConfigFile {
                raw: String::new(),
                config,
            })
        }
    }
}

/// `name`, an entry of the user-named table `table`, as a listing prints it, and
/// whether it is hidden; `raw` is the file it came from, empty for a running dux.
pub fn shown_name(raw: &str, table: &str, name: &str) -> (String, bool) {
    let segments = [table.to_string(), name.to_string()];
    if crate::config::name_is_hidden(raw, &segments) {
        (crate::config::shown_path(raw, &segments), true)
    } else {
        (name.to_string(), false)
    }
}

/// A global environment variable's name as a sentence names it: a name that is
/// no variable name is replaced by its line's placeholder.
pub fn env_label(raw: &str, name: &str) -> String {
    shown_name(raw, "env", name).0
}

/// A macro's name as a sentence names it: the name itself when it follows the
/// `[macros]` rule, else the placeholder for an entry whose name is not shown.
pub fn macro_label(name: &str) -> String {
    shown_name("", "macros", name).0
}

/// The refusal for removing a macro the list does not hold.
pub fn unknown_macro(name: &str) -> String {
    match shown_name("", "macros", name) {
        (name, false) => format!("unknown macro \"{name}\""),
        (label, true) => format!("unknown macro: {label}"),
    }
}

/// The refusal for removing a variable the global environment does not hold.
pub fn unknown_env_var(name: &str) -> String {
    match shown_name("", "env", name) {
        (name, false) => format!("unknown global environment variable \"{name}\""),
        (label, true) => format!("unknown global environment variable: {label}"),
    }
}

/// A request path as the access log may print it: a macro or global environment
/// name that breaks its table's rule is replaced by `(name not shown)`.
pub fn logged_path(path: &str) -> std::borrow::Cow<'_, str> {
    for (prefix, table) in [
        ("/api/v1/global-env/", "env"),
        ("/api/v1/macros/", "macros"),
    ] {
        let Some(segment) = path.strip_prefix(prefix) else {
            continue;
        };
        if segment.contains('/') {
            break;
        }
        let name = percent_encoding::percent_decode_str(segment).decode_utf8_lossy();
        if shown_name("", table, &name).1 {
            return format!("{prefix}(name not shown)").into();
        }
        break;
    }
    path.into()
}

/// One macro as listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroItem {
    pub name: String,
    pub surface: String,
    /// `None` when the name is hidden.
    pub text: Option<String>,
    /// The name it is looked up by: the file's own, never printed. Empty for
    /// one a running dux listed, which is looked up by `name`.
    #[serde(skip)]
    pub key: String,
}

/// Macros, given as name, text and surface in their order.
pub fn macros(
    raw: &str,
    entries: impl IntoIterator<Item = (String, String, String)>,
) -> Vec<MacroItem> {
    entries
        .into_iter()
        .map(|(name, text, surface)| {
            let (shown, hidden) = shown_name(raw, "macros", &name);
            MacroItem {
                name: shown,
                surface,
                text: (!hidden).then_some(text),
                key: name,
            }
        })
        .collect()
}

/// The macros of a loaded file, in file order.
pub fn macros_of(file: &ConfigFile) -> Vec<MacroItem> {
    macros(
        &file.raw,
        file.config.macros.entries.iter().map(|(name, entry)| {
            (
                name.clone(),
                entry.text.clone(),
                entry.surface.as_config_str().to_string(),
            )
        }),
    )
}

/// One global environment variable as listed: its value only when asked
/// for, and never under a hidden name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvItem {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// The global environment, by name; with `show`, each value through the one
/// value printer.
pub fn env(raw: &str, env: &BTreeMap<String, String>, show: bool) -> Vec<EnvItem> {
    env.iter()
        .map(|(name, value)| {
            let (shown, _) = shown_name(raw, "env", name);
            let value = show.then(|| {
                crate::config_keys::printed_value(
                    raw,
                    &["env".to_string(), name.clone()],
                    &toml::Value::String(value.clone()),
                    ValueForm::Summary,
                )
                .unwrap_or_else(|| NOT_SHOWN.to_string())
            });
            EnvItem { name: shown, value }
        })
        .collect()
}

/// One provider as listed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderItem {
    pub name: String,
    /// `built in` for a provider dux ships, `yours` for one the file adds.
    pub source: String,
    /// `None` when the name is hidden.
    pub settings: Option<ProviderCommandConfig>,
    /// The name it is looked up by, as [`MacroItem::key`].
    #[serde(skip)]
    pub key: String,
}

/// The providers of a loaded file, in its order, dux's own included.
pub fn providers(file: &ConfigFile) -> Vec<ProviderItem> {
    let built_in: Vec<&str> = crate::config::default_provider_commands()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    file.config
        .providers
        .commands
        .iter()
        .map(|(name, settings)| {
            let (shown, hidden) = shown_name(&file.raw, "providers", name);
            ProviderItem {
                name: shown,
                source: if built_in.contains(&name.as_str()) {
                    "built in"
                } else {
                    "yours"
                }
                .to_string(),
                settings: (!hidden).then(|| settings.clone()),
                key: name.clone(),
            }
        })
        .collect()
}

/// One keybinding as listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyItem {
    pub action: String,
    pub keys: Vec<String>,
    /// `default` when it is the terminal UI's own binding, `yours` otherwise.
    pub source: String,
}

/// Every action the terminal UI binds, with the keys it uses under the file
/// `raw`, in the terminal UI's own order.
pub fn keys(raw: &str) -> Result<Vec<KeyItem>, String> {
    let unreadable = || {
        "the [keys] in config.toml could not be read; run \"dux config get keys\" to see why"
            .to_string()
    };
    let used = crate::config::terminal_ui_keys(raw).ok_or_else(unreadable)?;
    let defaults = crate::config::terminal_ui_keys("").ok_or_else(unreadable)?;
    let strings = |value: &toml::Value| -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(used
        .iter()
        .filter(|(_, value)| value.is_array())
        .map(|(action, value)| KeyItem {
            action: action.clone(),
            keys: strings(value),
            source: if defaults.get(action) == Some(value) {
                "default"
            } else {
                "yours"
            }
            .to_string(),
        })
        .collect())
}

/// One theme as listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeItem {
    pub name: String,
    pub source: String,
    pub current: bool,
}

/// Every theme the terminal UI's picker offers, in its order, `current`
/// marking the one `ui.theme` names.
pub fn themes(paths: &DuxPaths, current: &str) -> Vec<ThemeItem> {
    crate::theme::discover_available(paths)
        .into_iter()
        .map(|theme| ThemeItem {
            current: theme.id == current,
            source: theme.source.as_str().to_string(),
            name: theme.id,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Edits with no dux running
// ---------------------------------------------------------------------------

/// What a missing config.toml means to an edit: refused while a dux may be
/// running, whose next reload would drop its settings; otherwise the default.
fn missing_file(paths: &DuxPaths) -> crate::config_write::MissingConfig<'static> {
    let lock_path = paths.lock_path.clone();
    let config_path = paths.config_path.clone();
    crate::config_write::MissingConfig::CheckFirst(Box::new(move || {
        if crate::reload_signal::dux_may_be_running(&lock_path) {
            anyhow::bail!(
                "{} is missing while dux may be running; nothing was changed",
                config_path.display()
            );
        }
        Ok(())
    }))
}

/// Add the macro `name`, or replace it where it stands, in the file.
pub fn set_macro_in_file(
    paths: &DuxPaths,
    name: &str,
    text: String,
    surface: &str,
) -> anyhow::Result<String> {
    let (name, entry) = crate::config::checked_macro(name, text, surface)?;
    crate::config_write::mutate_config_file_with(&paths.config_path, missing_file(paths), |doc| {
        crate::config_write::set_macro(doc, &name, &entry);
        Ok(())
    })?;
    Ok(name)
}

/// Remove the macro `name` from the file.
pub fn remove_macro_in_file(paths: &DuxPaths, name: &str) -> anyhow::Result<()> {
    crate::config_write::mutate_config_file_with(&paths.config_path, missing_file(paths), |doc| {
        let table = crate::config_write::ensure_table(doc, "macros");
        if table.remove(name).is_none() {
            anyhow::bail!("{}", unknown_macro(name));
        }
        Ok(())
    })
}

/// Set the global environment variable `name` in the file.
pub fn set_env_in_file(paths: &DuxPaths, name: &str, value: &str) -> anyhow::Result<()> {
    crate::config::check_global_env_var(name, value)?;
    crate::config_write::mutate_config_file_with(&paths.config_path, missing_file(paths), |doc| {
        // Where `dux config set` writes, so a replaced value keeps the
        // comment and spacing around it.
        crate::config_keys::set_in_doc(
            doc,
            &["env".to_string(), name.to_string()],
            toml_edit::Value::from(value),
        )
    })
}

/// Remove the global environment variable `name` from the file.
pub fn remove_env_in_file(paths: &DuxPaths, name: &str) -> anyhow::Result<()> {
    crate::config_write::mutate_config_file_with(&paths.config_path, missing_file(paths), |doc| {
        let table = crate::config_write::ensure_table(doc, "env");
        if table.remove(name).is_none() {
            anyhow::bail!("{}", unknown_env_var(name));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_with(body: Option<&str>) -> (crate::test_scratch::ScratchDir, DuxPaths) {
        let tmp = crate::test_scratch::ScratchDir::new();
        let root = tmp.path();
        let paths = DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };
        if let Some(body) = body {
            std::fs::write(&paths.config_path, body).unwrap();
        }
        (tmp, paths)
    }

    #[test]
    fn providers_list_the_files_own_and_dux_s_built_in_ones() {
        let (_tmp, paths) = paths_with(Some("[providers.mytool]\ncommand = \"my-cli\"\n"));
        let listed: Vec<(String, String, String)> = providers(&read(&paths.config_path).unwrap())
            .into_iter()
            .map(|p| (p.name, p.source, p.settings.unwrap().command))
            .collect();
        let row = |name: &str, source: &str, command: &str| {
            (name.to_string(), source.to_string(), command.to_string())
        };
        assert_eq!(
            listed,
            [
                row("mytool", "yours", "my-cli"),
                row("claude", "built in", "claude"),
                row("codex", "built in", "codex"),
                row("opencode", "built in", "opencode"),
                row("copilot", "built in", "copilot"),
            ]
        );
    }

    #[test]
    fn a_file_edit_changes_one_entry_and_keeps_the_comments_around_it() {
        let (_tmp, paths) = paths_with(Some(
            "# my settings\n[macros]\n# says hi\nhi = { text = \"hello\", surface = \"agent\" } # greets\n\n[env]\n# for the API\nTOKEN   = \"old\" # production credential\n# gone soon\nOLD = \"x\"\n",
        ));
        set_macro_in_file(&paths, " bye ", "see you".to_string(), "both").unwrap();
        set_macro_in_file(&paths, "hi", "hey".to_string(), "terminal").unwrap();
        set_env_in_file(&paths, "TOKEN", "new").unwrap();
        set_env_in_file(&paths, "REGION", "eu").unwrap();
        remove_env_in_file(&paths, "OLD").unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.config_path).unwrap(),
            "# my settings\n[macros]\n# says hi\nhi = { text = \"hey\", surface = \"terminal\" } # greets\nbye = { text = \"see you\", surface = \"both\" }\n\n[env]\n# for the API\nTOKEN   = \"new\" # production credential\nREGION = \"eu\"\n"
        );
        remove_macro_in_file(&paths, "hi").unwrap();
        let text = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(!text.contains("hey"), "{text}");
        assert!(text.contains("bye"), "{text}");
    }

    #[test]
    fn a_file_edit_refuses_what_a_running_dux_would_refuse_and_writes_nothing() {
        let body = "[macros]\nhi = { text = \"hello\", surface = \"agent\" }\n";
        let (_tmp, paths) = paths_with(Some(body));
        for (result, says) in [
            (
                set_macro_in_file(&paths, "x", String::new(), "agent").map(drop),
                "Macro \"x\" has no text. Enter the text to send.",
            ),
            (
                set_macro_in_file(&paths, "x", "t".to_string(), "everywhere").map(drop),
                "Macro \"x\" has an unknown surface \"everywhere\".",
            ),
            (
                remove_macro_in_file(&paths, "nope"),
                "unknown macro \"nope\"",
            ),
            (
                set_env_in_file(&paths, "1BAD", "v"),
                "That is not a valid environment variable name",
            ),
            (
                remove_env_in_file(&paths, "NOPE"),
                "unknown global environment variable \"NOPE\"",
            ),
        ] {
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.starts_with(says), "{error}");
        }
        assert_eq!(std::fs::read_to_string(&paths.config_path).unwrap(), body);
    }
}
