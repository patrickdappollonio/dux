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
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{Array, DocumentMut, Formatted, InlineTable, Item, Table, Value};

/// Permission bits for `config.toml`: owner read/write only (`0600`). The file
/// holds bcrypt password hashes under `[auth]` and may hold tokens under `[env]`,
/// so it must not be group/world readable. Unix-only — the project targets macOS
/// and Linux (CLAUDE.md), so no `cfg(windows)` branch is needed.
const CONFIG_FILE_MODE: u32 = 0o600;

/// Write `contents` to `path`, restricting the file to owner-only `0600`. Used
/// for every `config.toml` write so the secrets it carries (bcrypt hashes,
/// `[env]` tokens) are never left group/world readable.
///
/// We create the file with the `0600` mode applied AT creation (via
/// `OpenOptions::mode`) so a fresh file is never briefly world-readable: a plain
/// `fs::write` would create the file at umask perms (typically `0644`) with the
/// secrets already in it, then chmod afterward — a window where the hashes are
/// readable. The mode arg only takes effect when the file is newly created, so
/// for an EXISTING file (e.g. an older `0644` config) we still run an explicit
/// `set_permissions(0600)` after writing to tighten it. Both behaviors live in
/// this one helper so every write path is covered.
pub fn write_config_secure(path: &Path, contents: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(CONFIG_FILE_MODE)
        .open(path)
        .with_context(|| format!("failed to open {} for writing", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    // `mode` above only applies on creation; an existing file keeps its old
    // perms, so tighten it explicitly to upgrade older `0644` configs to `0600`.
    let perms = fs::Permissions::from_mode(CONFIG_FILE_MODE);
    fs::set_permissions(path, perms)
        .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    Ok(())
}

use crate::config::{
    AcmeSettings, Config, MacrosConfig, OneshotOutput, ProjectConfig, ProvidersConfig,
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

    write_config_secure(config_path, &doc.to_string())
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
    write_config_secure(config_path, &doc.to_string())
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
    // `bind` is DEPRECATED: it is migrated away on load and is never re-emitted
    // here, so a patch/recover/plain write produces the new port/listen_addrs
    // shape only.
    patch_table_u16(doc, "server", "port", config.server.port);
    patch_table_bool(
        doc,
        "server",
        "tailscale_enabled",
        config.server.tailscale_enabled,
    );
    patch_table_string_array(doc, "server", "listen_addrs", &config.server.listen_addrs);
    patch_table_bool(
        doc,
        "server",
        "insecure_allow_remote",
        config.server.insecure_allow_remote,
    );
    patch_table_str(doc, "server", "color", &config.server.color);
    patch_table_bool(doc, "server", "access_log", config.server.access_log);

    // --- [server.acme] ---
    patch_acme(doc, &config.server.acme);

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

/// Patch the `[server.acme]` subtable. Mirrors [`patch_providers`] for nested
/// table access — every field is written so a recover/plain write never silently
/// drops a setting (the [server] lesson).
fn patch_acme(doc: &mut DocumentMut, acme: &AcmeSettings) {
    let server = doc
        .entry("server")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .unwrap();
    let tbl = server
        .entry("acme")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .unwrap();

    tbl["enabled"] = toml_edit::value(acme.enabled);

    let mut domains = Array::new();
    for d in &acme.domains {
        domains.push(d.as_str());
    }
    tbl["domains"] = toml_edit::value(domains);

    tbl["email"] = toml_edit::value(acme.email.as_str());
    tbl["http_port"] = toml_edit::value(i64::from(acme.http_port));
    tbl["https_port"] = toml_edit::value(i64::from(acme.https_port));
    tbl["production"] = toml_edit::value(acme.production);
    tbl["cache_dir"] = toml_edit::value(acme.cache_dir.as_deref().unwrap_or(""));
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
        inline.insert(
            "surface",
            Value::String(Formatted::new(entry.surface.as_config_str().to_string())),
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
        config.server.port = 9000;
        config.server.tailscale_enabled = false;
        config.server.listen_addrs = vec!["0.0.0.0:9000".to_string()];
        config.server.insecure_allow_remote = true;
        config.server.color = "never".to_string();
        config.server.access_log = false;

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.port, 9000);
        assert!(!parsed.server.tailscale_enabled);
        assert_eq!(parsed.server.listen_addrs, vec!["0.0.0.0:9000".to_string()]);
        assert!(parsed.server.insecure_allow_remote);
        assert_eq!(parsed.server.color, "never");
        assert!(!parsed.server.access_log);
        // The deprecated `bind` key is never re-emitted by the patcher.
        assert!(
            !saved.contains("bind ="),
            "patcher must not emit bind: {saved}"
        );
    }

    #[test]
    fn write_config_plain_round_trips_acme_settings() {
        // LESSON from the [server] slice: every managed field needs an
        // apply_patches entry or a plain/recover write silently drops it. Guard
        // the whole [server.acme] subtable against that regression.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.server.acme.enabled = true;
        config.server.acme.domains =
            vec!["dux.example.com".to_string(), "www.example.com".to_string()];
        config.server.acme.email = "ops@example.com".to_string();
        config.server.acme.http_port = 8080;
        config.server.acme.https_port = 8443;
        config.server.acme.production = false;
        config.server.acme.cache_dir = Some("/var/lib/dux/acme".to_string());

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert!(parsed.server.acme.enabled);
        assert_eq!(
            parsed.server.acme.domains,
            vec!["dux.example.com".to_string(), "www.example.com".to_string()]
        );
        assert_eq!(parsed.server.acme.email, "ops@example.com");
        assert_eq!(parsed.server.acme.http_port, 8080);
        assert_eq!(parsed.server.acme.https_port, 8443);
        assert!(!parsed.server.acme.production);
        assert_eq!(
            parsed.server.acme.cache_dir.as_deref(),
            Some("/var/lib/dux/acme")
        );
    }

    #[test]
    fn patch_preserves_existing_acme_settings() {
        // Patching an existing file must keep the [server.acme] subtable intact
        // (mirrors patch_preserves_existing_auth_users for the nested table).
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[server.acme]\nenabled = true\ndomains = [\"dux.example.com\"]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        // Mirror what a real load would do: the in-memory config carries the
        // same acme settings back into the patch.
        config.server.acme.enabled = true;
        config.server.acme.domains = vec!["dux.example.com".to_string()];

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert!(parsed.server.acme.enabled);
        assert_eq!(
            parsed.server.acme.domains,
            vec!["dux.example.com".to_string()]
        );
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
    #[cfg(unix)]
    fn write_config_plain_sets_owner_only_perms() {
        // config.toml carries bcrypt hashes ([auth]) and possibly tokens ([env]),
        // so every write must restrict it to 0600 (owner read/write only).
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config_plain(&config_path, &config).expect("write_config_plain");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config.toml must be owner-read/write only, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    #[cfg(unix)]
    fn write_config_secure_creates_fresh_file_owner_only() {
        // The create path must apply 0600 AT creation (OpenOptions::mode), so a
        // brand-new config holding secrets is never briefly world-readable. We
        // call the low-level helper directly to assert the create branch, not
        // just the post-write chmod.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        assert!(
            !config_path.exists(),
            "file must not exist before the write"
        );

        write_config_secure(&config_path, "[defaults]\nprovider = \"claude\"\n")
            .expect("write_config_secure");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a freshly created config must be owner-read/write only, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    #[cfg(unix)]
    fn patch_config_file_sets_owner_only_perms() {
        // The patch path (existing file) must also re-restrict perms to 0600 so a
        // previously-loose file is tightened on the next save.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Seed a world-readable file first.
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("seed");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644)).expect("loosen");

        let config = Config::default();
        patch_config_file(&config_path, &config).expect("patch");

        let mode = fs::metadata(&config_path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "patching config.toml must tighten perms to 0600, got {:o}",
            mode & 0o777
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
