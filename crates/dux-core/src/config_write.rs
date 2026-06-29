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
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{Array, DocumentMut, Formatted, InlineTable, Item, Table, Value};

/// Permission bits for `config.toml`: owner read/write only (`0600`). The file
/// holds bcrypt password hashes under `[auth]` and may hold tokens under `[env]`,
/// so it must not be group/world readable. Unix-only — the project targets macOS
/// and Linux (CLAUDE.md), so no `cfg(windows)` branch is needed.
const CONFIG_FILE_MODE: u32 = 0o600;

/// Whether an atomic write fsyncs the file before the rename. Eager (critical)
/// writes use `Fsync` for power-loss durability of the file's data; lazy writes
/// use `NoFsync` (crash-safe via rename, but not power-loss-durable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Fsync,
    NoFsync,
}

/// Atomically write `contents` to `path`: a temp file in the same directory
/// (created `0600`), optionally fsync'd, then `rename`d into place. The temp file
/// self-deletes on drop if the rename never happens, so a failed/panicking write
/// leaves no orphan and never a partial real file.
pub fn write_config_atomic(path: &Path, contents: &str, durability: Durability) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("config path {} has no parent directory", path.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".config.toml.")
        .tempfile_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;

    // Explicit 0600 (tempfile already defaults to this; belt-and-suspenders).
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(CONFIG_FILE_MODE))
        .with_context(|| format!("failed to chmod temp file in {}", dir.display()))?;

    tmp.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write temp config in {}", dir.display()))?;

    if durability == Durability::Fsync {
        tmp.as_file()
            .sync_all()
            .with_context(|| format!("failed to fsync temp config in {}", dir.display()))?;
    }

    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to rename temp config over {}", path.display()))?;
    Ok(())
}

/// Atomic write at the default (Fsync) durability. Kept for existing callers.
///
/// # Migration lock
///
/// This function is intentionally `#[deprecated]` so that any new unrouted caller
/// fails `cargo clippy --all-targets --all-features -- -D warnings`. This is a
/// regression guard: all runtime config writes must go through `ConfigWriteQueue`.
/// Legitimate sync-direct callers (boot, first-creation, `config regenerate`,
/// recover, bootstrap project-sync) silence the lint with `#[allow(deprecated)]`
/// and a short comment explaining why direct write is correct there.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn write_config_secure(path: &Path, contents: &str) -> Result<()> {
    write_config_atomic(path, contents, Durability::Fsync)
}

use crate::config::{Config, MacrosConfig, ProjectConfig, ProvidersConfig};

/// Patch an EXISTING `config.toml` in place, preserving the user's comments,
/// formatting, and any keys this writer doesn't manage. Reads the file, applies
/// every section patch, and writes it back atomically at [`Durability::Fsync`].
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn patch_config_file(config_path: &Path, config: &Config) -> Result<()> {
    patch_config_file_with(config_path, config, Durability::Fsync)
}

pub fn patch_config_file_with(
    config_path: &Path,
    config: &Config,
    durability: Durability,
) -> Result<()> {
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    apply_patches(&mut doc, config);
    write_config_atomic(config_path, &doc.to_string(), durability)
}

/// Save config: patch in place if the file exists, otherwise write a plain
/// (uncommented) `toml_edit` serialization from scratch. Used by surfaces that
/// don't have the TUI's canonical commented renderer (e.g. the web). The TUI
/// keeps its own `save_config` for the pretty first-creation path.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn save_config(config_path: &Path, config: &Config) -> Result<()> {
    save_config_with(config_path, config, Durability::Fsync)
}

pub fn save_config_with(config_path: &Path, config: &Config, durability: Durability) -> Result<()> {
    if config_path.exists() {
        patch_config_file_with(config_path, config, durability)
    } else {
        write_config_plain_with(config_path, config, durability)
    }
}

/// Unconditionally write a fresh plain (uncommented) `toml_edit` serialization,
/// overwriting whatever is on disk. Unlike [`save_config`], this never patches
/// an existing file, so it succeeds even when the current `config.toml` is
/// corrupt or unparseable. Used by the web's "recover config" path, which must
/// overwrite a broken file from the in-memory config.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
pub fn write_config_plain(config_path: &Path, config: &Config) -> Result<()> {
    write_config_plain_with(config_path, config, Durability::Fsync)
}

pub fn write_config_plain_with(
    config_path: &Path,
    config: &Config,
    durability: Durability,
) -> Result<()> {
    // Render the same plain (comment-free) document `render_config_plain`
    // produces, then write it atomically. Sharing the renderer keeps the on-disk
    // shape and the `recover_render` string byte-identical.
    write_config_atomic(config_path, &render_config_plain(config), durability)
}

