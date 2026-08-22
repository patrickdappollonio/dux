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
use toml_edit::{Array, Decor, DocumentMut, Formatted, InlineTable, Item, Key, Table, Value};

/// Permission bits for `config.toml`: owner read/write only (`0600`). The file
/// may hold secrets such as tokens under `[env]`, so it must not be group/world
/// readable.
///
/// This is the ONE rule for every file dux keeps for itself, not a rule about
/// the config file in particular. It lives in [`crate::file_modes`] alongside
/// the directory mode and the tightening pass, so the database, its sidecars,
/// and the log get the same answer rather than three separate decisions.
use crate::file_modes::PRIVATE_FILE_MODE as CONFIG_FILE_MODE;

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
        // FIRST CREATION. This must emit the fully-commented template, not the
        // plain one: "the config file is the documentation" (CLAUDE.md), and the
        // patch path that runs on every later save preserves comments but never
        // ADDS them, so a config born bare stays bare forever. See
        // [`render_config_documented`].
        write_config_atomic(config_path, &render_config_documented(config), durability)
    }
}

/// The canonical fully-commented renderer, installed by the TUI at startup.
///
/// It cannot live in this crate: it needs the TUI's `RuntimeBindings` to render
/// the `[keys]` and `[macros]` comments, and `dux-core` does not depend on
/// `dux-tui`. So the binary registers it here once and every surface that
/// creates a config file (the TUI, and `dux server`'s bootstrap project-sync)
/// gets the documented output from the same source.
static CANONICAL_RENDERER: std::sync::OnceLock<fn(&Config) -> String> = std::sync::OnceLock::new();

/// Install the fully-commented renderer. Idempotent; the first caller wins.
pub fn set_canonical_renderer(renderer: fn(&Config) -> String) {
    let _ = CANONICAL_RENDERER.set(renderer);
}

/// Render `config` with comments when a canonical renderer has been installed,
/// falling back to the plain render otherwise.
///
/// The fallback is deliberate rather than a panic: a comment-free config is
/// degraded, not broken, and `dux config restore-docs` can add the comments
/// later. Refusing to write would lose the user's settings outright, which is a
/// far worse failure than losing the prose.
pub fn render_config_documented(config: &Config) -> String {
    match CANONICAL_RENDERER.get() {
        Some(render) => render(config),
        None => render_config_plain(config),
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
    // --- top-level (no table) keys ---
    // A dotless root key must render before any table header or TOML would parse
    // it as belonging to the preceding table. `patch_root_u16` positions it at
    // the front of the document, so this is order-safe whether the doc is empty
    // (plain render) or an existing user file already full of tables (patch).
    patch_root_u16(
        doc,
        "shutdown_timeout_seconds",
        config.shutdown_timeout_seconds,
    );

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
    patch_table_bool(
        doc,
        "defaults",
        "copy_uncommitted_changes_by_default",
        config.defaults.copy_uncommitted_changes_by_default,
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
    patch_table_u16(doc, "ui", "agent_tabs_max", config.ui.agent_tabs_max);
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
    patch_table_u16(
        doc,
        "ui",
        "pr_poll_interval_seconds",
        config.ui.pr_poll_interval_seconds,
    );
    patch_table_bool(doc, "ui", "copy_on_select", config.ui.copy_on_select);
    patch_table_str(
        doc,
        "ui",
        "terminal_font_family",
        &config.ui.terminal_font_family,
    );
    patch_table_u16(
        doc,
        "ui",
        "terminal_font_size",
        config.ui.terminal_font_size,
    );
    patch_table_str(doc, "ui", "compose_bar", &config.ui.compose_bar);
    patch_table_bool(doc, "ui", "mobile_top_bar", config.ui.mobile_top_bar);
    patch_table_bool(
        doc,
        "ui",
        "mobile_accessory_bar",
        config.ui.mobile_accessory_bar,
    );
    patch_table_u64(
        doc,
        "ui",
        "attention_grace_seconds",
        config.ui.attention_grace_seconds,
    );
    patch_table_bool(
        doc,
        "ui",
        "auto_reopen_agents",
        config.ui.auto_reopen_agents,
    );
    patch_table_bool(doc, "ui", "show_changes_pane", config.ui.show_changes_pane);
    patch_table_bool(
        doc,
        "ui",
        "always_show_tab_strip",
        config.ui.always_show_tab_strip,
    );
    patch_table_str(doc, "ui", "upload_directory", &config.ui.upload_directory);
    patch_table_bool(
        doc,
        "ui",
        "upload_write_gitignore",
        config.ui.upload_write_gitignore,
    );
    patch_table_usize(
        doc,
        "ui",
        "upload_pasted_text_chars",
        config.ui.upload_pasted_text_chars,
    );
    patch_table_bool(
        doc,
        "ui",
        "attention_indicator",
        config.ui.attention_indicator,
    );
    patch_table_bool(doc, "ui", "attention_on_bell", config.ui.attention_on_bell);
    patch_table_bool(
        doc,
        "ui",
        "disable_automated_welcome_screen",
        config.ui.disable_automated_welcome_screen,
    );
    patch_table_bool(
        doc,
        "ui",
        "disable_release_notes",
        config.ui.disable_release_notes,
    );
    patch_table_str(
        doc,
        "ui",
        "pr_banner_position",
        &config.ui.pr_banner_position,
    );
    patch_table_str(doc, "ui", "agent_sort", &config.ui.agent_sort);
    patch_table_str(doc, "ui", "theme", &config.ui.theme);

    // --- [capabilities] ---
    patch_table_str(
        doc,
        "capabilities",
        "terminal_identity",
        &config.capabilities.terminal_identity,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "passthrough",
        config.capabilities.passthrough,
    );
    patch_table_str(
        doc,
        "capabilities",
        "clipboard_passthrough",
        &config.capabilities.clipboard_passthrough,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "hyperlinks",
        config.capabilities.hyperlinks,
    );
    patch_table_bool(
        doc,
        "capabilities",
        "web_notifications",
        config.capabilities.web_notifications,
    );

    // --- [editor] ---
    patch_table_str(doc, "editor", "default", &config.editor.default);

    // --- [server] ---
    // The deprecated `bind` field is migrated away on load and is never
    // re-emitted here, so a patch/recover/plain write produces the new
    // host/port shape only.
    patch_table_str(doc, "server", "host", &config.server.host);
    patch_table_u16(doc, "server", "port", config.server.port);
    patch_table_str(doc, "server", "tailscale", &config.server.tailscale);
    // The boolean `tailscale_enabled` became the tri-state `tailscale`. Load-time
    // migration already rewrote it in memory; dropping it here is what takes it
    // OUT of the file, so a save stops carrying a key dux no longer reads (the
    // same one-line strip `max_websocket_connections` gets below).
    remove_table_key(doc, "server", "tailscale_enabled");
    patch_table_string_array(doc, "server", "allowed_hosts", &config.server.allowed_hosts);
    patch_table_str(doc, "server", "color", &config.server.color);
    patch_table_bool(doc, "server", "access_log", config.server.access_log);
    patch_table_bool(
        doc,
        "server",
        "serve_while_tui",
        config.server.serve_while_tui,
    );
    // The single WebSocket cap was split into three per-class caps; drop the
    // obsolete key from any existing config block on every save so saves stop
    // carrying it (mirrors the oneshot strip in `patch_providers`). Warn when
    // the key is actually present so a TUI user (who never calls load_config on
    // the server path) still sees the migration notice in dux.log and on stderr.
    if remove_table_key_item(doc, "server", "max_websocket_connections").is_some() {
        let msg = "[server] max_websocket_connections has been removed and is being ignored. \
            It was split into max_websocket_events_connections, \
            max_websocket_agent_connections, and max_websocket_terminal_connections. \
            Set those per-class caps instead; a value of 0 still means disable \
            (refuse all new connections of that class until restart).";
        crate::logger::warn(msg);
        eprintln!("dux config migration warning: {msg}");
    }
    patch_table_usize(
        doc,
        "server",
        "max_websocket_events_connections",
        config.server.max_websocket_events_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_agent_connections",
        config.server.max_websocket_agent_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_terminal_connections",
        config.server.max_websocket_terminal_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_tab_connections",
        config.server.max_websocket_tab_connections as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "max_websocket_tabs_per_agent",
        config.server.max_websocket_tabs_per_agent as usize,
    );
    patch_table_str(doc, "server", "title", &config.server.title);
    patch_table_str(doc, "server", "favicon", &config.server.favicon);
    patch_table_u16(
        doc,
        "server",
        "shutdown_timeout_seconds",
        config.server.shutdown_timeout_seconds,
    );
    patch_table_usize(
        doc,
        "server",
        "search_index_max_files",
        config.server.search_index_max_files,
    );
    patch_table_usize(
        doc,
        "server",
        "tree_list_max_concurrency",
        config.server.tree_list_max_concurrency as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "release_notes_max_concurrency",
        config.server.release_notes_max_concurrency as usize,
    );
    patch_table_usize(
        doc,
        "server",
        "file_drop_max_bytes",
        config.server.file_drop_max_bytes,
    );
    patch_table_usize(
        doc,
        "server",
        "file_drop_max_concurrency",
        config.server.file_drop_max_concurrency as usize,
    );

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

/// Set a dotless key at the document root. With the pinned `toml_edit`, a
/// table's own key/value pairs render before its child tables, so a root leaf
/// key emits at the top of the document, ahead of every `[table]` header — valid
/// TOML whether the document is empty (plain render) or an existing user file
/// already full of tables (patch). That ordering is emergent encoder behavior,
/// not a documented `toml_edit` API guarantee, so it is not assumed blindly: the
/// `root_key_renders_before_tables_and_round_trips` and
/// `patch_adds_root_key_to_existing_file_without_corruption` tests re-parse the
/// rendered output and would fail loudly if a `toml_edit` upgrade ever moved the
/// bare key after a table header (which would otherwise parse it into that
/// table). If they break, this key needs an explicit position fix-up here.
fn patch_root_u16(doc: &mut DocumentMut, key: &str, value: u16) {
    doc[key] = toml_edit::value(i64::from(value));
}

fn patch_table_usize(doc: &mut DocumentMut, section: &str, key: &str, value: usize) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value as i64);
}

fn patch_table_bool(doc: &mut DocumentMut, section: &str, key: &str, value: bool) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value);
}

