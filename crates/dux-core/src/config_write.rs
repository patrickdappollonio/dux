//! Shared `config.toml` writer built on `toml_edit`.
//!
//! This module owns the surgical PATCH path: given an in-memory [`Config`], it
//! updates only the keys it manages in an existing TOML document, preserving the
//! user's comments, formatting, and any unknown keys. It deliberately does NOT
//! render the fully-commented canonical template — that path needs the TUI's
//! `RuntimeBindings` for two comment strings and stays in the binary.
//!
//! Both the TUI and the web surface share this patch path so a save from either
//! preserves the same on-disk shape. The TUI keeps its own pretty
//! first-creation renderer; the web uses [`save_config`] for a plain fallback.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{Array, DocumentMut, Formatted, InlineTable, Item, Table, Value};

use crate::config::{
    Config, MacroSurface, MacrosConfig, OneshotOutput, ProjectConfig, ProvidersConfig,
};

/// Patch an EXISTING `config.toml` in place, preserving the user's comments,
/// formatting, and any keys this writer doesn't manage. Reads the file, applies
/// every section patch, and writes it back.
pub fn patch_config_file(config_path: &Path, config: &Config) -> Result<()> {
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", config_path.display()))?;

    apply_patches(&mut doc, config);

    fs::write(config_path, doc.to_string())
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(())
}

/// Save config: patch in place if the file exists, otherwise write a plain
/// (uncommented) `toml_edit` serialization from scratch. Used by surfaces that
/// don't have the TUI's canonical commented renderer (e.g. the web). The TUI
/// keeps its own `save_config` for the pretty first-creation path.
pub fn save_config(config_path: &Path, config: &Config) -> Result<()> {
    if config_path.exists() {
        return patch_config_file(config_path, config);
    }
    write_config_plain(config_path, config)
}

/// Unconditionally write a fresh plain (uncommented) `toml_edit` serialization,
/// overwriting whatever is on disk. Unlike [`save_config`], this never patches
/// an existing file, so it succeeds even when the current `config.toml` is
/// corrupt or unparseable. Used by the web's "recover config" path, which must
/// overwrite a broken file from the in-memory config.
pub fn write_config_plain(config_path: &Path, config: &Config) -> Result<()> {
    // Build a fresh document from scratch using the same patch helpers against
    // an empty document. No comments — this is the plain fallback, not the
    // TUI's pretty first-creation path.
    let mut doc = DocumentMut::new();
    apply_patches(&mut doc, config);
    fs::write(config_path, doc.to_string())
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(())
}