/// Render `config` to a fresh, plain (comment-free) `config.toml` text using the
/// shared patch set against an empty document — no comments (this is the plain
/// fallback, not the TUI's pretty first-creation path). The same shape
/// [`write_config_plain`] writes, but returned as a `String` instead of written.
/// Used by a surface's `recover_render` (e.g. the web's plain recovery render) so
/// the Engine can perform the atomic write through its own writer while holding
/// the quiesce barrier.
pub fn render_config_plain(config: &Config) -> String {
    let mut doc = DocumentMut::new();
    apply_patches(&mut doc, config);
    doc.to_string()
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
    // The AI commit-message feature was removed; drop its now-obsolete prompt key
    // from any existing config so saves stop carrying it forward.
    remove_table_key(doc, "defaults", "commit_prompt");
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
        "status_clear_seconds",
        config.ui.status_clear_seconds,
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
    patch_table_bool(doc, "ui", "show_changes_pane", config.ui.show_changes_pane);
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
    patch_table_bool(
        doc,
        "server",
        "dangerously_listen_http",
        config.server.dangerously_listen_http,
    );
    patch_table_str(doc, "server", "color", &config.server.color);
    patch_table_bool(doc, "server", "access_log", config.server.access_log);
    patch_table_usize(
        doc,
        "server",
        "max_websocket_connections",
        config.server.max_websocket_connections as usize,
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

        // The AI commit-message feature was removed; drop the obsolete oneshot
        // keys from any existing provider block so saves stop carrying them.
        tbl.remove("oneshot_args");
        tbl.remove("oneshot_output");

        if let Some(hint) = &config.install_hint {
            tbl["install_hint"] = toml_edit::value(hint.as_str());
        }

        // Tri-state: write the bool only when the user pinned a value. An
        // absent key means auto-detect (forward only to a fullscreen,
        // mouse-aware child), so omit it when `None`.
        match config.forward_scroll {
            Some(value) => tbl["forward_scroll"] = toml_edit::value(value),
            None => {
                tbl.remove("forward_scroll");
            }
        }
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
#[allow(deprecated)] // tests call the deprecated wrappers directly to verify their behaviour
mod tests {
    use super::*;

    #[test]
    fn write_config_atomic_writes_0600_and_no_temp_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");

        write_config_atomic(&path, "[env]\nFOO = \"bar\"\n", Durability::Fsync).expect("write");

        let saved = fs::read_to_string(&path).expect("read");
        assert!(saved.contains("FOO = \"bar\""));
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config must be 0600, got {mode:o}");

        // No leftover temp files in the config directory.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
    }

    #[test]
    fn forward_scroll_tri_state_deserializes() {
        // Absent key -> None; explicit true/false -> Some(..).
        let absent: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"claude\"\n").expect("parse absent");
        assert_eq!(absent.forward_scroll, None);

        let yes: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"opencode\"\nforward_scroll = true\n").expect("parse true");
        assert_eq!(yes.forward_scroll, Some(true));

        let no: crate::config::ProviderCommandConfig =
            toml::from_str("command = \"codex\"\nforward_scroll = false\n").expect("parse false");
        assert_eq!(no.forward_scroll, Some(false));
    }

    #[test]
    fn patch_omits_forward_scroll_when_none_and_writes_when_some() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        // Set explicit tri-state values to exercise the writer (None omits the
        // key, Some writes it); defaults are all None (auto) regardless.
        if let Some(claude) = config.providers.commands.get_mut("claude") {
            claude.forward_scroll = None;
        }
        if let Some(opencode) = config.providers.commands.get_mut("opencode") {
            opencode.forward_scroll = Some(true);
        }
        let codex = config
            .providers
            .commands
            .get_mut("codex")
            .expect("codex provider exists");
        codex.forward_scroll = Some(false);

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");

        // Round-trips back to the same tri-state values.
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed
                .providers
                .commands
                .get("claude")
                .unwrap()
                .forward_scroll,
            None,
            "absent key must parse back to None: {saved}"
        );
        assert_eq!(
            parsed
                .providers
                .commands
                .get("opencode")
                .unwrap()
                .forward_scroll,
            Some(true)
        );
        assert_eq!(
            parsed
                .providers
                .commands
                .get("codex")
                .unwrap()
                .forward_scroll,
            Some(false)
        );

        // The writer omits the key for None and writes it for Some.
        let claude_section = saved
            .split("[providers.claude]")
            .nth(1)
            .and_then(|s| s.split("[providers.").next())
            .unwrap_or("");
        assert!(
            !claude_section.contains("forward_scroll"),
            "None must omit forward_scroll; got: {claude_section}"
        );
    }

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
        config.server.dangerously_listen_http = true;
        config.server.color = "never".to_string();
        config.server.access_log = false;
        config.server.max_websocket_connections = 42;

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.port, 9000);
        assert!(!parsed.server.tailscale_enabled);
        assert_eq!(parsed.server.listen_addrs, vec!["0.0.0.0:9000".to_string()]);
        assert!(parsed.server.insecure_allow_remote);
        assert!(
            parsed.server.dangerously_listen_http,
            "dangerously_listen_http must round-trip through a plain write"
        );
        assert_eq!(parsed.server.color, "never");
        assert!(!parsed.server.access_log);
        assert_eq!(parsed.server.max_websocket_connections, 42);
        // The deprecated `bind` key is never re-emitted by the patcher.
        assert!(
            !saved.contains("bind ="),
            "patcher must not emit bind: {saved}"
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
