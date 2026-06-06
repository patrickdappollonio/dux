use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Value};

use crate::keybindings;

pub use dux_core::config::*;

pub fn ensure_config(paths: &DuxPaths) -> Result<Config> {
    paths.ensure_dirs()?;
    if !paths.config_path.exists() {
        fs::write(&paths.config_path, render_default_config())
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
    }

    let raw = fs::read_to_string(&paths.config_path)
        .with_context(|| format!("failed to read {}", paths.config_path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    if apply_config_deprecations(&mut doc)? {
        fs::write(&paths.config_path, doc.to_string())
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
    }

    let mut config: Config = toml::from_str(&doc.to_string())
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    config.providers.ensure_defaults();
    validate_project_envs(&config)?;
    Ok(config)
}

fn validate_project_envs(config: &Config) -> Result<()> {
    for project in &config.projects {
        resolve_agent_env(&config.env, &project.env).with_context(|| {
            format!(
                "invalid env for project {}",
                project.name.as_deref().unwrap_or(&project.path)
            )
        })?;
    }
    resolve_project_env(&config.env).context("invalid global env")?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct DeprecatedConfigKey {
    section: &'static str,
    key: &'static str,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum DeprecatedConfigKeyAction {
    Replace {
        migrate: fn(&mut DocumentMut, DeprecatedConfigKey, Item) -> Result<()>,
    },
    Remove,
    Fail {
        message: &'static str,
    },
}

#[derive(Clone, Copy)]
struct DeprecatedConfigKeyRule {
    old: DeprecatedConfigKey,
    action: DeprecatedConfigKeyAction,
}

const DEPRECATED_CONFIG_KEYS: &[DeprecatedConfigKeyRule] = &[DeprecatedConfigKeyRule {
    old: DeprecatedConfigKey {
        section: "defaults",
        key: "prompt_for_name",
    },
    action: DeprecatedConfigKeyAction::Replace {
        migrate: migrate_prompt_for_name,
    },
}];

fn apply_config_deprecations(doc: &mut DocumentMut) -> Result<bool> {
    apply_config_deprecations_with(doc, DEPRECATED_CONFIG_KEYS)
}

fn apply_config_deprecations_with(
    doc: &mut DocumentMut,
    rules: &[DeprecatedConfigKeyRule],
) -> Result<bool> {
    let mut changed = false;
    for rule in rules {
        let Some(old_item) =
            dux_core::config_write::remove_table_key_item(doc, rule.old.section, rule.old.key)
        else {
            continue;
        };
        match rule.action {
            DeprecatedConfigKeyAction::Replace { migrate } => {
                migrate(doc, rule.old, old_item)?;
            }
            DeprecatedConfigKeyAction::Remove => {}
            DeprecatedConfigKeyAction::Fail { message } => {
                bail!(
                    "unsupported config key [{}.{}]: {}",
                    rule.old.section,
                    rule.old.key,
                    message
                );
            }
        }
        changed = true;
    }
    Ok(changed)
}

fn migrate_prompt_for_name(
    doc: &mut DocumentMut,
    old: DeprecatedConfigKey,
    old_item: Item,
) -> Result<()> {
    let Some(prompt_for_name) = old_item.as_value().and_then(Value::as_bool) else {
        bail!(
            "unsupported config key [{}.{}]: expected a boolean value",
            old.section,
            old.key
        );
    };

    let table = dux_core::config_write::ensure_table(doc, "defaults");
    if !table.contains_key("enable_randomized_pet_name_by_default") {
        table["enable_randomized_pet_name_by_default"] = toml_edit::value(!prompt_for_name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Config schema: defines the layout, comments, and value accessors for the
// TOML config file. Adding a new setting means adding a struct field, its
// Default value, and one entry here — comments live in exactly one place.
// ---------------------------------------------------------------------------

/// A value extracted from [`Config`] for rendering into TOML.
enum FieldValue {
    Str(String),
    OptStr(Option<String>),
    U16(u16),
    Usize(usize),
    Bool(bool),
    MultilineStr(Option<String>),
}

/// A comment source: static string or dynamically built.
enum CommentSource {
    Static(&'static str),
    Dynamic(String),
}

/// One entry in the config file layout.
enum ConfigEntry {
    /// A comment line (must include the leading `#`).
    Comment(&'static str),
    /// A blank line for spacing.
    Blank,
    /// A TOML section header, e.g. `[defaults]`.
    Section(&'static str),
    /// A key = value line with an optional comment above it.
    Field {
        key: &'static str,
        comment: Option<CommentSource>,
        value_fn: fn(&Config) -> FieldValue,
    },
    /// Renders all `[providers.*]` sub-tables dynamically.
    Providers,
    /// Renders `[[projects]]` declarations.
    Projects,
    /// Renders the top-level `[env]` table.
    Env,
    /// Renders the `[terminal]` section.
    Terminal,
    /// Renders the `[startup_command_terminal]` section.
    StartupCommandTerminal,
    /// Renders the `[server.acme]` built-in TLS / Let's Encrypt subsection.
    ServerAcme,
    /// Renders the `[auth]` section with the web UI login users.
    Auth,
    /// Renders the `[keys]` section with all keybindings.
    Keys,
    /// Renders the `[macros]` section with text macros.
    Macros,
}

fn config_schema(generate_commit_key: &str) -> Vec<ConfigEntry> {
    vec![
        ConfigEntry::Comment("# dux configuration"),
        ConfigEntry::Comment(
            "# Every value is materialized here so the file doubles as documentation.",
        ),
        ConfigEntry::Blank,
        ConfigEntry::Section("defaults"),
        ConfigEntry::Field {
            key: "provider",
            comment: Some(CommentSource::Static(
                "# Global fallback provider for new sessions.\n\
                 # Project-specific provider overrides are managed inside dux, not in this file.",
            )),
            value_fn: |c| FieldValue::Str(c.defaults.provider.clone()),
        },
        ConfigEntry::Field {
            key: "start_directory",
            comment: Some(CommentSource::Static(
                "# Starting directory for the project browser.",
            )),
            value_fn: |c| FieldValue::OptStr(c.defaults.start_directory.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Field {
            key: "commit_prompt",
            comment: Some(CommentSource::Dynamic(format!(
                "# Prompt sent to the AI provider when generating commit messages ({generate_commit_key}).\n\
                 # The staged diff is appended automatically after the prompt text.\n\
                 # This is app-wide; projects do not carry their own commit prompts.",
            ))),
            value_fn: |c| FieldValue::MultilineStr(c.defaults.commit_prompt.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Field {
            key: "enable_randomized_pet_name_by_default",
            comment: Some(CommentSource::Static(
                "# When true, the new-agent name prompt starts with a random two-word pet name.\n\
                 # You can still clear it and type a custom name before creating the agent.\n\
                 # When false, the prompt starts empty and the pet-name checkbox is off.",
            )),
            value_fn: |c| FieldValue::Bool(c.defaults.enable_randomized_pet_name_by_default),
        },
        ConfigEntry::Field {
            key: "pull_before_creating_agent_by_default",
            comment: Some(CommentSource::Static(
                "# When true, dux safely fast-forward pulls the project source checkout\n\
                 # before creating a fresh project agent worktree.\n\
                 # This uses `git pull --ff-only`; it will not create merge commits or rebase.\n\
                 # Set to false to keep fresh agent creation from contacting the remote.",
            )),
            value_fn: |c| FieldValue::Bool(c.defaults.pull_before_creating_agent_by_default),
        },
        ConfigEntry::Blank,
        ConfigEntry::Env,
        ConfigEntry::Blank,
        ConfigEntry::Projects,
        ConfigEntry::Blank,
        ConfigEntry::Providers,
        ConfigEntry::Terminal,
        ConfigEntry::StartupCommandTerminal,
        ConfigEntry::Section("logging"),
        ConfigEntry::Field {
            key: "level",
            comment: Some(CommentSource::Static(
                "# Log level can be error, info, or debug.",
            )),
            value_fn: |c| FieldValue::Str(c.logging.level.clone()),
        },
        ConfigEntry::Field {
            key: "path",
            comment: Some(CommentSource::Static(
                "# Relative paths are resolved from the dux config directory.",
            )),
            value_fn: |c| FieldValue::Str(c.logging.path.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("ui"),
        ConfigEntry::Field {
            key: "left_width_pct",
            comment: Some(CommentSource::Static(
                "# Initial pane sizing percentages. They can still be resized at runtime.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.left_width_pct),
        },
        ConfigEntry::Field {
            key: "right_width_pct",
            comment: Some(CommentSource::Static(
                "# Percentage of the terminal width for the right (files/diff) pane (5-80).",
            )),
            value_fn: |c| FieldValue::U16(c.ui.right_width_pct),
        },
        ConfigEntry::Field {
            key: "terminal_pane_height_pct",
            comment: Some(CommentSource::Static(
                "# Maximum height percentage of the left pane used by the companion terminals list.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.terminal_pane_height_pct),
        },
        ConfigEntry::Field {
            key: "empty_project_separator_min_projects",
            comment: Some(CommentSource::Static(
                "# Separate projects with no agents below a \"Projects with no agents\" divider once\n\
                 # the total project count reaches this number. Set to 0 to disable.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.empty_project_separator_min_projects),
        },
        ConfigEntry::Field {
            key: "staged_pane_height_pct",
            comment: Some(CommentSource::Static(
                "# Height percentage of the right pane used by the staged changes and commit sections.\n# The remaining space goes to the unstaged changes list.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.staged_pane_height_pct),
        },
        ConfigEntry::Field {
            key: "commit_pane_height_pct",
            comment: Some(CommentSource::Static(
                "# Height percentage of the staged section used by the commit message input.\n# The remaining space goes to the staged changes list.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.commit_pane_height_pct),
        },
        ConfigEntry::Field {
            key: "agent_scrollback_lines",
            comment: Some(CommentSource::Static(
                "# Maximum number of lines retained in the embedded agent terminal scrollback.",
            )),
            value_fn: |c| FieldValue::Usize(c.ui.agent_scrollback_lines),
        },
        ConfigEntry::Field {
            key: "branch_sync_interval",
            comment: Some(CommentSource::Static(
                "# Interval in seconds for syncing git branch names in the background.\n# Keeps dux in sync if a branch is renamed outside the app.\n# Set to 0 to disable.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.branch_sync_interval),
        },
        ConfigEntry::Field {
            key: "show_diff_line_numbers",
            comment: Some(CommentSource::Static(
                "# Show old/new line numbers in the diff gutter.\n# Toggle at runtime from the command palette.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.show_diff_line_numbers),
        },
        ConfigEntry::Field {
            key: "diff_tab_width",
            comment: Some(CommentSource::Static(
                "# Number of spaces used to render tab characters in diffs.\n# Set to 0 to leave tabs as-is (they may render as zero-width).",
            )),
            value_fn: |c| FieldValue::U16(c.ui.diff_tab_width),
        },
        ConfigEntry::Field {
            key: "github_integration",
            comment: Some(CommentSource::Static(
                "# Enable GitHub PR tracking for agent sessions.\n# Requires the `gh` CLI installed and authenticated (`gh auth login`).\n# When enabled, a PR pill is shown in the agent pane for branches with\n# an open, merged, or closed pull request. Toggle at runtime from the\n# command palette.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.github_integration),
        },
        ConfigEntry::Field {
            key: "auto_reopen_agents",
            comment: Some(CommentSource::Static(
                "# Reopen agent PTYs that were still running when dux last exited.\n# Disabled by default. Toggle project-level and agent-level opt-outs from the command palette.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.auto_reopen_agents),
        },
        ConfigEntry::Field {
            key: "pr_banner_position",
            comment: Some(CommentSource::Static(
                "# Position of the PR banner in the agent pane: \"top\" or \"bottom\".\n# Toggle at runtime from the command palette.",
            )),
            value_fn: |c| FieldValue::Str(c.ui.pr_banner_position.clone()),
        },
        ConfigEntry::Field {
            key: "theme",
            comment: Some(CommentSource::Static(
                "# Visual theme for the dux interface.\n# Built-in options include \"dux_dark\" (the default), plus any theme\n# bundled with the opaline engine, for example: \"catppuccin_mocha\",\n# \"catppuccin_frappe\", \"nord\", \"dracula\", \"gruvbox_dark\",\n# \"tokyo_night\", \"solarized_dark\", \"one_dark\", \"rose_pine\", and others.\n# To use a custom theme, drop a TOML file into <config_dir>/themes/<name>.toml\n# (with the same token format as opaline themes) and reference it here\n# by file stem. Unknown names fall back to dux_dark with a warning.\n# Use the `change-theme` command in the palette (Ctrl-p) for an interactive picker.",
            )),
            value_fn: |c| FieldValue::Str(c.ui.theme.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("editor"),
        ConfigEntry::Field {
            key: "default",
            comment: Some(CommentSource::Static(
                "# Preferred editor when opening a selected agent worktree.\n# Supported values are matched against popular editor CLIs on PATH\n# (for example: cursor, vscode/code, zed, antigravity).",
            )),
            value_fn: |c| FieldValue::Str(c.editor.default.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("server"),
        ConfigEntry::Field {
            key: "bind",
            comment: Some(CommentSource::Static(
                "# Address and port the `dux server` web UI listens on.\n\
                 # Must be in IP:port form. The default 127.0.0.1:8080 is loopback,\n\
                 # meaning the web UI is reachable only from this machine.\n\
                 # Use 0.0.0.0:8080 to listen on every interface (see the warning below).",
            )),
            value_fn: |c| FieldValue::Str(c.server.bind.clone()),
        },
        ConfigEntry::Field {
            key: "insecure_allow_remote",
            comment: Some(CommentSource::Static(
                "# Allow binding to a non-loopback address even though the web UI has\n\
                 # NO authentication yet: anyone who can reach the port can fully control\n\
                 # your agents and worktrees. Keep this false unless you understand the\n\
                 # risk (for example, brief LAN testing on a trusted network).\n\
                 # When false, `dux server` refuses to start on a non-loopback bind.",
            )),
            value_fn: |c| FieldValue::Bool(c.server.insecure_allow_remote),
        },
        ConfigEntry::Blank,
        ConfigEntry::ServerAcme,
        ConfigEntry::Auth,
        ConfigEntry::Keys,
        ConfigEntry::Blank,
        ConfigEntry::Macros,
    ]
}

fn render_config(config: &Config, bindings: &crate::keybindings::RuntimeBindings) -> String {
    let generate_commit_key = bindings.label_for(crate::keybindings::Action::GenerateCommitMessage);
    let mut out = String::new();
    for entry in config_schema(&generate_commit_key) {
        match entry {
            ConfigEntry::Comment(text) => {
                out.push_str(text);
                out.push('\n');
            }
            ConfigEntry::Blank => out.push('\n'),
            ConfigEntry::Section(name) => {
                let _ = writeln!(out, "[{name}]");
            }
            ConfigEntry::Field {
                key,
                comment,
                value_fn,
            } => {
                if let Some(c) = comment {
                    match c {
                        CommentSource::Static(s) => out.push_str(s),
                        CommentSource::Dynamic(s) => out.push_str(&s),
                    }
                    out.push('\n');
                }
                match value_fn(config) {
                    FieldValue::Str(s) => {
                        let _ = writeln!(out, "{key} = \"{}\"", escape_toml_string(&s));
                    }
                    FieldValue::OptStr(Some(s)) => {
                        let _ = writeln!(out, "{key} = \"{}\"", escape_toml_string(&s));
                    }
                    FieldValue::OptStr(None) => {
                        let _ = writeln!(out, "{key} = \"\"");
                    }
                    FieldValue::U16(n) => {
                        let _ = writeln!(out, "{key} = {n}");
                    }
                    FieldValue::Usize(n) => {
                        let _ = writeln!(out, "{key} = {n}");
                    }
                    FieldValue::Bool(b) => {
                        let _ = writeln!(out, "{key} = {b}");
                    }
                    FieldValue::MultilineStr(Some(s)) => {
                        let escaped = dux_core::config_write::escape_toml_multiline(&s);
                        let _ = writeln!(out, "{key} = \"\"\"\n{escaped}\"\"\"");
                    }
                    FieldValue::MultilineStr(None) => {
                        let _ = writeln!(out, "{key} = \"\"");
                    }
                }
            }
            ConfigEntry::Providers => render_provider_configs(&mut out, &config.providers),
            ConfigEntry::Env => render_env_config(&mut out, &config.env),
            ConfigEntry::Projects => render_project_configs(&mut out, &config.projects),
            ConfigEntry::Terminal => render_terminal_config(&mut out, &config.terminal),
            ConfigEntry::StartupCommandTerminal => {
                render_startup_command_terminal_config(&mut out, &config.startup_command_terminal);
            }
            ConfigEntry::ServerAcme => render_server_acme_config(&mut out, &config.server.acme),
            ConfigEntry::Auth => render_auth_config(&mut out, &config.auth, bindings),
            ConfigEntry::Keys => render_keys_config(&mut out, &config.keys, bindings),
            ConfigEntry::Macros => render_macros_config(&mut out, &config.macros, bindings),
        }
    }
    out
}

pub fn render_default_config() -> String {
    let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default());
    render_config(&Config::default(), &bindings)
}

/// Render a config through the canonical renderer (public for CLI diff).
pub fn render_config_with(
    config: &Config,
    bindings: &crate::keybindings::RuntimeBindings,
) -> String {
    render_config(config, bindings)
}

/// Persist the in-memory `Config` to disk using surgical edits via `toml_edit`.
///
/// If the config file already exists, it is parsed as a TOML document and only
/// the keys that differ from the on-disk version are updated.  User comments,
/// formatting, and unknown keys are preserved.  If the file does not yet exist,
/// a fresh canonical config is rendered instead.
pub fn save_config(
    config_path: &Path,
    config: &Config,
    _bindings: &crate::keybindings::RuntimeBindings,
) -> Result<()> {
    if config_path.exists() {
        // Shared with the web: surgical toml_edit patch preserving user edits.
        dux_core::config_write::patch_config_file(config_path, config)?;
        Ok(())
    } else {
        // First creation: the fully-commented canonical template (TUI-only,
        // needs bindings for two dynamic comment strings). Render from the
        // config's own keys so the documented [keys] section matches what is
        // written, exactly as before.
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        let body = render_config(config, &bindings);
        // 0600 perms: this file holds [auth] bcrypt hashes and [env] tokens, so
        // it must not be group/world readable (shared with the config writer's
        // patch path so first-creation and later saves agree).
        dux_core::config_write::write_config_secure(config_path, &body)?;
        Ok(())
    }
}

fn render_keys_config(
    out: &mut String,
    keys: &KeysConfig,
    bindings: &crate::keybindings::RuntimeBindings,
) {
    out.push_str("[keys]\n");
    out.push_str("# Keybindings configuration. Each action maps to one or more key combos.\n");
    out.push_str(
        "# Key format: single chars (\"j\"), special names (\"up\", \"enter\", \"space\",\n",
    );
    out.push_str(
        "# \"tab\", \"shift-tab\", \"pageup\", \"esc\"), or modifier combos (\"Ctrl-d\").\n",
    );
    out.push_str("#\n");
    out.push_str("# Some keys shown in hints are terminal conventions (e.g. Ctrl-j for newline)\n");
    out.push_str("# that dux documents but does not control. Set this to false to hide them.\n");
    let _ = writeln!(out, "show_terminal_keys = {}", keys.show_terminal_keys);
    out.push('\n');

    let mut last_section: Option<&str> = None;
    for def in keybindings::BINDING_DEFS {
        if def.default_keys.is_empty() {
            continue;
        }
        let config_name = def.action.config_name();

        // Section header based on help section.
        let section = def.action.help_section().unwrap_or("Other");
        if last_section != Some(section) {
            if last_section.is_some() {
                out.push('\n');
            }
            let _ = writeln!(out, "# -- {section} --");
            last_section = Some(section);
        }

        // Description comment — dynamic override for actions that reference other keys.
        let desc = if def.action == keybindings::Action::ToggleResizeMode {
            format!(
                "Enter resize mode ({} to resize side panes).",
                bindings.combined_label(
                    keybindings::Action::ResizeGrow,
                    keybindings::Action::ResizeShrink,
                ),
            )
        } else {
            def.action.config_description().to_string()
        };
        let _ = writeln!(out, "# {desc}");

        // Value from config (or defaults if missing).
        let key_strs = keys.bindings.get(config_name).cloned().unwrap_or_else(|| {
            def.default_keys
                .iter()
                .map(|k| keybindings::format_key_for_config(*k))
                .collect()
        });
        let _ = writeln!(out, "{config_name} = {}", render_string_list(&key_strs));
    }
    out.push('\n');
}

fn render_macros_config(
    out: &mut String,
    macros: &MacrosConfig,
    bindings: &crate::keybindings::RuntimeBindings,
) {
    let macro_key = bindings.label_for(crate::keybindings::Action::OpenMacroBar);
    out.push_str("[macros]\n");
    let _ = writeln!(
        out,
        "# Text macros: press {macro_key} to open the macro bar and select one to send.\n\
         # Each entry is a name mapped to its text and a surface restriction.\n\
         # surface = \"agent\"    — only shown when the agent pane is focused.\n\
         # surface = \"terminal\" — only shown when the terminal pane is focused.\n\
         # surface = \"both\"     — shown on both surfaces.\n\
         # Newlines in text values are translated to Alt+Enter (ESC + CR) so\n\
         # multi-line macros are entered as a single prompt; press Enter yourself\n\
         # to submit afterwards.",
    );
    if macros.entries.is_empty() {
        out.push_str(
            "# \"Review\" = { text = \"review this code for bugs\", surface = \"agent\" }\n\
             # \"Build\" = { text = \"cargo build --release\", surface = \"terminal\" }\n",
        );
    } else {
        out.push('\n');
        for (name, entry) in &macros.entries {
            let text = escape_toml_string(&entry.text);
            let surface = match entry.surface {
                MacroSurface::Agent => "agent",
                MacroSurface::Terminal => "terminal",
                MacroSurface::Both => "both",
            };
            let _ = writeln!(
                out,
                "\"{}\" = {{ text = \"{}\", surface = \"{}\" }}",
                escape_toml_string(name),
                text,
                surface
            );
        }
    }
}

fn escape_toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn render_string_list(values: &[String]) -> String {
    let rendered = values
        .iter()
        .map(|value| format!("\"{}\"", escape_toml_string(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{rendered}]")
}

fn render_provider_configs(out: &mut String, providers: &ProvidersConfig) {
    for (name, config) in &providers.commands {
        render_provider_config(out, name, config);
    }
}

fn render_project_configs(out: &mut String, projects: &[ProjectConfig]) {
    out.push_str(
        "# Projects are mirrored with dux's runtime database.\n\
         # Paths may use $HOME, ${HOME}, or ~ for portability across machines.\n\
         # startup_command runs in each new agent worktree before the provider launches.\n\
         # env defines per-project variables passed to agent and companion terminal PTYs.\n\
         # Values may reference existing environment variables with $VAR or ${VAR}.\n",
    );
    if projects.is_empty() {
        out.push_str(
            "# [[projects]]\n\
             # id = \"00000000-0000-0000-0000-000000000000\"\n\
             # path = \"$HOME/projects/example\"\n\
             # name = \"example\"\n\
             # default_provider = \"codex\"\n\
             # auto_reopen_agents = true\n\
             # startup_command = \"npm install\"\n\
             # env = { EDITOR = \"true\", API_KEY = \"${FOOBAR_API_KEY}\" }\n\n",
        );
        return;
    }

    for project in projects {
        out.push_str("[[projects]]\n");
        out.push_str(&format!("id = \"{}\"\n", escape_toml_string(&project.id)));
        out.push_str(&format!(
            "path = \"{}\"\n",
            escape_toml_string(&project.path)
        ));
        if let Some(name) = &project.name {
            out.push_str(&format!("name = \"{}\"\n", escape_toml_string(name)));
        }
        if let Some(provider) = &project.default_provider {
            out.push_str(&format!(
                "default_provider = \"{}\"\n",
                escape_toml_string(provider)
            ));
        }
        if let Some(auto_reopen_agents) = project.auto_reopen_agents {
            out.push_str(&format!("auto_reopen_agents = {auto_reopen_agents}\n"));
        }
        if let Some(command) = &project.startup_command {
            out.push_str(&format!(
                "startup_command = \"{}\"\n",
                escape_toml_string(command)
            ));
        }
        if !project.env.is_empty() {
            out.push_str("env = { ");
            for (index, (key, value)) in project.env.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{} = \"{}\"", key, escape_toml_string(value)));
            }
            out.push_str(" }\n");
        }
        out.push('\n');
    }
}

fn render_env_config(out: &mut String, env: &BTreeMap<String, String>) {
    out.push_str("[env]\n");
    out.push_str(
        "# Environment variables passed to every agent PTY, companion terminal,\n\
         # and startup command. Project-level env overrides keys defined here.\n\
         # Values may reference existing environment variables with $VAR or ${VAR}.\n",
    );
    if env.is_empty() {
        out.push_str(
            "# EDITOR = \"true\"\n\
             # API_KEY = \"${FOOBAR_API_KEY}\"\n\n",
        );
        return;
    }
    for (name, value) in env {
        out.push_str(&format!("{} = \"{}\"\n", name, escape_toml_string(value)));
    }
    out.push('\n');
}

fn render_auth_config(
    out: &mut String,
    auth: &AuthConfig,
    bindings: &crate::keybindings::RuntimeBindings,
) {
    let palette_key = bindings.label_for(crate::keybindings::Action::OpenPalette);
    out.push_str("[auth]\n");
    let _ = writeln!(
        out,
        "# Login credentials for the `dux server` web UI. Each entry is an\n\
         # htpasswd-style \"username:bcrypt-hash\" string, for example:\n\
         #   users = [\"alice:$2y$12$......\"]\n\
         # The hash must be bcrypt (the $2a$/$2b$/$2y$ family); plaintext is never\n\
         # stored. Manage entries with the server-add-user and server-remove-user\n\
         # commands in the palette ({palette_key}) rather than editing hashes by hand.\n\
         # The login gate turns ON automatically as soon as at least one user is\n\
         # listed here, and OFF when the list is empty. To run with no login (for\n\
         # example behind an upstream auth proxy), leave this empty and start the\n\
         # server with `dux server --disable-auth`.",
    );
    out.push_str(&format!("users = {}\n\n", render_string_list(&auth.users)));
}

fn render_server_acme_config(out: &mut String, acme: &AcmeSettings) {
    out.push_str("[server.acme]\n");
    out.push_str(
        "# Built-in TLS via Let's Encrypt (ACME HTTP-01). When enabled, `dux server`\n\
         # obtains and renews real certificates itself and serves HTTPS — no reverse\n\
         # proxy required. This uses the HTTP-01 challenge, which means BOTH must hold:\n\
         #   1. Each domain below has a public DNS A/AAAA record pointing at this host.\n\
         #   2. Inbound http_port (default 80) is reachable from the internet so\n\
         #      Let's Encrypt can fetch the challenge token.\n\
         # The :80 listener answers the ACME challenge and otherwise redirects to HTTPS;\n\
         # :443 serves the TLS web UI. Both ports are configurable below.\n\
         #\n\
         # Behind your own reverse proxy instead? Leave enabled = false, let the proxy\n\
         # terminate TLS, and point [server] bind at a loopback or private address.\n\
         #\n\
         # NOTE: ACME settings are read once when the server starts. Changing them and\n\
         # running reload-config does NOT rebind the listeners — restart the server.\n",
    );
    out.push_str(
        "# Turn the built-in ACME server on. Leave false to serve plain HTTP (loopback\n\
         # dev, or TLS terminated by an upstream proxy).\n",
    );
    out.push_str(&format!("enabled = {}\n", acme.enabled));
    out.push_str(
        "# Domains to request certificates for. Each one needs a public DNS record\n\
         # resolving to this host. Example: domains = [\"dux.example.com\"].\n",
    );
    out.push_str(&format!(
        "domains = {}\n",
        render_string_list(&acme.domains)
    ));
    out.push_str("# Contact email Let's Encrypt uses for expiry/renewal notices (recommended).\n");
    out.push_str(&format!(
        "email = \"{}\"\n",
        escape_toml_string(&acme.email)
    ));
    out.push_str(
        "# Port for the HTTP-01 challenge and the HTTPS redirect. Let's Encrypt always\n\
         # connects to port 80 for HTTP-01, so keep this 80 unless a proxy forwards it.\n",
    );
    out.push_str(&format!("http_port = {}\n", acme.http_port));
    out.push_str("# Port the TLS web UI listens on (443 is the browser default).\n");
    out.push_str(&format!("https_port = {}\n", acme.https_port));
    out.push_str(
        "# Use the Let's Encrypt PRODUCTION directory (true) or the STAGING directory\n\
         # (false). Production has strict rate limits and issues browser-trusted certs;\n\
         # staging issues untrusted certs but lets you test the flow freely. Test with\n\
         # production = false first, then flip to true once the challenge succeeds.\n",
    );
    out.push_str(&format!("production = {}\n", acme.production));
    out.push_str(
        "# Directory holding the ACME account key and issued certificates. These are\n\
         # PRIVATE KEYS — keep the directory readable only by the dux user. Leave empty\n\
         # to use <config-dir>/acme. Env vars and a leading ~ are expanded.\n",
    );
    out.push_str(&format!(
        "cache_dir = \"{}\"\n\n",
        escape_toml_string(acme.cache_dir.as_deref().unwrap_or(""))
    ));
}

fn render_terminal_config(out: &mut String, terminal: &TerminalConfig) {
    out.push_str("[terminal]\n");
    out.push_str(
        "# CLI command dux should use for the companion terminal bound to an agent session.\n",
    );
    out.push_str(&format!(
        "command = \"{}\"\n",
        escape_toml_string(&terminal.command)
    ));
    out.push_str(
        "# Arguments for the companion terminal command. The default [\"-l\"] launches a login\n# shell so your profile, aliases, and prompt are loaded.\n",
    );
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&terminal.args)
    ));
}

fn render_startup_command_terminal_config(
    out: &mut String,
    terminal: &StartupCommandTerminalConfig,
) {
    out.push_str("[startup_command_terminal]\n");
    out.push_str(
        "# Shell used to run project startup commands before launching a new agent.\n\
         # \"$SHELL\" is expanded when the command runs and falls back to /bin/sh if unset.\n",
    );
    out.push_str(&format!(
        "command = \"{}\"\n",
        escape_toml_string(&terminal.command)
    ));
    out.push_str(
        "# Arguments passed before the configured project startup command.\n\
         # The default [\"-l\", \"-c\"] runs a login shell without interactive job-control warnings.\n",
    );
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&terminal.args)
    ));
}

fn render_provider_config(out: &mut String, name: &str, config: &ProviderCommandConfig) {
    out.push_str(&format!("[providers.{name}]\n"));
    out.push_str(&format!("# CLI command for {name} sessions.\n"));
    out.push_str(&format!(
        "command = \"{}\"\n",
        escape_toml_string(&config.command)
    ));
    out.push_str(
        "# Arguments passed to the provider command when launching an interactive PTY session.\n",
    );
    out.push_str(&format!("args = {}\n", render_string_list(&config.args)));
    out.push_str(
        "# Optional args dux should use when reconnecting a detached session.\n\
         # Leave this empty for CLIs that do not support cwd/repo-scoped session resume.\n",
    );
    out.push_str(&format!(
        "resume_args = {}\n",
        render_string_list(config.resume_args.as_deref().unwrap_or(&[]))
    ));
    out.push_str(
        "# Optional timeout for resumed sessions that produce no visible output.\n\
         # If resume hangs before rendering anything, dux kills it and retries fresh after this many milliseconds.\n\
         # Set to 0 to disable the timeout.\n",
    );
    out.push_str(&format!(
        "resume_wait_timeout_ms = {}\n",
        config.resume_wait_timeout_ms.unwrap_or(0)
    ));
    out.push_str("# Oneshot args for non-interactive use (e.g. AI commit messages).\n");
    out.push_str("# Placeholders: {prompt} = the prompt text, {tempfile} = temp file path.\n");
    out.push_str(&format!(
        "oneshot_args = {}\n",
        render_string_list(&config.oneshot_args)
    ));
    let output_str = match config.oneshot_output {
        OneshotOutput::Stdout => "stdout",
        OneshotOutput::Tempfile => "tempfile",
    };
    out.push_str("# Where to read the provider's oneshot response: \"stdout\" or \"tempfile\".\n");
    out.push_str(&format!("oneshot_output = \"{output_str}\"\n"));
    if let Some(hint) = &config.install_hint {
        out.push_str("# Hint shown to the user when the provider command is not found on PATH.\n");
        out.push_str(&format!(
            "install_hint = \"{}\"\n",
            escape_toml_string(hint)
        ));
    }
    out.push_str(
        "# When true, scroll events are forwarded to the provider instead of being\n\
         # handled as dux host scrollback. Enable this for agents that manage their\n\
         # own scrollback buffer (e.g. opencode).\n",
    );
    out.push_str(&format!("forward_scroll = {}\n", config.forward_scroll));
    out.push('\n');
}

/// Validate all key bindings in the config. Returns a descriptive error on failure.
///
/// Checks:
/// 1. Every action name is known (present in `BINDING_DEFS`).
/// 2. Every key string parses successfully after normalization
///    (bare uppercase letters like `"P"` are rewritten to `"shift-p"`).
/// 3. No two actions bind the same normalized key in overlapping scopes.
pub fn validate_keys(keys: &KeysConfig) -> Result<(), String> {
    for (name, key_strs) in &keys.bindings {
        let valid = keybindings::BINDING_DEFS
            .iter()
            .any(|d| d.action.config_name() == name);
        if !valid {
            return Err(format!("[keys] unknown action: \"{name}\""));
        }
        for s in key_strs {
            let normalized = keybindings::normalize_key_string(s);
            crokey::parse(&normalized)
                .map_err(|_| format!("[keys] invalid key \"{s}\" for action \"{name}\""))?;
        }
    }

    // Detect conflicting bindings (same key in overlapping scopes).
    let conflicts = keybindings::detect_conflicts(keys);
    if !conflicts.is_empty() {
        let mut msg = String::from("[keys] conflicting keybindings detected:");
        for c in &conflicts {
            msg.push_str(&format!(
                "\n  - \"{}\" is bound to both \"{}\" and \"{}\" in {}",
                c.key_label,
                c.action_a,
                c.action_b,
                c.scope.display_name(),
            ));
        }
        msg.push_str(
            "\nCheck your [keys.bindings] configuration and ensure each key is unique within its scope.",
        );
        return Err(msg);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    /// Render config using default keybinding labels (for tests that don't need custom bindings).
    fn render_config_default(config: &Config) -> String {
        let bindings =
            crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default());
        render_config(config, &bindings)
    }

    #[test]
    fn default_config_is_commented_and_complete() {
        let rendered = render_default_config();
        assert!(rendered.contains("# dux configuration"));
        assert!(rendered.contains("[defaults]"));
        assert!(rendered.contains("provider = \"claude\""));
        assert!(rendered.contains("enable_randomized_pet_name_by_default = false"));
        assert!(rendered.contains("pull_before_creating_agent_by_default = true"));
        assert!(!rendered.contains("prompt_for_name"));
        assert!(rendered.contains("[providers.claude]"));
        assert!(rendered.contains("[providers.codex]"));
        assert!(rendered.contains("[providers.copilot]"));
        assert!(rendered.contains("oneshot_args = "));
        assert!(rendered.contains("oneshot_output = "));
        assert!(rendered.contains("resume_args = "));
        assert!(rendered.contains("[terminal]"));
        assert!(rendered.contains("command = "));
        assert!(rendered.contains("args = []"));
        assert!(rendered.contains("[startup_command_terminal]"));
        assert!(rendered.contains("command = \"$SHELL\""));
        assert!(rendered.contains("args = [\"-l\", \"-c\"]"));
        assert!(rendered.contains("[ui]"));
        assert!(rendered.contains("agent_scrollback_lines = 10000"));
        assert!(rendered.contains("empty_project_separator_min_projects = 5"));
        assert!(rendered.contains("auto_reopen_agents = false"));
        assert!(rendered.contains("staged_pane_height_pct = "));
        assert!(rendered.contains("commit_pane_height_pct = "));
        assert!(rendered.contains("[editor]"));
        assert!(rendered.contains("default = \"cursor\""));
        assert!(rendered.contains("[server]"));
        assert!(rendered.contains("bind = \"127.0.0.1:8080\""));
        assert!(rendered.contains("insecure_allow_remote = false"));
        assert!(rendered.contains("[server.acme]"));
        assert!(rendered.contains("enabled = false"));
        assert!(rendered.contains("domains = []"));
        assert!(rendered.contains("http_port = 80"));
        assert!(rendered.contains("https_port = 443"));
        assert!(rendered.contains("production = true"));
        assert!(rendered.contains("[auth]"));
        assert!(rendered.contains("users = []"));
        assert!(rendered.contains("server-add-user"));
        assert!(rendered.contains("--disable-auth"));
        // The palette-open keybinding in the [auth] comment is interpolated from
        // the runtime bindings, never hardcoded (CLAUDE.md). The default label is
        // "Ctrl-p", but assert the dynamic value so a rebind keeps the comment
        // accurate.
        let palette_key =
            crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default())
                .label_for(crate::keybindings::Action::OpenPalette);
        assert!(rendered.contains(&format!("commands in the palette ({palette_key})")));
        assert!(rendered.contains("[keys]"));
        assert!(rendered.contains("show_terminal_keys = true"));
        assert!(rendered.contains("move_down = "));
        assert!(rendered.contains("quit = "));
        assert!(rendered.contains("commit_prompt = \"\"\""));
        assert!(rendered.contains("Conventional Commits"));
    }

    #[test]
    fn server_acme_section_documents_key_concepts() {
        // The config file IS the documentation: the [server.acme] comments must
        // explain HTTP-01 reachability, the staging/production toggle + rate
        // limits, the behind-a-proxy path, the private-key cache, and that ACME
        // changes need a server restart.
        let rendered = render_default_config();
        // HTTP-01: DNS record + inbound :80 reachable from the internet.
        assert!(rendered.contains("HTTP-01"), "must name the challenge type");
        assert!(
            rendered.contains("DNS A/AAAA record"),
            "must explain the DNS requirement"
        );
        assert!(
            rendered.contains("reachable from the internet"),
            "must explain the inbound :80 reachability requirement"
        );
        // Staging vs production + Let's Encrypt rate limits.
        assert!(
            rendered.contains("STAGING") && rendered.contains("PRODUCTION"),
            "must explain the staging/production toggle"
        );
        assert!(
            rendered.contains("rate limits"),
            "must warn about Let's Encrypt rate limits"
        );
        // Behind-my-own-proxy path: enabled = false + private/loopback bind.
        assert!(
            rendered.contains("reverse proxy") && rendered.contains("enabled = false"),
            "must describe the proxy path with enabled = false"
        );
        // Cache dir default + it holds PRIVATE KEYS.
        assert!(
            rendered.contains("<config-dir>/acme"),
            "must document the default cache dir"
        );
        assert!(
            rendered.contains("PRIVATE KEYS"),
            "must warn the cache dir holds private keys"
        );
        // ACME changes need a RESTART (reload-config does not rebind).
        assert!(
            rendered.contains("restart the server"),
            "must explain that ACME changes need a restart"
        );
    }

    #[test]
    fn rendered_config_with_acme_round_trips() {
        // The rendered template (including a populated [server.acme]) must
        // re-parse into an equivalent config — proving the section ordering is
        // valid TOML (subtable after the bare [server] keys).
        let mut config = Config::default();
        config.server.acme.enabled = true;
        config.server.acme.domains = vec!["dux.example.com".to_string()];
        config.server.acme.email = "ops@example.com".to_string();
        config.server.acme.production = false;
        config.server.acme.cache_dir = Some("/var/lib/dux/acme".to_string());

        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("rendered config must re-parse");
        assert!(parsed.server.acme.enabled);
        assert_eq!(
            parsed.server.acme.domains,
            vec!["dux.example.com".to_string()]
        );
        assert_eq!(parsed.server.acme.email, "ops@example.com");
        assert!(!parsed.server.acme.production);
        assert_eq!(
            parsed.server.acme.cache_dir.as_deref(),
            Some("/var/lib/dux/acme")
        );
    }

    #[test]
    fn validate_keys_accepts_valid_config() {
        let keys = KeysConfig::default();
        assert!(validate_keys(&keys).is_ok());
    }

    #[test]
    fn validate_keys_rejects_bad_key() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("quit".to_string(), vec!["badkey!!!".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("badkey!!!"));
    }

    #[test]
    fn validate_keys_rejects_unknown_action() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("nonexistent_action".to_string(), vec!["q".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonexistent_action"));
    }

    #[test]
    fn render_config_omits_legacy_projects() {
        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: new_project_id(),
            path: "/home/user/project".to_string(),
            name: Some("test".to_string()),
            default_provider: None,
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: Some("npm install".to_string()),
            env: Default::default(),
        });
        let rendered = render_config_default(&config);
        assert!(rendered.contains("[[projects]]"));
        assert!(rendered.contains("startup_command = \"npm install\""));
        assert!(!rendered.contains("leading_branch"));
        let parsed: Config = toml::from_str(&rendered).expect("should parse back");
        assert_eq!(parsed.projects.len(), 1);
    }

    #[test]
    fn legacy_projects_still_parse_for_migration() {
        let parsed: Config = toml::from_str(
            r#"
[[projects]]
id = "project-1"
path = "/home/user/project"
name = "test"
default_provider = "codex"
leading_branch = "main"
"#,
        )
        .expect("legacy projects should parse");
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "project-1");
        assert_eq!(parsed.projects[0].path, "/home/user/project");
        assert_eq!(
            parsed.projects[0].default_provider.as_deref(),
            Some("codex")
        );
        assert_eq!(parsed.projects[0].leading_branch.as_deref(), Some("main"));
    }

    #[test]
    fn old_config_missing_pull_before_create_defaults_to_true() {
        let parsed: Config = toml::from_str(
            r#"
[defaults]
provider = "claude"
start_directory = "/tmp"
commit_prompt = ""
enable_randomized_pet_name_by_default = false
"#,
        )
        .expect("config should parse");

        assert!(parsed.defaults.pull_before_creating_agent_by_default);
    }

    #[test]
    fn old_config_missing_startup_command_terminal_uses_portable_default() {
        let parsed: Config = toml::from_str(
            r#"
[defaults]
provider = "claude"
"#,
        )
        .expect("config should parse");

        assert_eq!(parsed.startup_command_terminal.command, "$SHELL");
        assert_eq!(parsed.startup_command_terminal.args, ["-l", "-c"]);
    }

    #[test]
    fn save_config_strips_legacy_projects() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
[[projects]]
id = "project-1"
path = "/home/user/project"
name = "test"
"#,
        )
        .expect("write config");

        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: "project-1".to_string(),
            path: "/home/user/path\nwith\nnewlines".to_string(),
            name: Some("name\twith\ttabs".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: Some("echo ready".to_string()),
            env: Default::default(),
        });
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        save_config(&config_path, &config, &bindings).expect("save config");

        let saved = fs::read_to_string(config_path).expect("read config");
        assert!(saved.contains("[[projects]]"));
        assert!(saved.contains("project-1"));
        assert!(saved.contains("startup_command = \"echo ready\""));
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let rendered = render_default_config();
        let parsed: Config = toml::from_str(&rendered).expect("default config should parse");
        let re_rendered = render_config_default(&parsed);
        assert_eq!(
            rendered, re_rendered,
            "render → parse → render should be stable"
        );
    }

    #[test]
    fn default_config_round_trips_agent_scrollback_lines() {
        let mut config = Config::default();
        config.ui.agent_scrollback_lines = 12_345;
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.ui.agent_scrollback_lines, 12_345);
    }

    #[test]
    fn default_config_round_trips_auto_reopen_options() {
        let mut config = Config::default();
        config.ui.auto_reopen_agents = true;

        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");

        assert!(parsed.ui.auto_reopen_agents);
    }

    #[test]
    fn default_config_round_trips_default_editor() {
        let mut config = Config::default();
        config.editor.default = "zed".to_string();
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.editor.default, "zed");
    }

    #[test]
    fn default_config_round_trips_terminal_command() {
        let mut config = Config::default();
        config.terminal.command = "fish".to_string();
        config.terminal.args = vec!["-l".to_string()];
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.terminal.command, "fish");
        assert_eq!(parsed.terminal.args, vec!["-l"]);
    }

    #[test]
    fn default_config_round_trips_startup_command_terminal() {
        let mut config = Config::default();
        config.startup_command_terminal.command = "/bin/bash".to_string();
        config.startup_command_terminal.args = vec!["-l".to_string(), "-c".to_string()];
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.startup_command_terminal.command, "/bin/bash");
        assert_eq!(parsed.startup_command_terminal.args, vec!["-l", "-c"]);
    }

    #[test]
    fn default_config_round_trips_staged_pane_height() {
        let mut config = Config::default();
        config.ui.staged_pane_height_pct = 65;
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.ui.staged_pane_height_pct, 65);
    }

    #[test]
    fn old_config_missing_staged_pane_height_defaults_to_50() {
        let toml_str = r#"
[ui]
left_width_pct = 20
right_width_pct = 23
terminal_pane_height_pct = 35
agent_scrollback_lines = 10000
"#;
        let parsed: Config = toml::from_str(toml_str).expect("should parse");
        assert_eq!(parsed.ui.empty_project_separator_min_projects, 5);
        assert_eq!(parsed.ui.staged_pane_height_pct, 50);
        assert_eq!(parsed.ui.commit_pane_height_pct, 40);
    }

    #[test]
    fn config_deprecation_replace_migrates_prompt_for_name() {
        let mut doc: DocumentMut = r#"
[defaults]
provider = "claude"
prompt_for_name = false
"#
        .parse()
        .expect("parse doc");

        let changed = apply_config_deprecations(&mut doc).expect("migrate");

        assert!(changed);
        let defaults = doc["defaults"].as_table().expect("defaults table");
        assert!(!defaults.contains_key("prompt_for_name"));
        assert_eq!(
            defaults["enable_randomized_pet_name_by_default"]
                .as_value()
                .and_then(Value::as_bool),
            Some(true),
        );
    }

    #[test]
    fn config_deprecation_replace_preserves_explicit_new_key() {
        let mut doc: DocumentMut = r#"
[defaults]
prompt_for_name = false
enable_randomized_pet_name_by_default = false
"#
        .parse()
        .expect("parse doc");

        apply_config_deprecations(&mut doc).expect("migrate");

        let defaults = doc["defaults"].as_table().expect("defaults table");
        assert!(!defaults.contains_key("prompt_for_name"));
        assert_eq!(
            defaults["enable_randomized_pet_name_by_default"]
                .as_value()
                .and_then(Value::as_bool),
            Some(false),
        );
    }

    #[test]
    fn config_deprecation_remove_discards_old_key() {
        let mut doc: DocumentMut = r#"
[defaults]
obsolete = true
"#
        .parse()
        .expect("parse doc");
        let rules = [DeprecatedConfigKeyRule {
            old: DeprecatedConfigKey {
                section: "defaults",
                key: "obsolete",
            },
            action: DeprecatedConfigKeyAction::Remove,
        }];

        let changed = apply_config_deprecations_with(&mut doc, &rules).expect("remove");

        assert!(changed);
        assert!(!doc["defaults"].as_table().unwrap().contains_key("obsolete"));
    }

    #[test]
    fn config_deprecation_fail_rejects_old_key() {
        let mut doc: DocumentMut = r#"
[defaults]
dangerous = true
"#
        .parse()
        .expect("parse doc");
        let rules = [DeprecatedConfigKeyRule {
            old: DeprecatedConfigKey {
                section: "defaults",
                key: "dangerous",
            },
            action: DeprecatedConfigKeyAction::Fail {
                message: "remove it manually",
            },
        }];

        let err = apply_config_deprecations_with(&mut doc, &rules).expect_err("should fail");

        assert!(err.to_string().contains("unsupported config key"));
        assert!(err.to_string().contains("remove it manually"));
    }

    #[test]
    fn default_config_round_trips_commit_pane_height() {
        let mut config = Config::default();
        config.ui.commit_pane_height_pct = 30;
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.ui.commit_pane_height_pct, 30);
    }

    #[test]
    fn built_in_providers_ship_resume_args() {
        let config = Config::default();
        assert_eq!(config.defaults.provider, "claude");
        let claude = config
            .providers
            .get("claude")
            .expect("claude provider should exist");
        assert_eq!(
            claude.resume_args.clone(),
            Some(vec!["--continue".to_string()])
        );
        assert!(claude.supports_session_resume());

        let codex = config
            .providers
            .get("codex")
            .expect("codex provider should exist");
        assert_eq!(
            codex.resume_args.clone(),
            Some(vec!["resume".to_string(), "--last".to_string()])
        );
        assert!(codex.supports_session_resume());
    }

    #[test]
    fn provider_command_config_appends_resume_args_when_available() {
        let cfg = ProviderCommandConfig {
            command: "example".to_string(),
            args: vec!["--interactive".to_string()],
            resume_args: Some(vec!["--resume".to_string(), "--last".to_string()]),
            resume_wait_timeout_ms: Some(2_000),
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
            forward_scroll: false,
        };
        assert_eq!(cfg.interactive_args(false), ["--interactive"]);
        assert_eq!(
            cfg.interactive_args(true),
            ["--interactive", "--resume", "--last"]
        );

        let unsupported = ProviderCommandConfig {
            command: "example".to_string(),
            args: vec!["--interactive".to_string()],
            resume_args: None,
            resume_wait_timeout_ms: None,
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
            forward_scroll: false,
        };
        assert_eq!(unsupported.interactive_args(true), ["--interactive"]);
        assert!(!unsupported.supports_session_resume());
    }

    #[test]
    fn ensure_defaults_backfills_missing_resume_args_for_builtins() {
        let mut providers = ProvidersConfig {
            commands: IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: None,
                    resume_wait_timeout_ms: None,
                    oneshot_args: Vec::new(),
                    oneshot_output: OneshotOutput::Stdout,
                    install_hint: None,
                    forward_scroll: false,
                },
            )]),
        };

        providers.ensure_defaults();

        let claude = providers
            .get("claude")
            .expect("claude provider should still exist");
        assert_eq!(
            claude.resume_args.clone(),
            Some(vec!["--continue".to_string()])
        );
    }

    #[test]
    fn ensure_defaults_preserves_explicit_resume_disable() {
        let mut providers = ProvidersConfig {
            commands: IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: Some(Vec::new()),
                    resume_wait_timeout_ms: None,
                    oneshot_args: Vec::new(),
                    oneshot_output: OneshotOutput::Stdout,
                    install_hint: None,
                    forward_scroll: false,
                },
            )]),
        };

        providers.ensure_defaults();

        let claude = providers
            .get("claude")
            .expect("claude provider should still exist");
        assert_eq!(claude.resume_args, Some(Vec::new()));
        assert!(!claude.supports_session_resume());
    }

    #[test]
    fn built_in_opencode_ships_resume_timeout() {
        let config = Config::default();
        let opencode = config
            .providers
            .get("opencode")
            .expect("opencode provider should exist");
        assert_eq!(opencode.resume_wait_timeout_ms, Some(3_000));
    }

    #[test]
    fn provider_configs_without_resume_args_still_parse() {
        let parsed: Config = toml::from_str(
            r#"
            [defaults]
            provider = "claude"

            [logging]
            level = "info"
            path = "dux.log"

            [ui]
            left_width_pct = 20
            right_width_pct = 23
            agent_scrollback_lines = 10000

            [editor]
            default = "cursor"

            [keys]
            show_terminal_keys = true

            [providers.custom]
            command = "custom-agent"
            args = ["chat"]
            oneshot_args = ["ask", "{prompt}"]
            oneshot_output = "stdout"
            "#,
        )
        .expect("legacy provider config should parse");

        let provider = parsed
            .providers
            .get("custom")
            .expect("custom provider should exist");
        assert_eq!(provider.resume_args, None);
        assert_eq!(provider.resume_wait_timeout_ms, None);
        assert_eq!(provider.interactive_args(true), ["chat"]);
    }

    #[test]
    fn legacy_provider_config_without_resume_args_still_parses() {
        let parsed: ProviderCommandConfig = toml::from_str(
            r#"
command = "legacy-agent"
args = ["serve"]
oneshot_args = ["--prompt", "{prompt}"]
oneshot_output = "stdout"
"#,
        )
        .expect("legacy provider config should parse");
        assert_eq!(parsed.command, "legacy-agent");
        assert_eq!(parsed.args, vec!["serve"]);
        assert_eq!(parsed.resume_args, None);
        assert_eq!(parsed.resume_wait_timeout_ms, None);
        assert!(!parsed.supports_session_resume());
    }

    #[test]
    fn default_config_keys_valid_after_round_trip() {
        let rendered = render_default_config();
        let parsed: Config = toml::from_str(&rendered).expect("default config should parse");
        validate_keys(&parsed.keys).expect("round-tripped keys should be valid");
    }

    #[test]
    fn default_commit_prompt_uses_system_default() {
        let mut config = Config::default();
        config.defaults.commit_prompt = Some("custom system prompt".to_string());
        assert_eq!(config.default_commit_prompt(), "custom system prompt");

        // Empty system prompt falls back to hardcoded constant.
        config.defaults.commit_prompt = Some(String::new());
        assert_eq!(config.default_commit_prompt(), DEFAULT_COMMIT_PROMPT);

        // None falls back to hardcoded constant.
        config.defaults.commit_prompt = None;
        assert_eq!(config.default_commit_prompt(), DEFAULT_COMMIT_PROMPT);
    }

    #[test]
    fn multiline_string_with_triple_quotes_roundtrips() {
        let mut config = Config::default();
        config.defaults.commit_prompt =
            Some("Use triple \"\"\" quotes in your prompt.".to_string());
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("should parse back");
        assert_eq!(
            parsed.defaults.commit_prompt.as_deref(),
            Some("Use triple \"\"\" quotes in your prompt."),
        );
    }

    #[test]
    fn config_comment_uses_dynamic_keybinding_label() {
        let rendered = render_default_config();
        // The default binding for GenerateCommitMessage is Ctrl-g.
        assert!(
            rendered.contains("(Ctrl-g)"),
            "config comment should include the dynamic keybinding label"
        );
        assert!(
            !rendered.contains("(Ctrl+G)"),
            "config comment should NOT contain hardcoded Ctrl+G"
        );
    }

    #[test]
    fn default_claude_oneshot_uses_bare_and_no_tools() {
        let providers = default_provider_commands();
        let claude = &providers[0].1;
        assert!(
            claude.oneshot_args.contains(&"--bare".to_string()),
            "claude oneshot_args should include --bare"
        );
        assert!(
            claude.oneshot_args.contains(&"--tools".to_string()),
            "claude oneshot_args should include --tools flag"
        );
        // --tools is followed by an empty string to disable all tools
        let tools_idx = claude
            .oneshot_args
            .iter()
            .position(|a| a == "--tools")
            .unwrap();
        assert_eq!(
            claude.oneshot_args[tools_idx + 1],
            "",
            "--tools should be followed by empty string to disable tools"
        );
        assert!(
            claude.oneshot_args.contains(&"--max-turns".to_string()),
            "claude oneshot_args should include --max-turns"
        );
    }

    #[test]
    fn default_opencode_oneshot_uses_run_subcommand() {
        let providers = default_provider_commands();
        let opencode = providers.iter().find(|(n, _)| *n == "opencode").unwrap();
        let cfg = &opencode.1;
        assert_eq!(cfg.command, "opencode");
        assert_eq!(cfg.oneshot_args, vec!["run", "{prompt}"]);
        assert!(matches!(cfg.oneshot_output, OneshotOutput::Stdout));
        assert!(cfg.resume_args.is_some());
    }

    #[test]
    fn default_gemini_oneshot_uses_prompt_flag() {
        let providers = default_provider_commands();
        let gemini = providers.iter().find(|(n, _)| *n == "gemini").unwrap();
        let cfg = &gemini.1;
        assert_eq!(cfg.command, "gemini");
        assert_eq!(cfg.oneshot_args, vec!["-p", "{prompt}"]);
        assert!(matches!(cfg.oneshot_output, OneshotOutput::Stdout));
        assert!(cfg.resume_args.is_some());
    }

    #[test]
    fn default_copilot_oneshot_uses_prompt_flag_with_allow_all_tools() {
        let providers = default_provider_commands();
        let copilot = providers.iter().find(|(n, _)| *n == "copilot").unwrap();
        let cfg = &copilot.1;
        assert_eq!(cfg.command, "copilot");
        assert_eq!(
            cfg.oneshot_args,
            vec!["-p", "{prompt}", "--allow-all-tools"]
        );
        assert!(matches!(cfg.oneshot_output, OneshotOutput::Stdout));
        assert_eq!(cfg.resume_args, None);
        assert!(!cfg.supports_session_resume());
    }

    #[test]
    fn ensure_defaults_adds_opencode_and_gemini() {
        let mut providers = ProvidersConfig {
            commands: indexmap::IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: Some(vec!["--continue".to_string()]),
                    resume_wait_timeout_ms: None,
                    oneshot_args: vec!["-p".to_string(), "{prompt}".to_string()],
                    oneshot_output: OneshotOutput::Stdout,
                    install_hint: None,
                    forward_scroll: false,
                },
            )]),
        };

        providers.ensure_defaults();

        assert!(
            providers.get("opencode").is_some(),
            "opencode should be added"
        );
        assert!(providers.get("gemini").is_some(), "gemini should be added");
        assert!(providers.get("codex").is_some(), "codex should be added");
        assert!(
            providers.get("copilot").is_some(),
            "copilot should be added"
        );
        assert_eq!(providers.get("opencode").unwrap().command, "opencode");
        assert_eq!(providers.get("gemini").unwrap().command, "gemini");
        assert_eq!(providers.get("copilot").unwrap().command, "copilot");
    }

    #[test]
    fn validate_keys_normalizes_bare_uppercase() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("quit".to_string(), vec!["P".to_string()]);
        // Should succeed — "P" is normalized to "shift-p" before parsing.
        assert!(
            validate_keys(&keys).is_ok(),
            "bare uppercase 'P' should be normalized to 'shift-p' and accepted"
        );
    }

    #[test]
    fn validate_keys_detects_conflict() {
        let mut keys = KeysConfig::default();
        // Bind the same key to two actions that share the Left scope.
        keys.bindings
            .insert("toggle_project".to_string(), vec!["x".to_string()]);
        keys.bindings
            .insert("new_agent".to_string(), vec!["x".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err(), "duplicate key in same scope should error");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("conflicting"),
            "error should mention conflict: {msg}"
        );
        assert!(
            msg.contains("toggle_project"),
            "error should name first action: {msg}"
        );
        assert!(
            msg.contains("new_agent"),
            "error should name second action: {msg}"
        );
    }

    // ── MacrosConfig tests ────────────────────────────────────────

    #[test]
    fn macros_config_default_is_empty() {
        let config = MacrosConfig::default();
        assert!(config.entries.is_empty());
    }

    #[test]
    fn macros_config_entry_round_trip() {
        let toml_str = r#"
"Review" = { text = "review this code", surface = "agent" }
"#;
        let config: MacrosConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.entries.len(), 1);
        assert_eq!(config.entries["Review"].text, "review this code");
        assert_eq!(config.entries["Review"].surface, MacroSurface::Agent);
    }

    #[test]
    fn macros_config_multiple_entries() {
        let toml_str = r#"
"Explain" = { text = "explain what this function does", surface = "agent" }
"Review" = { text = "review this code for bugs", surface = "both" }
"Build" = { text = "cargo build", surface = "terminal" }
"#;
        let config: MacrosConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.entries.len(), 3);
        assert_eq!(
            config.entries["Explain"].text,
            "explain what this function does"
        );
        assert_eq!(config.entries["Explain"].surface, MacroSurface::Agent);
        assert_eq!(config.entries["Review"].surface, MacroSurface::Both);
        assert_eq!(config.entries["Build"].surface, MacroSurface::Terminal);
    }

    #[test]
    fn macros_config_preserves_declaration_order() {
        // Names are deliberately non-alphabetical to verify we get declaration
        // order (IndexMap) rather than sorted order (BTreeMap).
        let toml_str = r#"
"Zebra" = { text = "z cmd", surface = "agent" }
"Alpha" = { text = "a cmd", surface = "terminal" }
"Middle" = { text = "m cmd", surface = "both" }
"#;
        let config: MacrosConfig = toml::from_str(toml_str).unwrap();
        let names: Vec<&str> = config.entries.keys().map(|s| s.as_str()).collect();
        assert_eq!(names, vec!["Zebra", "Alpha", "Middle"]);
    }

    #[test]
    fn macros_config_order_survives_serialize_round_trip() {
        let toml_str = r#"
"Zebra" = { text = "z cmd", surface = "agent" }
"Alpha" = { text = "a cmd", surface = "terminal" }
"Middle" = { text = "m cmd", surface = "both" }
"#;
        let config: MacrosConfig = toml::from_str(toml_str).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let round_tripped: MacrosConfig = toml::from_str(&serialized).unwrap();
        let names: Vec<&str> = round_tripped.entries.keys().map(|s| s.as_str()).collect();
        assert_eq!(names, vec!["Zebra", "Alpha", "Middle"]);
    }

    #[test]
    fn macros_config_insert_order_preserved() {
        let mut config = MacrosConfig::default();
        config.entries.insert(
            "Zulu".into(),
            MacroEntry {
                text: "z".into(),
                surface: MacroSurface::Agent,
            },
        );
        config.entries.insert(
            "Alpha".into(),
            MacroEntry {
                text: "a".into(),
                surface: MacroSurface::Agent,
            },
        );
        config.entries.insert(
            "Mike".into(),
            MacroEntry {
                text: "m".into(),
                surface: MacroSurface::Agent,
            },
        );
        let names: Vec<&str> = config.entries.keys().map(|s| s.as_str()).collect();
        assert_eq!(names, vec!["Zulu", "Alpha", "Mike"]);
    }

    #[test]
    fn macros_surface_default_is_agent() {
        assert_eq!(MacroSurface::default(), MacroSurface::Agent);
    }

    #[test]
    fn macros_surface_matches() {
        use crate::model::SessionSurface;
        assert!(MacroSurface::Both.matches(SessionSurface::Agent));
        assert!(MacroSurface::Both.matches(SessionSurface::Terminal));
        assert!(MacroSurface::Agent.matches(SessionSurface::Agent));
        assert!(!MacroSurface::Agent.matches(SessionSurface::Terminal));
        assert!(MacroSurface::Terminal.matches(SessionSurface::Terminal));
        assert!(!MacroSurface::Terminal.matches(SessionSurface::Agent));
    }

    #[test]
    fn macros_surface_next_cycles() {
        assert_eq!(MacroSurface::Agent.next(), MacroSurface::Terminal);
        assert_eq!(MacroSurface::Terminal.next(), MacroSurface::Both);
        assert_eq!(MacroSurface::Both.next(), MacroSurface::Agent);
    }

    #[test]
    fn macros_surface_prev_cycles() {
        assert_eq!(MacroSurface::Agent.prev(), MacroSurface::Both);
        assert_eq!(MacroSurface::Both.prev(), MacroSurface::Terminal);
        assert_eq!(MacroSurface::Terminal.prev(), MacroSurface::Agent);
    }

    #[test]
    fn render_macros_config_empty() {
        let config = Config::default();
        let rendered = render_config_default(&config);
        assert!(rendered.contains("[macros]"));
        assert!(rendered.contains("# \"Review\" = { text = \"review this code"));
        assert!(rendered.contains("surface = \"agent\""));
    }

    #[test]
    fn render_macros_config_with_entries() {
        let mut config = Config::default();
        config.macros.entries.insert(
            "Review".to_string(),
            MacroEntry {
                text: "hello world".to_string(),
                surface: MacroSurface::Agent,
            },
        );
        config.macros.entries.insert(
            "Test".to_string(),
            MacroEntry {
                text: "foo bar".to_string(),
                surface: MacroSurface::Terminal,
            },
        );
        let rendered = render_config_default(&config);
        assert!(rendered.contains("\"Review\" = { text = \"hello world\", surface = \"agent\" }"));
        assert!(rendered.contains("\"Test\" = { text = \"foo bar\", surface = \"terminal\" }"));
    }

    #[test]
    fn render_auth_config_interpolates_rebound_palette_key() {
        // The [auth] comment must reflect the USER's palette binding, not a
        // hardcoded "(Ctrl-p)" (CLAUDE.md). Rebind the palette and assert the
        // comment names the new key and never the literal default.
        let mut config = Config::default();
        config
            .keys
            .bindings
            .insert("open_palette".to_string(), vec!["Ctrl-k".to_string()]);
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        let rendered = render_config(&config, &bindings);
        let palette_key = bindings.label_for(crate::keybindings::Action::OpenPalette);
        assert_eq!(
            palette_key, "Ctrl-k",
            "rebind must take effect in the label"
        );
        assert!(
            rendered.contains("commands in the palette (Ctrl-k)"),
            "the [auth] comment must interpolate the rebound palette key"
        );
        assert!(
            !rendered.contains("commands in the palette (Ctrl-p)"),
            "the [auth] comment must not hardcode the default palette key"
        );
    }

    #[test]
    fn render_macros_config_escapes_special_chars() {
        let mut config = Config::default();
        config.macros.entries.insert(
            "Multi".to_string(),
            MacroEntry {
                text: "line1\nline2".to_string(),
                surface: MacroSurface::Both,
            },
        );
        let rendered = render_config_default(&config);
        assert!(rendered.contains("\"Multi\" = { text = \"line1\\nline2\", surface = \"both\" }"));
    }

    #[test]
    fn save_config_preserves_user_comments() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        // Write a config with a user comment.
        let initial = "\
# My custom note about this config
[ui]
left_width_pct = 20
right_width_pct = 23
terminal_pane_height_pct = 35
staged_pane_height_pct = 50
commit_pane_height_pct = 40
agent_scrollback_lines = 10000
branch_sync_interval = 30
show_diff_line_numbers = false
github_integration = true

[logging]
level = \"info\"
path = \"dux.log\"

[defaults]
provider = \"claude\"

[editor]
default = \"cursor\"

[keys]
show_terminal_keys = true

[terminal]
command = \"/bin/sh\"
args = [\"-l\"]
";
        fs::write(&config_path, initial).expect("write initial");

        // Modify a value and save.
        let mut config = Config::default();
        config.ui.left_width_pct = 25;
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        save_config(&config_path, &config, &bindings).expect("save");

        let saved = fs::read_to_string(&config_path).expect("read back");
        // The user comment must still be present.
        assert!(
            saved.contains("# My custom note about this config"),
            "user comment was lost: {saved}"
        );
        // The value must be updated.
        assert!(
            saved.contains("left_width_pct = 25"),
            "value not updated: {saved}"
        );
    }

    #[test]
    fn save_config_round_trips_values() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");

        // Start from canonical default.
        let default_body = render_default_config();
        fs::write(&config_path, &default_body).expect("write");

        // Modify and save.
        let mut config: Config = toml::from_str(&default_body).expect("parse");
        config.ui.right_width_pct = 30;
        config.ui.auto_reopen_agents = true;
        config.defaults.pull_before_creating_agent_by_default = false;
        config.editor.default = "zed".to_string();
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        save_config(&config_path, &config, &bindings).expect("save");

        // Re-read and verify values round-tripped.
        let saved = fs::read_to_string(&config_path).expect("read");
        let reloaded: Config = toml::from_str(&saved).expect("parse saved");
        assert_eq!(reloaded.ui.right_width_pct, 30);
        assert!(reloaded.ui.auto_reopen_agents);
        assert!(!reloaded.defaults.pull_before_creating_agent_by_default);
        assert_eq!(reloaded.editor.default, "zed");
    }

    #[test]
    fn rendered_default_config_documents_global_env() {
        let rendered = render_default_config();

        assert!(rendered.contains("[env]"));
        assert!(rendered.contains("# EDITOR = \"true\""));
        assert!(rendered.contains("# API_KEY = \"${FOOBAR_API_KEY}\""));
    }
}