/// Apply every section patch to `doc`. Mirrors the section sequence the TUI's
/// existing-file branch ran, so both surfaces produce the same managed shape.
fn apply_patches(doc: &mut DocumentMut, config: &Config) {
    // --- [defaults] ---
    patch_table_str(doc, "defaults", "provider", &config.defaults.provider);
    patch_table_opt_str(
        doc,
        "defaults",
        "start_directory",
        config.defaults.start_directory.as_deref(),
    );
    patch_table_opt_multiline(
        doc,
        "defaults",
        "commit_prompt",
        config.defaults.commit_prompt.as_deref(),
    );
    patch_table_bool(
        doc,
        "defaults",
        "enable_randomized_pet_name_by_default",
        config.defaults.enable_randomized_pet_name_by_default,
    );
    patch_table_bool(
        doc,
        "defaults",
        "pull_before_creating_agent_by_default",
        config.defaults.pull_before_creating_agent_by_default,
    );
    remove_table_key(doc, "defaults", "prompt_for_name");

    // --- [env] ---
    patch_env_table(doc, "env", &config.env);

    // --- [logging] ---
    patch_table_str(doc, "logging", "level", &config.logging.level);
    patch_table_str(doc, "logging", "path", &config.logging.path);

    // --- [ui] ---
    patch_table_u16(doc, "ui", "left_width_pct", config.ui.left_width_pct);
    patch_table_u16(doc, "ui", "right_width_pct", config.ui.right_width_pct);
    patch_table_u16(
        doc,
        "ui",
        "terminal_pane_height_pct",
        config.ui.terminal_pane_height_pct,
    );
    patch_table_u16(
        doc,
        "ui",
        "empty_project_separator_min_projects",
        config.ui.empty_project_separator_min_projects,
    );
    patch_table_u16(
        doc,
        "ui",
        "staged_pane_height_pct",
        config.ui.staged_pane_height_pct,
    );
    patch_table_u16(
        doc,
        "ui",
        "commit_pane_height_pct",
        config.ui.commit_pane_height_pct,
    );
    patch_table_usize(
        doc,
        "ui",
        "agent_scrollback_lines",
        config.ui.agent_scrollback_lines,
    );
    patch_table_u16(
        doc,
        "ui",
        "branch_sync_interval",
        config.ui.branch_sync_interval,
    );
    patch_table_bool(
        doc,
        "ui",
        "show_diff_line_numbers",
        config.ui.show_diff_line_numbers,
    );
    patch_table_u16(doc, "ui", "diff_tab_width", config.ui.diff_tab_width);
    patch_table_bool(
        doc,
        "ui",
        "github_integration",
        config.ui.github_integration,
    );
    patch_table_bool(
        doc,
        "ui",
        "auto_reopen_agents",
        config.ui.auto_reopen_agents,
    );
    patch_table_str(
        doc,
        "ui",
        "pr_banner_position",
        &config.ui.pr_banner_position,
    );
    patch_table_str(doc, "ui", "theme", &config.ui.theme);

    // --- [editor] ---
    patch_table_str(doc, "editor", "default", &config.editor.default);

    // --- [server] ---
    patch_table_str(doc, "server", "bind", &config.server.bind);
    patch_table_bool(
        doc,
        "server",
        "insecure_allow_remote",
        config.server.insecure_allow_remote,
    );

    // --- [auth] ---
    patch_table_string_array(doc, "auth", "users", &config.auth.users);

    // --- [terminal] ---
    patch_table_str(doc, "terminal", "command", &config.terminal.command);
    patch_table_string_array(doc, "terminal", "args", &config.terminal.args);

    // --- [startup_command_terminal] ---
    patch_table_str(
        doc,
        "startup_command_terminal",
        "command",
        &config.startup_command_terminal.command,
    );
    patch_table_string_array(
        doc,
        "startup_command_terminal",
        "args",
        &config.startup_command_terminal.args,
    );

    // --- [keys] ---
    patch_table_bool(
        doc,
        "keys",
        "show_terminal_keys",
        config.keys.show_terminal_keys,
    );
    {
        let keys_table = doc
            .entry("keys")
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .unwrap();
        for (action, key_strs) in &config.keys.bindings {
            let mut arr = Array::new();
            for s in key_strs {
                arr.push(s.as_str());
            }
            keys_table[action] = toml_edit::value(arr);
        }
    }

    // --- [providers.*] ---
    patch_providers(doc, &config.providers);

    // --- [[projects]] ---
    patch_projects(doc, &config.projects);

    // --- [macros] ---
    patch_macros(doc, &config.macros);
}

// ---------------------------------------------------------------------------
// toml_edit patch helpers
// ---------------------------------------------------------------------------

/// Get or create a table named `section` at the document root.
///
/// Public because the TUI's deprecation migrations reuse it.
pub fn ensure_table<'a>(doc: &'a mut DocumentMut, section: &str) -> &'a mut Table {
    doc.entry(section)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .unwrap()
}

fn patch_table_str(doc: &mut DocumentMut, section: &str, key: &str, value: &str) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value);
}

fn patch_table_opt_str(doc: &mut DocumentMut, section: &str, key: &str, value: Option<&str>) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value.unwrap_or(""));
}

fn patch_table_opt_multiline(doc: &mut DocumentMut, section: &str, key: &str, value: Option<&str>) {
    let table = ensure_table(doc, section);
    match value {
        Some(s) => {
            // Build a tiny TOML document with a triple-quoted string and extract
            // the parsed value so the repr uses the multiline form.
            let escaped = escape_toml_multiline(s);
            let snippet = format!("v = \"\"\"\n{escaped}\"\"\"");
            if let Ok(mini) = snippet.parse::<DocumentMut>()
                && let Some(item) = mini.get("v")
            {
                table[key] = item.clone();
                return;
            }
            // Fallback: regular string (newlines escaped as \n).
            table[key] = toml_edit::value(s);
        }
        None => {
            table[key] = toml_edit::value("");
        }
    }
}

fn patch_table_u16(doc: &mut DocumentMut, section: &str, key: &str, value: u16) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(i64::from(value));
}

fn patch_table_usize(doc: &mut DocumentMut, section: &str, key: &str, value: usize) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value as i64);
}