fn patch_table_u64(doc: &mut DocumentMut, section: &str, key: &str, value: u64) {
    let table = ensure_table(doc, section);
    table[key] = toml_edit::value(value as i64);
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

        // The drag-and-drop paste form for the WEB UI. Every provider dux ships
        // carries one (`ensure_defaults` fills it in), so in practice this writes;
        // a provider the user added themselves has none and an absent key means
        // `bare`, so omit it rather than inventing a value they did not choose.
        match &config.web_dragdrop_paste {
            Some(value) => tbl["web_dragdrop_paste"] = toml_edit::value(value.as_str()),
            None => {
                tbl.remove("web_dragdrop_paste");
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

    // Add or update entries, IN `macros.entries` ORDER. Order is meaningful
    // (the macro bar, the web quick-picker, and the editor list all render in
    // declaration order, and the web editor reorders by drag-and-drop through
    // this same wholesale save), and `table[name] = ...` on an existing key
    // keeps its OLD toml_edit position — so a pure reorder used to round-trip
    // through the in-memory config while the file kept the old order, and a
    // dux restart silently undid it. Removing each key first and re-inserting
    // with `insert_formatted` appends in iteration order while carrying the
    // key's own decor (a comment the user wrote above a macro line) with it.
    for (name, entry) in &macros.entries {
        let existing_key = table.remove_entry(name).map(|(key, _)| key);
        let mut inline = InlineTable::new();
        inline.insert("text", Value::String(Formatted::new(entry.text.clone())));
        inline.insert(
            "surface",
            Value::String(Formatted::new(entry.surface.as_config_str().to_string())),
        );
        let item = toml_edit::value(Value::InlineTable(inline));
        match existing_key {
            Some(key) => {
                table.insert_formatted(&key, item);
            }
            None => {
                table[name] = item;
            }
        }
    }
}

/// The keys [`patch_projects`] writes itself. Everything else it finds in an
/// existing `[[projects]]` entry belongs to the user (a hand-added note, a key
/// from a newer dux, a fork's own setting) and is carried over verbatim, except
/// for [`PROJECT_KEYS_DROPPED_ON_WRITE`].
const PROJECT_MANAGED_KEYS: &[&str] = &[
    "id",
    "path",
    "name",
    "default_provider",
    "auto_reopen_agents",
    "startup_command",
    "env",
];

/// Project keys dux once wrote to config and now keeps in SQLite instead. These
/// are DROPPED on every write rather than carried over: `leading_branch` is
/// autodetected runtime state, and leaving it in portable config pins the
/// detection (CLAUDE.md, "Keep derived project state in SQLite"). The loader
/// still reads it to repair SQLite, which is why it stays a `ProjectConfig`
/// field; only the writer refuses to emit it.
const PROJECT_KEYS_DROPPED_ON_WRITE: &[&str] = &["leading_branch"];

/// One existing `[[projects]]` entry's user-owned material, captured before the
/// rebuild replaces it.
#[derive(Default)]
struct CarriedProject {
    /// The entry's `id`, when it wrote one. A hand-written entry may legally leave
    /// it out: `ProjectConfig::id` carries `#[serde(default = "new_project_id")]`,
    /// so the loader mints an identifier and the file works. Such an entry used to
    /// be SKIPPED here, which deleted both its unmanaged keys and its comment on
    /// the next save (measured on the file in
    /// `patch_keeps_the_keys_and_comment_of_an_entry_that_carries_no_id`).
    id: Option<String>,
    /// The entry's `path` AS SPELLED IN THE FILE. It is the fallback identity for
    /// an entry with no id, and the tie-breaker between two entries that share one.
    ///
    /// The raw spelling is deliberately not normalized: the loader env-expands the
    /// path, so `$HOME/p` in the file is `/home/user/p` in memory and the two do
    /// not compare equal. A miss is the correct outcome there. Guessing past it
    /// would mean attaching one project's keys to another project's entry, which is
    /// worse than dropping them.
    path: Option<String>,
    /// The comment block the user wrote ABOVE the entry, as comment lines with the
    /// surrounding whitespace already dropped (see [`carried_comment_prefix`]).
    /// `toml_edit` files it on the entry's own decor rather than on any of its keys,
    /// so carrying the keys alone deleted it.
    ///
    /// Present for BOTH spellings. The array-of-tables spelling keeps it on the
    /// table's decor; the multi-line `projects = [ ... ]` spelling can carry a
    /// comment between its elements (legal TOML, and pure user data), and
    /// `toml_edit` files that on the following element's value decor.
    header_comment: Option<String>,
    /// The comment block written between the `[[projects]]` header and the entry's
    /// FIRST key. `toml_edit` files that as the first key's prefix, and the first
    /// key is `id`, which this writer rebuilds from scratch, so it reached neither
    /// the decor carry nor the key carry and was deleted.
    first_key_comment: Option<String>,
    /// The keys this writer does not manage, each with its own [`Key`] so a
    /// comment attached to it travels along, exactly as in
    /// [`merge_unmanaged_keys`].
    keys: Vec<(Key, Item)>,
}

/// Every key and comment the user's own `[[projects]]` entries carry that this
/// writer does not manage, in FILE ORDER. Each entry keeps its own identity
/// (`id`, `path`) so it can be matched back to the right project even when
/// projects were added, removed, or reordered between saves.
///
/// Captured BEFORE the rebuild below. `patch_projects` replaces the whole array
/// (it has to: a project can be removed, and per-project keys are optional, so
/// there is no in-place edit that covers every case), and without this capture
/// that replacement silently deleted anything the user had put there.
///
/// BOTH legal spellings of the array are read. `[[projects]]` (an array of
/// tables) is what dux writes, but `projects = [ { ... } ]` (an array of inline
/// tables) parses to the identical `Config`, so a user who hand-wrote that form
/// has a working config, and reading only one spelling deleted their keys on the
/// next save.
///
/// Every entry is captured, including one with no unmanaged keys and one with no
/// `id` at all, because the comments hang off the entry rather than off its keys.
/// [`take_carried_project`] owns the matching rules.
fn unmanaged_project_keys(doc: &DocumentMut) -> Vec<Option<CarriedProject>> {
    let mut carried: Vec<Option<CarriedProject>> = Vec::new();
    match doc.get("projects") {
        Some(Item::ArrayOfTables(existing)) => {
            for table in existing.iter() {
                let keys: Vec<(Key, Item)> = table
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .filter(|name| is_unmanaged_project_key(name))
                    .filter_map(|name| {
                        table
                            .get_key_value(&name)
                            .map(|(key, item)| (key.clone(), item.clone()))
                    })
                    .collect();
                // Only when the first key is one this writer REBUILDS. An
                // unmanaged key written first keeps that comment on its own decor
                // and is carried with it, so re-homing it onto `id` as well
                // duplicated it further down the file on every save.
                let first_key_comment = table
                    .iter()
                    .next()
                    .filter(|(name, _)| !is_unmanaged_project_key(name))
                    .and_then(|(name, _)| table.get_key_value(name))
                    .and_then(|(key, _)| decor_comment(key.leaf_decor()));
                carried.push(Some(CarriedProject {
                    id: table.get("id").and_then(Item::as_str).map(str::to_string),
                    path: table.get("path").and_then(Item::as_str).map(str::to_string),
                    header_comment: decor_comment(table.decor()),
                    first_key_comment,
                    keys,
                }));
            }
        }
        Some(Item::Value(Value::Array(existing))) => {
            for value in existing.iter() {
                let Some(inline) = value.as_inline_table() else {
                    continue;
                };
                let keys: Vec<(Key, Item)> = inline
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .filter(|name| is_unmanaged_project_key(name))
                    .filter_map(|name| {
                        inline
                            .get_key_value(&name)
                            .map(|(key, item)| (strip_inline_decor(key), item.clone()))
                    })
                    .map(|(key, item)| (key, block_spaced_item(item)))
                    .collect();
                carried.push(Some(CarriedProject {
                    id: inline.get("id").and_then(Value::as_str).map(str::to_string),
                    path: inline
                        .get("path")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    // A comment written between two elements of a multi-line array
                    // is filed as the prefix of the element that FOLLOWS it, which
                    // is where an inline entry's comment lives. (A comment after the
                    // LAST element is the array's own trailing trivia and is not an
                    // entry's data, the same call the array-of-tables spelling makes
                    // for a comment below the last key.)
                    header_comment: decor_comment(value.decor()),
                    // An inline table is one line, so there is no "between the
                    // header and the first key" position to capture.
                    first_key_comment: None,
                    keys,
                }));
            }
        }
        // No projects yet, or a `projects` key of some other type entirely (a
        // string, say). Nothing to carry, and the rebuild replaces it.
        _ => {}
    }
    carried
}

/// The captured entry that belongs to `project`, removed from `carried` so no two
/// projects can claim the same one.
///
/// Identity is tried in three steps, most specific first:
///
/// 1. the PAIR of `id` and raw `path`. Matching on the id alone kept each entry's
///    keys with whatever landed in the same SLOT, so two entries sharing an id
///    (a hand-edit the project sync rejects, but that a file can hold) swapped
///    their keys the moment the two projects were reordered in memory: measured,
///    `/b` came back carrying `/a`'s key.
/// 2. the `id` alone, because the raw path is not always comparable: the loader
///    env-expands it, so a file that spells it `$HOME/p` holds `/home/ada/p` in
///    memory and never matches on the pair.
/// 3. the raw `path` alone, and only for an entry that carried NO id, whose id was
///    minted by the loader and therefore cannot match anything in the file.
///
/// Step 3 accepts a miss (an id-less entry whose path is env-expanded loses its
/// extras) rather than guessing, because the wrong guess attaches one project's
/// keys and comments to a different project's worktree.
fn take_carried_project(
    carried: &mut [Option<CarriedProject>],
    project: &ProjectConfig,
) -> Option<CarriedProject> {
    let position = |matches: &dyn Fn(&CarriedProject) -> bool| {
        carried
            .iter()
            .position(|slot| slot.as_ref().is_some_and(matches))
    };
    let found = position(&|entry: &CarriedProject| {
        entry.id.as_deref() == Some(project.id.as_str())
            && entry.path.as_deref() == Some(project.path.as_str())
    })
    .or_else(|| {
        position(&|entry: &CarriedProject| entry.id.as_deref() == Some(project.id.as_str()))
    })
    .or_else(|| {
        position(&|entry: &CarriedProject| {
            entry.id.is_none() && entry.path.as_deref() == Some(project.path.as_str())
        })
    })?;
    carried[found].take()
}

/// A key carried out of an inline table, with the inline spacing dropped.
///
/// Inside `{ id = "a", note = 1 }` the key records a leading and trailing space of
/// its own; pasted into a block table those become ` note = 1 `. The document still
/// reparses, so this is cosmetic, but it accumulates over saves.
fn strip_inline_decor(key: &Key) -> Key {
    let mut key = key.clone();
    *key.leaf_decor_mut() = Decor::default();
    key
}

/// The same for the value half of a carried inline entry: back to the default
/// decor, so the block table spaces it the way it spaces everything else.
fn block_spaced_item(mut item: Item) -> Item {
    if let Some(value) = item.as_value_mut() {
        *value.decor_mut() = Decor::default();
    }
    item
}

/// Whether a key found in an existing project entry is the user's rather than
/// this writer's, and so has to be carried over.
fn is_unmanaged_project_key(name: &str) -> bool {
    !PROJECT_MANAGED_KEYS.contains(&name) && !PROJECT_KEYS_DROPPED_ON_WRITE.contains(&name)
}

/// The comment lines in a captured decor prefix, or `None` when it holds only the
/// whitespace `toml_edit` records by default.
///
/// The whitespace is deliberately NOT kept. Pasting the recorded prefix verbatim
/// re-used the source entry's own separation, and the file's FIRST entry has no
/// leading blank line because nothing precedes it, so moving that project to
/// second position ran the two entries together (measured in
/// `a_commented_project_moved_later_keeps_a_blank_line_before_its_header`). Only
/// the comment lines are the user's data; the spacing around them belongs to
/// wherever the entry ends up.
fn decor_comment(decor: &Decor) -> Option<String> {
    let prefix = decor.prefix().and_then(|prefix| prefix.as_str())?;
    let comments: Vec<&str> = prefix
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .collect();
    (!comments.is_empty()).then(|| comments.join("\n"))
}

fn patch_projects(doc: &mut DocumentMut, projects: &[ProjectConfig]) {
    let mut carried = unmanaged_project_keys(doc);
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
        // Put the user's own keys and comments back. The source entry is matched
        // by identity rather than by position (see `take_carried_project`) and is
        // consumed, so no two rebuilt entries can claim the same one.
        if let Some(extras) = take_carried_project(&mut carried, project) {
            for (key, item) in extras.keys {
                table.insert_formatted(&key, item);
            }
            // The comment block the user wrote above this entry's header, which
            // lives on the entry's decor rather than on any of its keys. The
            // leading newline is what separates it from whatever now precedes it,
            // whether or not anything did in the source file.
            if let Some(comment) = extras.header_comment {
                table.decor_mut().set_prefix(format!("\n{comment}\n"));
            }
            // ...and the comment block between the header and the first key, which
            // `toml_edit` files as the prefix of that first key. The rebuilt first
            // key is always `id`, written just above.
            if let Some(comment) = extras.first_key_comment
                && let Some(mut key) = table.key_mut("id")
            {
                key.leaf_decor_mut().set_prefix(format!("{comment}\n"));
            }
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

// ---------------------------------------------------------------------------
// Documentation restore: merging a user's unmanaged keys into a fresh render
// ---------------------------------------------------------------------------

/// Sections dux once wrote but no longer reads. They survive in real user files
/// only because the surgical patch path preserves unknown keys, so a config that
/// predates their removal carries them forever.
///
/// Anything listed here is REMOVED by the documentation-restore merge (and the
/// removal is reported to the user — a silent drop is data loss even when the
/// data was inert). Everything NOT listed here is preserved verbatim: a user may
/// hand-add keys, run a fork, or have keys from a newer dux.
///
/// Entries are dotted table paths. A path matches when it is equal to an entry
/// or nested beneath one.
pub const ORPHANED_CONFIG_SECTIONS: &[&str] = &[
    // Removed with the HTTP-basic-auth experiment. The server is single-tenant /
    // trusted-access by design (CLAUDE.md) and has no login of any kind.
    "auth",
    // Removed with the built-in ACME/TLS listener. TLS is delegated to an
    // upstream proxy or to Tailscale.
    "server.acme",
];

/// One step of a path through a TOML document: a table key, or an index into an
/// array of tables (`[[projects]]`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PathSeg {
    Key(String),
    Index(usize),
}

/// Render a path for display: `server.acme.production`, `projects[0].custom_key`.
fn path_display(path: &[PathSeg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            PathSeg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            PathSeg::Index(i) => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("[{i}]"));
            }
        }
    }
    out
}