fn patch_table_bool(doc: &mut DocumentMut, section: &str, key: &str, value: bool) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value);
}

fn remove_table_key(doc: &mut DocumentMut, section: &str, key: &str) {
    let _ = remove_table_key_item(doc, section, key);
}

/// Remove `key` from the table named `section`, returning the removed item.
///
/// Public because the TUI's deprecation migrations reuse it.
pub fn remove_table_key_item(doc: &mut DocumentMut, section: &str, key: &str) -> Option<Item> {
    doc.get_mut(section)
        .and_then(Item::as_table_mut)
        .and_then(|table| table.remove(key))
}

fn patch_table_string_array(doc: &mut DocumentMut, section: &str, key: &str, values: &[String]) {
    let table = ensure_table(doc, section);
    let mut arr = Array::new();
    for v in values {
        arr.push(v.as_str());
    }
    table[key] = toml_edit::value(arr);
}

fn patch_providers(doc: &mut DocumentMut, providers: &ProvidersConfig) {
    let providers_table = doc
        .entry("providers")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .unwrap();

    for (name, config) in &providers.commands {
        let tbl = providers_table
            .entry(name)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .unwrap();

        tbl["command"] = toml_edit::value(&config.command);

        let mut args = Array::new();
        for a in &config.args {
            args.push(a.as_str());
        }
        tbl["args"] = toml_edit::value(args);

        let mut resume = Array::new();
        for a in config.resume_args.as_deref().unwrap_or(&[]) {
            resume.push(a.as_str());
        }
        tbl["resume_args"] = toml_edit::value(resume);
        if let Some(timeout_ms) = config.resume_wait_timeout_ms {
            tbl["resume_wait_timeout_ms"] = toml_edit::value(timeout_ms as i64);
        }

        let mut oneshot = Array::new();
        for a in &config.oneshot_args {
            oneshot.push(a.as_str());
        }
        tbl["oneshot_args"] = toml_edit::value(oneshot);

        let output = match config.oneshot_output {
            OneshotOutput::Stdout => "stdout",
            OneshotOutput::Tempfile => "tempfile",
        };
        tbl["oneshot_output"] = toml_edit::value(output);

        if let Some(hint) = &config.install_hint {
            tbl["install_hint"] = toml_edit::value(hint.as_str());
        }

        tbl["forward_scroll"] = toml_edit::value(config.forward_scroll);
    }
}

fn patch_macros(doc: &mut DocumentMut, macros: &MacrosConfig) {
    let table = ensure_table(doc, "macros");

    // Remove entries that no longer exist in config.
    let existing_keys: Vec<String> = table
        .iter()
        .filter(|(_, v)| v.is_inline_table())
        .map(|(k, _)| k.to_string())
        .collect();
    for key in &existing_keys {
        if !macros.entries.contains_key(key) {
            table.remove(key);
        }
    }

    // Add or update entries.
    for (name, entry) in &macros.entries {
        let mut inline = InlineTable::new();
        inline.insert("text", Value::String(Formatted::new(entry.text.clone())));
        let surface_str = match entry.surface {
            MacroSurface::Agent => "agent",
            MacroSurface::Terminal => "terminal",
            MacroSurface::Both => "both",
        };
        inline.insert(
            "surface",
            Value::String(Formatted::new(surface_str.to_string())),
        );
        table[name] = toml_edit::value(Value::InlineTable(inline));
    }
}

fn patch_projects(doc: &mut DocumentMut, projects: &[ProjectConfig]) {
    let _ = doc.remove("projects");
    if projects.is_empty() {
        return;
    }

    let mut array = toml_edit::ArrayOfTables::new();
    for project in projects {
        let mut table = Table::new();
        table["id"] = toml_edit::value(project.id.as_str());
        table["path"] = toml_edit::value(project.path.as_str());
        if let Some(name) = project.name.as_deref() {
            table["name"] = toml_edit::value(name);
        }
        if let Some(provider) = project.default_provider.as_deref() {
            table["default_provider"] = toml_edit::value(provider);
        }
        if let Some(auto_reopen_agents) = project.auto_reopen_agents {
            table["auto_reopen_agents"] = toml_edit::value(auto_reopen_agents);
        }
        if let Some(command) = project.startup_command.as_deref() {
            table["startup_command"] = toml_edit::value(command);
        }
        if !project.env.is_empty() {
            let mut inline = InlineTable::new();
            for (name, value) in &project.env {
                inline.insert(name, Value::String(Formatted::new(value.clone())));
            }
            table["env"] = toml_edit::value(Value::InlineTable(inline));
        }
        array.push(table);
    }
    doc["projects"] = Item::ArrayOfTables(array);
}