/// The dotted TABLE path (indices elided), used for drop-list matching so that
/// `projects[0].auth` never collides with the top-level `[auth]` section.
fn dotted_key_path(path: &[PathSeg]) -> String {
    let keys: Vec<&str> = path
        .iter()
        .map(|seg| match seg {
            PathSeg::Key(k) => k.as_str(),
            PathSeg::Index(_) => "[]",
        })
        .collect();
    keys.join(".")
}

/// Whether `dotted` is exactly an orphaned section.
fn is_orphan_root(dotted: &str) -> bool {
    ORPHANED_CONFIG_SECTIONS.contains(&dotted)
}

/// What the documentation-restore merge did to a user's non-canonical content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreMergeReport {
    /// Dotted paths of orphaned sections that were removed.
    pub dropped: Vec<String>,
    /// Dotted paths of unknown keys that were carried over verbatim.
    pub preserved: Vec<String>,
    /// Dotted paths of unknown keys the merge could NOT place anywhere in the
    /// rendered document, and which are therefore absent from the result.
    ///
    /// Distinct from [`Self::dropped`], which names sections dux removes ON
    /// PURPOSE. This list names a failure, and it exists so the failure can
    /// never be silent. It is empty for every config the canonical renderer
    /// can produce (see `insert_at_path`), so a non-empty one is a bug report.
    pub unplaceable: Vec<String>,
}

impl RestoreMergeReport {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty() && self.preserved.is_empty() && self.unplaceable.is_empty()
    }
}

/// Copy every key of `original` that the freshly-rendered `rendered` document
/// does not already carry into `rendered`, EXCEPT keys under
/// [`ORPHANED_CONFIG_SECTIONS`], which are dropped. Returns what happened.
///
/// This is the safety net under "restore the documentation": the canonical
/// renderer only emits keys dux knows about, so re-rendering a user's config
/// from its parsed [`Config`] would otherwise silently discard anything else in
/// the file. Values are moved as `toml_edit` items, so a preserved key keeps its
/// own formatting and its own trailing/leading comments.
pub fn merge_unmanaged_keys(
    rendered: &mut DocumentMut,
    original: &DocumentMut,
) -> RestoreMergeReport {
    let mut report = RestoreMergeReport::default();
    let mut carry: Vec<CarriedLeaf> = Vec::new();
    let mut path = Vec::new();
    collect_unmanaged(
        original.as_table(),
        Some(rendered.as_table()),
        &mut path,
        &mut carry,
        &mut report.dropped,
    );

    for leaf in carry {
        let display = path_display(&leaf.path);
        if insert_at_path(rendered, &leaf.path, leaf.key, leaf.item) {
            report.preserved.push(display);
        } else {
            // Neither preserved nor deliberately dropped: the key is GONE and
            // the user has to be told. Falling through to neither list is how
            // a merge loses data in silence.
            report.unplaceable.push(display);
        }
    }
    report.dropped.sort();
    report.dropped.dedup();
    report.preserved.sort();
    report.unplaceable.sort();
    report
}