fn patch_env_table(doc: &mut DocumentMut, section: &str, env: &BTreeMap<String, String>) {
    let table = ensure_table(doc, section);
    let existing = table
        .iter()
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    for key in existing {
        table.remove(&key);
    }
    for (name, value) in env {
        table[name] = toml_edit::value(value.as_str());
    }
}

/// Escape triple-quotes in a TOML multiline basic string.
///
/// Per the TOML spec, `"""` inside `"""..."""` can be included by escaping at
/// least one quote: `""\"`. Public because the TUI's canonical renderer reuses
/// it for the same multiline fields.
pub fn escape_toml_multiline(value: &str) -> String {
    value.replace("\"\"\"", "\"\"\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_preserves_comments_and_unknown_keys() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "\
# A user comment that must survive
[env]
EXISTING = \"keep-me\"

[some_unknown_section]
unknown_key = \"untouched\"
",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("# A user comment that must survive"),
            "user comment lost: {saved}"
        );
        assert!(
            saved.contains("unknown_key = \"untouched\""),
            "unknown key lost: {saved}"
        );
        assert!(
            saved.contains("FOO = \"bar\""),
            "new value missing: {saved}"
        );
    }

    #[test]
    fn patch_writes_env() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn patch_writes_project_fields() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        let mut env = BTreeMap::new();
        env.insert("KEY".to_string(), "value".to_string());
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("test".to_string()),
            default_provider: Some("codex".to_string()),
            leading_branch: None,
            auto_reopen_agents: Some(true),
            startup_command: Some("npm install".to_string()),
            env,
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.projects.len(), 1);
        let project = &parsed.projects[0];
        assert_eq!(project.default_provider.as_deref(), Some("codex"));
        assert_eq!(project.startup_command.as_deref(), Some("npm install"));
        assert_eq!(project.auto_reopen_agents, Some(true));
        assert_eq!(project.env.get("KEY").map(String::as_str), Some("value"));
    }

    #[test]
    fn write_config_plain_overwrites() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Seed a corrupt/unparseable file that `save_config`'s patch path would
        // choke on. `write_config_plain` must overwrite it regardless.
        fs::write(&config_path, "this is not = valid toml [[[ \n broken").expect("write garbage");

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse valid config");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(parsed.defaults.provider, config.defaults.provider);
    }

    #[test]
    fn write_config_plain_round_trips_server_section() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.server.bind = "0.0.0.0:9000".to_string();
        config.server.insecure_allow_remote = true;

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.bind, "0.0.0.0:9000");
        assert!(parsed.server.insecure_allow_remote);
    }

    #[test]
    fn write_config_plain_round_trips_auth_users() {
        // LESSON from the [server] slice: a managed section that forgets its
        // apply_patches entry silently wipes user settings on the recover path.
        // Guard the [auth] users against that regression: non-default users must
        // survive a from-scratch plain write.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.auth.users = vec![
            "alice:$2y$12$abcdefghijklmnopqrstuv".to_string(),
            "bob:$2y$12$wxyz0123456789abcdefgh".to_string(),
        ];

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed.auth.users,
            vec![
                "alice:$2y$12$abcdefghijklmnopqrstuv".to_string(),
                "bob:$2y$12$wxyz0123456789abcdefgh".to_string(),
            ]
        );
    }

    #[test]
    fn patch_preserves_existing_auth_users() {
        // Patching an existing file must keep configured [auth] users intact.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[auth]\nusers = [\"alice:$2y$12$existinghashvalue000000\"]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        // Mirror what a real load would do: the in-memory config carries the
        // same users back into the patch.
        config.auth.users = vec!["alice:$2y$12$existinghashvalue000000".to_string()];

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed.auth.users,
            vec!["alice:$2y$12$existinghashvalue000000".to_string()]
        );
    }

    #[test]
    fn save_config_creates_file_when_missing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let mut config = Config::default();
        config.env.insert("FOO".to_string(), "bar".to_string());
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("test".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        save_config(&config_path, &config).expect("save");

        assert!(config_path.exists(), "save_config did not create the file");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "project-1");
    }
}