/// A key the rendered document lacks, captured with its own `Key` so that the
/// comment attached to it (which lives on the key's decor, not the item's)
/// travels with it into the restored file.
struct CarriedLeaf {
    path: Vec<PathSeg>,
    key: Key,
    item: Item,
}

/// Walk `orig` alongside its counterpart in the rendered document, collecting
/// leaf keys the rendered document lacks and noting dropped orphan sections.
fn collect_unmanaged(
    orig: &Table,
    rendered: Option<&Table>,
    path: &mut Vec<PathSeg>,
    carry: &mut Vec<CarriedLeaf>,
    dropped: &mut Vec<String>,
) {
    for (key, item) in orig.iter() {
        path.push(PathSeg::Key(key.to_string()));
        let dotted = dotted_key_path(path);

        if is_orphan_root(&dotted) {
            // Report the section once and do not descend: everything beneath it
            // goes away with it.
            dropped.push(dotted);
            path.pop();
            continue;
        }

        match item {
            Item::Table(table) => {
                let counterpart = rendered.and_then(|r| r.get(key)).and_then(Item::as_table);
                collect_unmanaged(table, counterpart, path, carry, dropped);
            }
            Item::ArrayOfTables(arrays) => {
                let counterpart = rendered
                    .and_then(|r| r.get(key))
                    .and_then(Item::as_array_of_tables);
                for (index, table) in arrays.iter().enumerate() {
                    path.push(PathSeg::Index(index));
                    collect_unmanaged(
                        table,
                        counterpart.and_then(|a| a.get(index)),
                        path,
                        carry,
                        dropped,
                    );
                    path.pop();
                }
            }
            leaf => {
                let already_rendered = rendered.map(|r| r.contains_key(key)).unwrap_or(false);
                if !already_rendered && let Some(owned_key) = orig.key(key) {
                    carry.push(CarriedLeaf {
                        path: path.clone(),
                        key: owned_key.clone(),
                        item: leaf.clone(),
                    });
                }
            }
        }
        path.pop();
    }
}

/// Insert `item` at `path` in `doc`, creating intermediate tables as needed.
///
/// Returns false when the path cannot be materialized — the only such case is an
/// array-of-tables index the rendered document does not have (dux cannot invent
/// a `[[projects]]` entry that the canonical renderer did not emit).
fn insert_at_path(doc: &mut DocumentMut, path: &[PathSeg], key: Key, item: Item) -> bool {
    let Some((PathSeg::Key(_), parents)) = path.split_last() else {
        return false;
    };

    let mut table: &mut Table = doc.as_table_mut();
    let mut step = 0;
    while step < parents.len() {
        let PathSeg::Key(key) = &parents[step] else {
            // An index never leads a path: it always follows the key naming the
            // array it indexes into.
            return false;
        };

        if let Some(PathSeg::Index(index)) = parents.get(step + 1) {
            // `key[index]` — descend into an existing array-of-tables entry. dux
            // cannot invent an entry the canonical renderer did not emit, so a
            // missing array or index means "cannot preserve here".
            let Some(arrays) = table.get_mut(key).and_then(Item::as_array_of_tables_mut) else {
                return false;
            };
            let Some(next) = arrays.get_mut(*index) else {
                return false;
            };
            table = next;
            step += 2;
            continue;
        }

        // Plain table step, creating an implicit table when absent. An implicit
        // table emits no `[header]` line of its own, so a synthetic parent that
        // exists only to hold a preserved leaf adds no stray empty section.
        let entry = table.entry(key).or_insert_with(|| {
            let mut fresh = Table::new();
            fresh.set_implicit(true);
            Item::Table(fresh)
        });
        let Some(next) = entry.as_table_mut() else {
            return false;
        };
        table = next;
        step += 1;
    }

    // `insert_formatted` (rather than `insert`) is what carries the key's own
    // decor — including any comment written above it — into the restored file.
    table.insert_formatted(&key, item);
    true
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

    /// A pure REORDER of the macro set must reach the file. `[macros]` order is
    /// meaningful (the macro bar, quick-picker, and web editor all render in
    /// declaration order), and the web editor now reorders by drag-and-drop
    /// through the same wholesale `update_macros` save. Writing an existing key
    /// with `table[name] = ...` keeps its old toml_edit position, so before this
    /// was pinned a reorder round-tripped through the in-memory config but the
    /// file kept the old order and a dux restart silently undid the drag.
    #[test]
    fn patch_macros_writes_entries_in_config_order() {
        let raw = "\
[macros]
# my review macro
review = { text = \"review this\", surface = \"agent\" }
build = { text = \"cargo build\", surface = \"terminal\" }
deploy = { text = \"deploy it\", surface = \"both\" }
";
        let mut doc: DocumentMut = raw.parse().expect("parse");

        // Same three entries, reordered: deploy, review, build.
        let mut macros = MacrosConfig::default();
        for (name, text, surface) in [
            ("deploy", "deploy it", crate::config::MacroSurface::Both),
            ("review", "review this", crate::config::MacroSurface::Agent),
            (
                "build",
                "cargo build",
                crate::config::MacroSurface::Terminal,
            ),
        ] {
            macros.entries.insert(
                name.to_string(),
                crate::config::MacroEntry {
                    text: text.to_string(),
                    surface,
                },
            );
        }
        patch_macros(&mut doc, &macros);

        let saved = doc.to_string();
        let pos = |needle: &str| {
            saved
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(
            pos("deploy") < pos("review") && pos("review") < pos("build"),
            "entries must be written in the new order, got:\n{saved}"
        );
        // The comment above `review` travels with its key through the reorder.
        assert!(
            pos("# my review macro") < pos("review ="),
            "the key's comment must survive and stay attached, got:\n{saved}"
        );

        // The reparsed config sees the new order, so a restart keeps it.
        let reparsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            reparsed.macros.entries.keys().collect::<Vec<_>>(),
            vec!["deploy", "review", "build"]
        );
    }

    /// The reorder rewrite must not clobber entries that changed CONTENT in the
    /// same save, and stale keys still disappear.
    #[test]
    fn patch_macros_reorder_carries_edits_and_removals() {
        let raw = "\
[macros]
review = { text = \"review this\", surface = \"agent\" }
gone = { text = \"bye\", surface = \"both\" }
build = { text = \"cargo build\", surface = \"terminal\" }
";
        let mut doc: DocumentMut = raw.parse().expect("parse");

        let mut macros = MacrosConfig::default();
        macros.entries.insert(
            "build".to_string(),
            crate::config::MacroEntry {
                text: "cargo build --release".to_string(),
                surface: crate::config::MacroSurface::Terminal,
            },
        );
        macros.entries.insert(
            "review".to_string(),
            crate::config::MacroEntry {
                text: "review this".to_string(),
                surface: crate::config::MacroSurface::Agent,
            },
        );
        patch_macros(&mut doc, &macros);

        let saved = doc.to_string();
        assert!(
            !saved.contains("gone"),
            "stale key must be removed:\n{saved}"
        );
        let reparsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            reparsed.macros.entries.keys().collect::<Vec<_>>(),
            vec!["build", "review"]
        );
        assert_eq!(
            reparsed.macros.entries["build"].text,
            "cargo build --release"
        );
    }

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

    /// A REPLACE, not an edit in place, and the mode of the destination does
    /// not survive it. The write goes to a fresh `0600` temp file which is then
    /// `rename`d over the original, and a rename needs write permission on the
    /// DIRECTORY, not on the file, so a `config.toml` the user deliberately
    /// made read-only at `0400` is replaced anyway and comes back `0600`.
    ///
    /// This is pinned because the shipped template comment used to promise the
    /// opposite. "dux only ever removes group and world access, so a `0400`
    /// config stays read-only" is true of the tightening pass in
    /// [`crate::file_modes`] and false of this function, and the comment did
    /// not say which one it was talking about.
    ///
    /// Preserving the destination mode instead was considered and deliberately
    /// NOT done. It would carry a `0644` forward on every save, undoing the
    /// startup tightening for a file that holds `[env]` tokens, and preserving
    /// `0400` would make dux look as though it had honoured the read-only file
    /// while having replaced its contents regardless, which is the more
    /// misleading of the two outcomes. The honest fix was to stop claiming it.
    #[test]
    fn write_config_atomic_replaces_a_read_only_config_and_resets_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[env]\nOLD = \"1\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();

        write_config_atomic(&path, "[env]\nNEW = \"2\"\n", Durability::Fsync).expect("write");

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[env]\nNEW = \"2\"\n",
            "the read-only file was replaced; nothing here refuses the write"
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "owner write comes back, got {mode:o}");
    }

    #[test]
    fn root_key_renders_before_tables_and_round_trips() {
        let rendered = render_config_plain(&Config::default());

        // The dotless root key must appear before the first table header, or TOML
        // would bind it to a table. Guards the toml_edit ordering assumption.
        let first_table = rendered.find('[').expect("rendered config has tables");
        let key_pos = rendered
            .find("shutdown_timeout_seconds")
            .expect("root shutdown_timeout_seconds present");
        assert!(
            key_pos < first_table,
            "root shutdown_timeout_seconds must render before any table:\n{rendered}"
        );

        // And it must parse back to the defaults (30 at root and under [server]).
        let parsed: Config = toml::from_str(&rendered).expect("rendered config re-parses");
        assert_eq!(parsed.shutdown_timeout_seconds, 30);
        assert_eq!(parsed.server.shutdown_timeout_seconds, 30);
    }

    #[test]
    fn patch_adds_root_key_to_existing_file_without_corruption() {
        // An existing user file already full of tables and comments — the worst
        // case for appending a dotless root key.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        let original = "# my dux config\n\
                        [defaults]\n\
                        provider = \"claude\"\n\n\
                        # keep this comment\n\
                        [server]\n\
                        port = 9000\n";
        fs::write(&path, original).expect("seed config");

        let config = Config {
            shutdown_timeout_seconds: 12,
            server: crate::config::ServerConfig {
                shutdown_timeout_seconds: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");

        let saved = fs::read_to_string(&path).expect("read back");
        // Must still be valid TOML and the root key must not have been swallowed
        // into [server] (which would make it parse as 0/default, not 12).
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.shutdown_timeout_seconds, 12, "saved:\n{saved}");
        assert_eq!(parsed.server.shutdown_timeout_seconds, 7);
        // User comments are preserved by the surgical patch.
        assert!(saved.contains("# keep this comment"), "saved:\n{saved}");
    }

    #[test]
    fn zero_timeout_round_trips() {
        let config = Config {
            shutdown_timeout_seconds: 0,
            server: crate::config::ServerConfig {
                shutdown_timeout_seconds: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.shutdown_timeout_seconds, 0);
        assert_eq!(parsed.server.shutdown_timeout_seconds, 0);
    }

    #[test]
    fn compose_bar_renders_and_round_trips() {
        // The default ("auto") renders and re-parses.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.ui.compose_bar, "auto");

        // A user-set mode survives a regenerate. This is the half that catches
        // a missing `patch_table_str` line: with the key absent from the
        // render, the re-parse would silently fall back to the default.
        for mode in ["always", "never"] {
            let config = Config {
                ui: crate::config::UiConfig {
                    compose_bar: mode.to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let rendered = render_config_plain(&config);
            let parsed: Config = toml::from_str(&rendered).expect("re-parse");
            assert_eq!(parsed.ui.compose_bar, mode);
        }
    }

    /// The bool-to-enum migration survives the SAVE path, not just the load
    /// path: a config file still holding the old boolean is read through
    /// `deserialize_compose_bar` and written back out as the string form, so
    /// the legacy value is retired the first time anything saves.
    #[test]
    fn a_legacy_boolean_compose_bar_is_rewritten_as_a_mode_on_save() {
        for (legacy, expected) in [("true", "auto"), ("false", "never")] {
            let parsed: Config = toml::from_str(&format!("[ui]\ncompose_bar = {legacy}\n"))
                .expect("a legacy boolean must still parse");
            assert_eq!(parsed.ui.compose_bar, expected);

            let rendered = render_config_plain(&parsed);
            assert!(
                rendered.contains(&format!("compose_bar = \"{expected}\"")),
                "saved config must carry the string form, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn mobile_bar_preferences_render_and_round_trip() {
        // Both default true, and the defaults render and re-parse.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(parsed.ui.mobile_top_bar);
        assert!(parsed.ui.mobile_accessory_bar);

        // A user-set false survives a regenerate for each field independently.
        // Same shape as `compose_bar_renders_and_round_trips` above: this is
        // the half that catches a missing `patch_table_bool` line, where the
        // re-parse would silently fall back to the default (true).
        let config = Config {
            ui: crate::config::UiConfig {
                mobile_top_bar: false,
                mobile_accessory_bar: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(!parsed.ui.mobile_top_bar);
        assert!(!parsed.ui.mobile_accessory_bar);
    }

    #[test]
    fn first_load_screen_opt_outs_default_off_and_round_trip() {
        // Both screens are on by default, so both DISABLE flags default false.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(!parsed.ui.disable_automated_welcome_screen);
        assert!(!parsed.ui.disable_release_notes);

        // A user-set true survives a regenerate: without the `patch_table_bool`
        // lines the key would be absent and silently fall back to false, which
        // would re-enable a screen the user turned off.
        let config = Config {
            ui: crate::config::UiConfig {
                disable_automated_welcome_screen: true,
                disable_release_notes: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert!(parsed.ui.disable_automated_welcome_screen);
        assert!(parsed.ui.disable_release_notes);
    }

    #[test]
    fn search_index_max_files_defaults_and_round_trips() {
        // The default renders and re-parses to 50 000.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.search_index_max_files,
            crate::config::DEFAULT_SEARCH_INDEX_MAX_FILES
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                search_index_max_files: 1234,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.search_index_max_files, 1234);
    }

    #[test]
    fn search_index_max_files_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value
        // rather than leaving the seeded file untouched.
        fs::write(&path, "[server]\nsearch_index_max_files = 777\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                search_index_max_files: 4321,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.search_index_max_files, 4321,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn tree_list_max_concurrency_defaults_and_round_trips() {
        // The default renders and re-parses to 8.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.tree_list_max_concurrency,
            crate::config::DEFAULT_TREE_LIST_MAX_CONCURRENCY
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                tree_list_max_concurrency: 123,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.tree_list_max_concurrency, 123);
    }

    #[test]
    fn tree_list_max_concurrency_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value
        // rather than leaving the seeded file untouched.
        fs::write(&path, "[server]\ntree_list_max_concurrency = 3\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                tree_list_max_concurrency: 16,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.tree_list_max_concurrency, 16,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn file_drop_settings_default_and_round_trip() {
        // The size default is the one the maintainer chose against real
        // screenshots, and it is deliberately far above the web framework's own
        // 2 MB body limit, which is the whole reason the route sets it.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.file_drop_max_bytes,
            crate::config::DEFAULT_FILE_DROP_MAX_BYTES
        );
        assert_eq!(parsed.server.file_drop_max_bytes, 104_857_600);
        assert_eq!(
            parsed.server.file_drop_max_concurrency,
            crate::config::DEFAULT_FILE_DROP_MAX_CONCURRENCY
        );

        // Both survive a regenerate, including the `0` that switches file drop
        // off: a zero that silently reverted to the default would turn the
        // documented opt-out into a no-op.
        let config = Config {
            server: crate::config::ServerConfig {
                file_drop_max_bytes: 0,
                file_drop_max_concurrency: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.file_drop_max_bytes, 0);
        assert_eq!(parsed.server.file_drop_max_concurrency, 7);
    }

    #[test]
    fn upload_settings_default_and_round_trip() {
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.ui.upload_directory,
            crate::config::DEFAULT_UPLOAD_DIRECTORY
        );
        assert!(parsed.ui.upload_write_gitignore);

        // The opt-out is the value that must survive: a `false` reverting to
        // the default would silently start hiding files from git again for a
        // user who deliberately wants to commit what they drop.
        let mut config = Config::default();
        config.ui.upload_directory = "tmp/drops".to_string();
        config.ui.upload_write_gitignore = false;
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.ui.upload_directory, "tmp/drops");
        assert!(!parsed.ui.upload_write_gitignore);
    }

    #[test]
    fn upload_user_values_survive_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[ui]\nupload_directory = \"old\"\nupload_write_gitignore = true\n",
        )
        .expect("seed config");

        let mut config = Config::default();
        config.ui.upload_directory = "tmp/drops".to_string();
        config.ui.upload_write_gitignore = false;
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(parsed.ui.upload_directory, "tmp/drops", "saved:\n{saved}");
        assert!(!parsed.ui.upload_write_gitignore, "saved:\n{saved}");
    }

    #[test]
    fn file_drop_user_values_survive_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[server]\nfile_drop_max_bytes = 1\nfile_drop_max_concurrency = 1\n",
        )
        .expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                file_drop_max_bytes: 5_000_000,
                file_drop_max_concurrency: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.file_drop_max_bytes, 5_000_000,
            "saved:\n{saved}"
        );
        assert_eq!(
            parsed.server.file_drop_max_concurrency, 3,
            "saved:\n{saved}"
        );
    }

    #[test]
    fn release_notes_max_concurrency_defaults_and_round_trips() {
        // The default renders and re-parses to the documented small bound.
        let rendered = render_config_plain(&Config::default());
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(
            parsed.server.release_notes_max_concurrency,
            crate::config::DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY
        );

        // A user-set value survives a regenerate.
        let config = Config {
            server: crate::config::ServerConfig {
                release_notes_max_concurrency: 9,
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = render_config_plain(&config);
        let parsed: Config = toml::from_str(&rendered).expect("re-parse");
        assert_eq!(parsed.server.release_notes_max_concurrency, 9);
    }

    #[test]
    fn release_notes_max_concurrency_user_value_survives_patch() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Seed a DIFFERENT value than the patch target below, so the assertion
        // can only pass if patch_config_file_with actually wrote the new value.
        fs::write(&path, "[server]\nrelease_notes_max_concurrency = 1\n").expect("seed config");

        let config = Config {
            server: crate::config::ServerConfig {
                release_notes_max_concurrency: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        patch_config_file_with(&path, &config, Durability::NoFsync).expect("patch");
        let saved = fs::read_to_string(&path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("patched file re-parses");
        assert_eq!(
            parsed.server.release_notes_max_concurrency, 5,
            "saved:\n{saved}"
        );
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
    fn patch_round_trips_web_dragdrop_paste_and_omits_it_when_unset() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[defaults]\nprovider = \"claude\"\n").expect("write initial");

        let mut config = Config::default();
        // A provider the user added themselves: no form of its own, so the key
        // must not appear at all rather than be invented as "bare".
        config.providers.commands.insert(
            "myagent".to_string(),
            crate::config::ProviderCommandConfig {
                command: "myagent".to_string(),
                ..Default::default()
            },
        );
        config
            .providers
            .commands
            .get_mut("opencode")
            .expect("opencode provider exists")
            .web_dragdrop_paste = Some("backslash_escaped".to_string());

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");

        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(
            parsed.providers.commands["codex"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::SingleQuoted,
            "the shipped form must survive a save: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["opencode"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::BackslashEscaped,
            "an explicit user value must survive a save: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["myagent"].web_dragdrop_paste, None,
            "a provider with no form must not gain one: {saved}"
        );
        assert_eq!(
            parsed.providers.commands["myagent"].resolved_web_dragdrop_paste(),
            crate::config::WebDragDropPaste::Bare,
            "and an absent key resolves to bare"
        );

        let myagent_section = saved
            .split("[providers.myagent]")
            .nth(1)
            .and_then(|s| s.split("[providers.").next())
            .unwrap_or("");
        assert!(
            !myagent_section.contains("web_dragdrop_paste"),
            "None must omit the key; got: {myagent_section}"
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

    /// A project entry is REBUILT on every save, so anything the user put in it
    /// that dux does not manage has to be captured first or it is deleted. This
    /// was a real loss: an upgrade test caught `custom_note` vanishing from a
    /// hand-edited `[[projects]]` block on the first save after the upgrade.
    #[test]
    fn patch_keeps_a_hand_added_key_inside_a_project_entry() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             id = \"project-1\"\n\
             path = \"/home/user/project\"\n\
             # why this project matters\n\
             custom_note = \"hand added\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/project".to_string(),
            name: Some("renamed".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "the hand-added key was deleted by the save: {saved}"
        );
        // The comment lives on the key's decor, so it travels with it.
        assert!(
            saved.contains("# why this project matters"),
            "the comment attached to the carried key was lost: {saved}"
        );
        // ...and the managed edit still landed.
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.projects[0].name.as_deref(), Some("renamed"));
    }

    /// The carry must not resurrect `leading_branch`. It is a `ProjectConfig`
    /// field the loader reads (to repair SQLite) and the writer deliberately
    /// omits, so "not a managed key" is not enough to decide it belongs to the
    /// user.
    #[test]
    fn patch_still_drops_leading_branch_rather_than_carrying_it_over() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"project-1\"\npath = \"/p\"\nleading_branch = \"trunk\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/p".to_string(),
            name: None,
            default_provider: None,
            leading_branch: Some("trunk".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            !saved.contains("leading_branch"),
            "derived branch state must never be pinned back into config: {saved}"
        );
    }

    /// The carry is keyed by project id, not by position, so removing the first
    /// of two projects must not move the second one's keys onto a stranger.
    #[test]
    fn carried_project_keys_follow_the_id_when_a_project_is_removed() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"a\"\npath = \"/a\"\nnote_a = 1\n\n\
             [[projects]]\nid = \"b\"\npath = \"/b\"\nnote_b = 2\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "b".to_string(),
            path: "/b".to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        });

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(saved.contains("note_b = 2"), "{saved}");
        assert!(
            !saved.contains("note_a"),
            "a removed project's keys must go with it: {saved}"
        );
    }

    /// A bare `ProjectConfig` with only the two required fields, for the carry
    /// tests below.
    fn bare_project(id: &str, path: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.to_string(),
            path: path.to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        }
    }

    /// `projects = [ { ... } ]` is the OTHER legal TOML spelling of the same
    /// array. `toml` loads it identically, so a user who writes it that way has a
    /// working config, and the carry has to see it too. It used to look only for
    /// the array-of-tables spelling, so the key survived one spelling and was
    /// deleted in the other.
    #[test]
    fn patch_keeps_a_hand_added_key_written_in_the_inline_array_spelling() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "projects = [ { id = \"project-1\", path = \"/p\", custom_note = \"hand added\" } ]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "the inline-array spelling lost the hand-added key: {saved}"
        );
    }

    /// Two entries sharing an id is a hand-edit dux itself never writes, and the
    /// project sync rejects it. The writer still must not SCRAMBLE it: each
    /// entry's own extras belong to that entry, matched by occurrence, and a key
    /// name reused across the two must not be collapsed to one value.
    #[test]
    fn duplicate_project_ids_keep_their_own_keys_rather_than_pooling_them() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"dup\"\npath = \"/a\"\nnote = \"from-first\"\n\n\
             [[projects]]\nid = \"dup\"\npath = \"/b\"\nnote = \"from-second\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("dup", "/a"));
        config.projects.push(bare_project("dup", "/b"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let doc: DocumentMut = saved.parse().expect("reparse");
        let entries = doc
            .get("projects")
            .and_then(Item::as_array_of_tables)
            .expect("projects array");
        assert_eq!(entries.len(), 2, "{saved}");
        assert_eq!(
            entries
                .get(0)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str),
            Some("from-first"),
            "the first entry lost its own key: {saved}"
        );
        assert_eq!(
            entries
                .get(1)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str),
            Some("from-second"),
            "the second entry's key was pooled onto the first: {saved}"
        );
    }

    /// A comment block written ABOVE a `[[projects]]` header is user data, and it
    /// used to be deleted on save: `toml_edit` files it on the entry's own decor
    /// rather than on any of its keys, so carrying the keys was not enough.
    #[test]
    fn patch_keeps_the_comment_written_above_a_project_header() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# projects I care about\n\
             [[projects]]\n\
             id = \"project-1\"\n\
             path = \"/p\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let lines: Vec<&str> = saved.lines().collect();
        let header = lines
            .iter()
            .position(|l| l.trim() == "[[projects]]")
            .unwrap_or_else(|| panic!("the projects header vanished: {saved}"));
        assert_eq!(
            lines.get(header.wrapping_sub(1)).map(|l| l.trim()),
            Some("# projects I care about"),
            "the comment above the header was deleted or moved: {saved}"
        );
    }

    /// A comment written after the LAST key of the last entry is NOT the entry's
    /// data in `toml_edit`'s model: it becomes the prefix of whatever item follows
    /// it, or the DOCUMENT's trailing trivia when nothing does. So a save that
    /// appends sections (which every save does, materializing keys the file
    /// predates) leaves it at the very end of the file, below those new sections.
    ///
    /// It is not lost, and it is not the projects rebuild that moves it. Re-homing
    /// it onto the entry would mean guessing that document-trailing trivia belongs
    /// to whichever block happened to be last, which would just as readily steal a
    /// comment the user wrote about the file as a whole. Measured and pinned
    /// rather than "fixed", so the next reader knows which of the two it is.
    #[test]
    fn a_comment_below_the_last_project_key_survives_but_stays_document_trailing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             id = \"project-1\"\n\
             path = \"/p\"\n\
             # a note at the end of this entry\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("# a note at the end of this entry"),
            "the trailing comment was deleted outright: {saved}"
        );
        assert_eq!(
            saved.trim_end().lines().last().map(str::trim),
            Some("# a note at the end of this entry"),
            "trailing trivia is expected to render last, below the appended sections: {saved}"
        );
    }

    /// An entry with no `id` is a legal, loadable hand-edit: `ProjectConfig::id`
    /// carries `#[serde(default = "new_project_id")]`, so the loader mints one and
    /// the file keeps working. The capture used to SKIP such an entry, which threw
    /// away both its unmanaged keys and the comment above its header. `path` is the
    /// only other stable field the rebuild writes, so it is the fallback identity.
    #[test]
    fn patch_keeps_the_keys_and_comment_of_an_entry_that_carries_no_id() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# my only project\n\
             [[projects]]\n\
             path = \"/p\"\n\
             custom_note = \"hand added\"\n",
        )
        .expect("write initial");

        // What the loader does with that file: it mints an id and keeps the path.
        let loaded: Config = toml::from_str(&fs::read_to_string(&config_path).expect("read"))
            .expect("a project entry with no id must still load");
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].path, "/p");
        assert!(!loaded.projects[0].id.is_empty(), "the loader mints an id");

        patch_config_file(&config_path, &loaded).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            saved.contains("custom_note = \"hand added\""),
            "an entry with no id lost its hand-added key: {saved}"
        );
        assert!(
            saved.contains("# my only project"),
            "an entry with no id lost the comment above its header: {saved}"
        );
    }

    /// A comment sitting BETWEEN the `[[projects]]` header and the first key is
    /// filed by `toml_edit` as the prefix of that first key, which is `id`, a
    /// managed key rebuilt from scratch. Carrying only the header decor therefore
    /// deleted it. The fixture puts a comment in five different positions so the
    /// test says which ones survive rather than testing one and hoping.
    #[test]
    fn patch_keeps_a_comment_in_every_position_inside_a_project_entry() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# 1 above the header\n\
             [[projects]]\n\
             # 2 between the header and the first key\n\
             id = \"project-1\"\n\
             path = \"/p\"\n\
             # 3 above an unmanaged key\n\
             custom_note = \"hand added\" # 4 at the end of its line\n\
             # 5 after the last key\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        for comment in [
            "# 1 above the header",
            "# 2 between the header and the first key",
            "# 3 above an unmanaged key",
            "# 4 at the end of its line",
            // Position 5 is document-trailing trivia in `toml_edit`'s model and is
            // pinned separately by
            // `a_comment_below_the_last_project_key_survives_but_stays_document_trailing`.
            "# 5 after the last key",
        ] {
            assert!(saved.contains(comment), "{comment:?} was deleted: {saved}");
        }
        // ...and position 2 is still where it was written, not floated elsewhere.
        let lines: Vec<&str> = saved.lines().collect();
        let header = lines
            .iter()
            .position(|l| l.trim() == "[[projects]]")
            .unwrap_or_else(|| panic!("the projects header vanished: {saved}"));
        assert_eq!(
            lines.get(header + 1).map(|l| l.trim()),
            Some("# 2 between the header and the first key"),
            "the comment under the header moved: {saved}"
        );
        // The file still parses, which a mis-placed decor prefix can break.
        let _: Config = toml::from_str(&saved).expect("reparse");

        // A carried prefix is re-captured on the next save, so it has to settle
        // rather than grow a blank line per save. Measured from the SECOND save
        // on: the first save also materializes sections the file predates (here
        // `[macros]`) and re-appends the rebuilt `projects` array after them, which
        // moves the array within the document exactly once and is unrelated to the
        // carry. Saves two and three are byte-identical.
        patch_config_file(&config_path, &config).expect("second save");
        let second = fs::read_to_string(&config_path).expect("read back");
        patch_config_file(&config_path, &config).expect("third save");
        assert_eq!(
            fs::read_to_string(&config_path).expect("read back"),
            second,
            "the carried comments must settle rather than drift on every save"
        );
        for comment in [
            "# 1 above the header",
            "# 2 between the header and the first key",
            "# 3 above an unmanaged key",
            "# 4 at the end of its line",
            "# 5 after the last key",
        ] {
            assert!(
                second.contains(comment),
                "{comment:?} survived one save and was lost by the next: {second}"
            );
        }
    }

    /// The comment above the entry's FIRST key is re-homed onto the rebuilt `id`,
    /// but only when the source's first key was one this writer rebuilds. When the
    /// user put an UNMANAGED key first, that same comment also travels on the key's
    /// own decor, and carrying it twice duplicates it further down the file.
    #[test]
    fn a_comment_above_an_unmanaged_first_key_is_carried_once_not_twice() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\n\
             # about the note\n\
             custom_note = \"hand added\"\n\
             id = \"project-1\"\n\
             path = \"/p\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("project-1", "/p"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert_eq!(
            saved.matches("# about the note").count(),
            1,
            "the comment was carried twice: {saved}"
        );
    }

    /// Two entries sharing an id is a hand-edit the project sync rejects, but the
    /// writer must not SCRAMBLE it. Matching by occurrence alone kept each entry's
    /// keys with whatever landed in the same SLOT, so merely reordering the two in
    /// memory moved one project's key onto the other, a different worktree.
    /// Identity is therefore the pair of id and path.
    #[test]
    fn duplicate_project_ids_keep_their_own_keys_when_the_entries_are_reordered() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[[projects]]\nid = \"dup\"\npath = \"/a\"\nnote = \"from-a\"\n\n\
             [[projects]]\nid = \"dup\"\npath = \"/b\"\nnote = \"from-b\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("dup", "/b"));
        config.projects.push(bare_project("dup", "/a"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let doc: DocumentMut = saved.parse().expect("reparse");
        let entries = doc
            .get("projects")
            .and_then(Item::as_array_of_tables)
            .expect("projects array");
        let note_at = |index: usize| {
            entries
                .get(index)
                .and_then(|t| t.get("note"))
                .and_then(Item::as_str)
                .map(str::to_string)
        };
        assert_eq!(
            entries
                .get(0)
                .and_then(|t| t.get("path"))
                .and_then(Item::as_str),
            Some("/b"),
            "{saved}"
        );
        assert_eq!(
            note_at(0).as_deref(),
            Some("from-b"),
            "/b took /a's key: {saved}"
        );
        assert_eq!(
            note_at(1).as_deref(),
            Some("from-a"),
            "/a took /b's key: {saved}"
        );
    }

    /// `projects = [ ... ]` spelled across several lines can carry a comment
    /// BETWEEN its elements, which is legal TOML and pure user data. `toml_edit`
    /// files it as the prefix of the element that follows, so it is carryable
    /// exactly like an array-of-tables header comment, and it used to be dropped.
    #[test]
    fn patch_keeps_comments_written_between_inline_array_entries() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "projects = [\n  \
             # about a\n  \
             { id = \"a\", path = \"/a\", note_a = 1 },\n  \
             # about b\n  \
             { id = \"b\", path = \"/b\" },\n  \
             # about c\n  \
             { id = \"c\", path = \"/c\" },\n]\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("a", "/a"));
        config.projects.push(bare_project("b", "/b"));
        config.projects.push(bare_project("c", "/c"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        for comment in ["# about a", "# about b", "# about c"] {
            assert!(
                saved.contains(comment),
                "the inline-array spelling dropped {comment:?}: {saved}"
            );
        }
        // ...and the key carried out of an inline table is re-spaced for the block
        // table it lands in, rather than bleeding its inline spacing. Asserted as a
        // whole LINE, because `contains("note_a = 1")` also matches the bleeding
        // form `" note_a = 1 "` and so would pass without the fix.
        assert!(
            saved.lines().any(|line| line == "note_a = 1"),
            "the carried key kept its inline spacing: {saved}"
        );
    }

    /// The carried prefix is normalized to the comment lines rather than pasted
    /// verbatim. The recorded prefix of the FILE'S FIRST entry has no leading blank
    /// line (nothing precedes it), so pasting it onto that project once it has moved
    /// to second position ran the two entries together.
    #[test]
    fn a_commented_project_moved_later_keeps_a_blank_line_before_its_header() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "# about a\n[[projects]]\nid = \"a\"\npath = \"/a\"\n\n\
             [[projects]]\nid = \"b\"\npath = \"/b\"\n",
        )
        .expect("write initial");

        let mut config = Config::default();
        config.projects.push(bare_project("b", "/b"));
        config.projects.push(bare_project("a", "/a"));

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let lines: Vec<&str> = saved.lines().collect();
        let comment = lines
            .iter()
            .position(|l| l.trim() == "# about a")
            .unwrap_or_else(|| panic!("the comment was deleted: {saved}"));
        assert!(
            lines
                .get(comment.wrapping_sub(1))
                .is_some_and(|l| l.trim().is_empty()),
            "the moved entry ran into the one above it: {saved}"
        );
    }

    #[test]
    fn patch_materializes_tab_cap_keys_on_a_file_that_predates_them() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Simulate a config written before the tab-cap keys existed: the full
        // canonical render with those three keys stripped back out.
        let mut doc: DocumentMut = render_config_plain(&Config::default())
            .parse()
            .expect("parse render");
        doc["ui"]
            .as_table_mut()
            .expect("[ui] table")
            .remove("agent_tabs_max");
        let server = doc["server"].as_table_mut().expect("[server] table");
        server.remove("max_websocket_tab_connections");
        server.remove("max_websocket_tabs_per_agent");
        fs::write(&config_path, doc.to_string()).expect("write older config");
        assert!(
            !fs::read_to_string(&config_path)
                .unwrap()
                .contains("agent_tabs_max")
        );

        let mut config = Config::default();
        config.ui.agent_tabs_max = 7;
        config.server.max_websocket_tab_connections = 123;
        config.server.max_websocket_tabs_per_agent = 5;

        patch_config_file(&config_path, &config).expect("patch");

        let saved = fs::read_to_string(&config_path).expect("read back");
        // The patch path must materialize the new keys (parity with the sibling
        // ws caps), not rely on defaults being filled in at load time.
        assert!(saved.contains("agent_tabs_max"));
        assert!(saved.contains("max_websocket_tab_connections"));
        assert!(saved.contains("max_websocket_tabs_per_agent"));
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.ui.agent_tabs_max, 7);
        assert_eq!(parsed.server.max_websocket_tab_connections, 123);
        assert_eq!(parsed.server.max_websocket_tabs_per_agent, 5);
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
        config.server.host = "0.0.0.0".to_string();
        config.server.port = 9000;
        config.server.tailscale = "no".to_string();
        config.server.allowed_hosts = vec!["box.tailnet.ts.net".to_string()];
        config.server.color = "never".to_string();
        config.server.access_log = false;
        config.server.max_websocket_events_connections = 42;
        config.server.max_websocket_agent_connections = 43;
        config.server.max_websocket_terminal_connections = 44;
        config.server.title = "dux #1".to_string();
        config.server.favicon = "violet".to_string();

        write_config_plain(&config_path, &config).expect("write_config_plain");

        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.host, "0.0.0.0");
        assert_eq!(parsed.server.port, 9000);
        assert_eq!(parsed.server.tailscale, "no");
        assert_eq!(
            parsed.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
        assert_eq!(parsed.server.color, "never");
        assert!(!parsed.server.access_log);
        assert_eq!(parsed.server.max_websocket_events_connections, 42);
        assert_eq!(parsed.server.max_websocket_agent_connections, 43);
        assert_eq!(parsed.server.max_websocket_terminal_connections, 44);
        assert_eq!(parsed.server.title, "dux #1");
        assert_eq!(parsed.server.favicon, "violet");
        // The deprecated `bind` key is never re-emitted by the patcher.
        assert!(
            !saved.contains("bind ="),
            "patcher must not emit bind: {saved}"
        );
    }

    #[test]
    fn a_save_migrates_the_legacy_tailscale_enabled_key_out_of_the_file() {
        // A user upgrading from the boolean has `tailscale_enabled` on disk. The
        // save writes the tri-state key and takes the boolean OUT, so the file
        // stops carrying a key dux no longer reads. This is the on-disk half of
        // the compat story; the in-memory half is `config_migrate`.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[server]\nport = 9000\ntailscale_enabled = false\n",
        )
        .expect("seed an old config");

        let mut config = Config::default();
        config.server.port = 9000;
        config.server.tailscale = "no".to_string();
        patch_config_file(&config_path, &config).expect("patch the existing config");

        let saved = fs::read_to_string(&config_path).expect("read back");
        assert!(
            !saved.contains("tailscale_enabled"),
            "the legacy key must be gone after a save: {saved}"
        );
        assert!(
            saved.contains("tailscale = \"no\""),
            "the tri-state key must be written: {saved}"
        );
    }

    #[test]
    fn write_config_plain_round_trips_title_with_toml_specials() {
        // The instance title is free-form user text. Lock in the escaping contract
        // so a future toml_edit bump or patcher refactor can't silently emit a
        // value with an unescaped quote/backslash that fails to re-parse.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.server.title = r#"dux "prod" \ lab"#.to_string();

        write_config_plain(&config_path, &config).expect("write_config_plain");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.title, r#"dux "prod" \ lab"#);
    }

    #[test]
    fn patch_config_file_round_trips_title_with_toml_specials() {
        // The in-place patcher is the production save hot-path. Exercise its
        // read-parse-apply-write cycle from an existing [server] block with a
        // title containing a quote and a backslash, locking in the same escaping
        // contract the plain writer is held to.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[server]\ntitle = \"old\"\n").expect("write initial");

        let mut config = Config::default();
        config.server.title = r#"dux "prod" \ lab"#.to_string();

        patch_config_file(&config_path, &config).expect("patch");
        let saved = fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&saved).expect("reparse");
        assert_eq!(parsed.server.title, r#"dux "prod" \ lab"#);
    }

    #[test]
    fn write_config_plain_round_trips_host_and_allowed_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.toml");
        let mut cfg = Config::default();
        cfg.server.host = "0.0.0.0".into();
        cfg.server.port = 9000;
        cfg.server.allowed_hosts = vec!["box.tailnet.ts.net".into()];
        write_config_plain(&path, &cfg).unwrap();
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.server.host, "0.0.0.0");
        assert_eq!(parsed.server.port, 9000);
        assert_eq!(
            parsed.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
    }

    #[test]
    #[cfg(unix)]
    fn write_config_plain_sets_owner_only_perms() {
        // config.toml may carry secrets (tokens under [env]),
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

    // -----------------------------------------------------------------------
    // merge_unmanaged_keys
    // -----------------------------------------------------------------------

    #[test]
    fn merge_preserves_an_unknown_key_and_drops_an_orphaned_section() {
        let original: DocumentMut = "\
[server]
port = 8080
listen_addrs = []

[server.acme]
enabled = false
production = true

[auth]
users = []

[my_fork_section]
knob = 42
"
        .parse()
        .expect("parse original");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse rendered");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        // Unknown keys survive verbatim, whether beside a managed key or in a
        // section dux has never heard of.
        assert!(out.contains("listen_addrs = []"), "out:\n{out}");
        assert!(out.contains("knob = 42"), "out:\n{out}");
        // Orphaned sections are gone.
        assert!(!out.contains("acme"), "out:\n{out}");
        assert!(!out.contains("users"), "out:\n{out}");
        // And their removal is reported, not silent.
        assert_eq!(report.dropped, vec!["auth", "server.acme"]);
        assert_eq!(
            report.preserved,
            vec!["my_fork_section.knob", "server.listen_addrs"]
        );
        // The merged document is still valid TOML.
        let _: toml_edit::DocumentMut = out.parse().expect("merged output re-parses");
    }

    #[test]
    fn merge_reports_nothing_when_the_render_already_covers_everything() {
        let original: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");
        let mut rendered: DocumentMut = "# doc\n[server]\nport = 8080\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        assert!(report.is_empty(), "unexpected report: {report:?}");
        assert!(rendered.to_string().contains("# doc"));
    }

    #[test]
    fn merge_preserves_an_unknown_key_inside_an_array_of_tables() {
        // A hand-added key inside a `[[projects]]` block must survive, because
        // the canonical renderer only emits the project fields dux knows.
        let original: DocumentMut = "\
[[projects]]
id = \"a\"
custom_note = \"do not lose me\"

[[projects]]
id = \"b\"
"
        .parse()
        .expect("parse original");
        let mut rendered: DocumentMut = "[[projects]]\nid = \"a\"\n\n[[projects]]\nid = \"b\"\n"
            .parse()
            .expect("parse rendered");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(
            out.contains("custom_note = \"do not lose me\""),
            "out:\n{out}"
        );
        assert_eq!(report.preserved, vec!["projects[0].custom_note"]);
        assert!(report.dropped.is_empty());
    }

    /// A key `insert_at_path` cannot place must be REPORTED, not silently
    /// vanished.
    ///
    /// **This is currently unreachable through any real config.** The canonical
    /// renderer emits one `[[projects]]` block per parsed project, so the
    /// rendered document always has an entry at every index the original has,
    /// and `insert_at_path` never returns false. The input below is doctored
    /// (a rendered document with FEWER array entries than the original) to
    /// exercise the branch directly. The guard exists so that if the renderer
    /// ever stops emitting one block per project, the failure is loud.
    #[test]
    fn merge_reports_a_key_it_could_not_place_instead_of_dropping_it_silently() {
        let original: DocumentMut = "\
[[projects]]
id = \"a\"
note = \"kept\"

[[projects]]
id = \"b\"
second_note = \"nowhere to go\"
"
        .parse()
        .expect("parse original");
        // Doctored: only ONE rendered project, so `projects[1]` has no home.
        let mut rendered: DocumentMut = "[[projects]]\nid = \"a\"\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(out.contains("note = \"kept\""), "out:\n{out}");
        assert!(!out.contains("second_note"), "out:\n{out}");
        assert_eq!(report.preserved, vec!["projects[0].note"]);
        assert_eq!(
            report.unplaceable,
            vec!["projects[1].id", "projects[1].second_note"],
            "a key that could not be placed must be named, not vanish"
        );
        assert!(
            !report.is_empty(),
            "a report naming a lost key is not empty"
        );
    }

    #[test]
    fn merge_does_not_confuse_a_nested_auth_key_with_the_orphaned_auth_section() {
        // The drop-list matches TABLE paths. A key called `auth` nested inside a
        // live section is a user key and must be preserved, not dropped.
        let original: DocumentMut = "[server]\nauth = \"token\"\n".parse().expect("parse");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");

        let report = merge_unmanaged_keys(&mut rendered, &original);

        assert!(rendered.to_string().contains("auth = \"token\""));
        assert_eq!(report.preserved, vec!["server.auth"]);
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn merge_preserves_a_comment_attached_to_an_unknown_key() {
        let original: DocumentMut = "[server]\n# why this knob exists\nfork_knob = 3\n"
            .parse()
            .expect("parse");
        let mut rendered: DocumentMut = "[server]\nport = 8080\n".parse().expect("parse");

        merge_unmanaged_keys(&mut rendered, &original);

        let out = rendered.to_string();
        assert!(out.contains("# why this knob exists"), "out:\n{out}");
        assert!(out.contains("fork_knob = 3"), "out:\n{out}");
    }

    #[test]
    fn apply_patches_strips_removed_max_websocket_connections_key() {
        // Build a DocumentMut that still carries the obsolete key.
        let raw = "[server]\nmax_websocket_connections = 16\nport = 7878\n";
        assert!(
            crate::config::raw_has_removed_max_websocket_connections(raw),
            "precondition: raw must contain the removed key"
        );
        let mut doc: DocumentMut = raw.parse().expect("parse toml");
        let config = Config::default();
        apply_patches(&mut doc, &config);
        // The key must be stripped after apply_patches.
        let stripped = doc.to_string();
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(&stripped),
            "apply_patches must remove max_websocket_connections; got: {stripped}"
        );
        // Other server settings survive.
        assert!(
            stripped.contains("port"),
            "apply_patches must not wipe unrelated server keys; got: {stripped}"
        );
    }

    #[test]
    fn apply_patches_does_not_warn_when_key_is_absent() {
        // A config that never had max_websocket_connections must not trip the
        // detection predicate after patching.
        let raw = "[server]\nport = 7878\n";
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(raw),
            "precondition: raw must not contain the removed key"
        );
        let mut doc: DocumentMut = raw.parse().expect("parse toml");
        let config = Config::default();
        // Must not panic and must leave the key absent.
        apply_patches(&mut doc, &config);
        let stripped = doc.to_string();
        assert!(
            !crate::config::raw_has_removed_max_websocket_connections(&stripped),
            "key must remain absent; got: {stripped}"
        );
    }
}
