use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item};

use crate::keybindings;

pub use dux_core::config::*;

#[allow(deprecated)] // blessed sync-direct: boot/first-creation path runs before the queue exists
pub fn ensure_config(paths: &DuxPaths) -> Result<Config> {
    // Any core-side write that CREATES config.toml (bootstrap project-sync) must
    // emit the commented template, not a bare one.
    install_canonical_renderer();
    paths.ensure_dirs()?;
    if !paths.config_path.exists() {
        dux_core::config_write::write_config_secure(&paths.config_path, &render_default_config())
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
    }

    let raw = fs::read_to_string(&paths.config_path)
        .with_context(|| format!("failed to read {}", paths.config_path.display()))?;
    let mut doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    // The deprecated-key + retired-provider migrations are the core-owned
    // `dux_core::config_migrate::apply_load_migrations` (also applied in memory
    // by `load_config`, so `dux serve` honors them); the TUI ADDITIONALLY
    // persists the migrated document. Retired KEYBINDING actions are pruned only
    // here (they matter only to the TUI's `validate_keys`).
    let migrations_changed = dux_core::config_migrate::apply_load_migrations(&mut doc)?;
    let retired_keys_changed = prune_retired_key_actions(&mut doc);
    if migrations_changed || retired_keys_changed {
        // blessed sync-direct: deprecation/retirement migration also runs at boot before the queue exists
        dux_core::config_write::write_config_secure(&paths.config_path, &doc.to_string())
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
    }

    let mut config: Config = toml::from_str(&doc.to_string())
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    config.providers.ensure_defaults();
    validate_server_host(&config)?;
    validate_project_envs(&config)?;
    // Warn once here (TUI startup and reload both funnel through ensure_config) on
    // an unrecognized clipboard_passthrough so the per-tick host forward can parse
    // silently (FIX-F5). The warning is from_config_str's side effect.
    let _ = ClipboardPassthroughMode::from_config_str(&config.capabilities.clipboard_passthrough);
    Ok(config)
}

/// Reject a `[server] host` that is not an IP literal before the TUI starts.
/// `dux server` resolves the bind plan with its own `?` validation as a backstop,
/// but the TUI flip reads `host` too, so catch a bad value here with a clear
/// message rather than failing later. Delegates to the single-source
/// `dux_core::config::parse_server_host` (trimming and message shared with
/// `resolve_server_plan`, so both accept exactly the same values).
fn validate_server_host(config: &Config) -> Result<()> {
    dux_core::config::parse_server_host(&config.server.host).map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
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

// ---------------------------------------------------------------------------
// Retired keybinding actions
//
// An action that once shipped (and could be bound in `[keys]`) but has since
// been removed from the app. `validate_keys` rejects any `[keys]` entry whose
// action is not in `BINDING_DEFS`, so a stale binding left in an existing config
// would abort startup with `[keys] unknown action: "..."`. To let those configs
// keep working, a binding for a retired action is pruned from the document on
// load (and the pruned config is rewritten), exactly like a retired provider
// block. This is the config migration that removes the key for good.
// ---------------------------------------------------------------------------

/// Actions that were removed from dux but may still appear in an older user's
/// `[keys]` section. A binding for any of these is silently dropped on load so
/// the stale key cannot fail `validate_keys`.
const RETIRED_KEY_ACTIONS: &[&str] = &[
    // Removed together with the AI-generated commit-message feature.
    "generate_commit_message",
];

/// Remove `[keys]` entries for retired actions so an old config that still binds
/// them does not fail `validate_keys` with "unknown action". Returns whether the
/// document changed.
fn prune_retired_key_actions(doc: &mut DocumentMut) -> bool {
    let Some(keys) = doc.get_mut("keys").and_then(Item::as_table_mut) else {
        return false;
    };
    let mut changed = false;
    for action in RETIRED_KEY_ACTIONS {
        if keys.remove(action).is_some() {
            changed = true;
        }
    }
    changed
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
    StrList(Vec<String>),
}

/// A comment source.
enum CommentSource {
    Static(&'static str),
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
    /// Renders the `[keys]` section with all keybindings.
    Keys,
    /// Renders the `[macros]` section with text macros.
    Macros,
}

/// KEEP IN SYNC WITH `crates/dux-web/web/src/lib/settingsDescriptors.ts`: the
/// web Preferences dialog, opened from the app menu's cog, renders a
/// first-cut subset of these `[ui]`/`[capabilities]` fields (plus
/// `server.title`/`server.favicon`) with
/// descriptions adapted from this table's `comment` text into readable
/// prose. If you change a field's default, zero-value meaning, or comment
/// here, check whether `settingsDescriptors.ts`'s matching entry needs the
/// same update (and vice versa). There is no codegen linking the two.
fn config_schema() -> Vec<ConfigEntry> {
    vec![
        ConfigEntry::Comment("# dux configuration"),
        ConfigEntry::Comment(
            "# Every value is materialized here so the file doubles as documentation.",
        ),
        ConfigEntry::Blank,
        ConfigEntry::Field {
            key: "shutdown_timeout_seconds",
            comment: Some(CommentSource::Static(
                "# Seconds the TUI waits for running agents and companion terminals to exit\n\
                 # after SIGTERM when you quit, before force-killing (SIGKILL) any that ignore\n\
                 # it. Set to 0 to skip the wait and force-kill immediately; values above 600\n\
                 # are clamped (the unit is SECONDS, not milliseconds). Press Ctrl-c again\n\
                 # during the wait to force an immediate exit.\n\
                 # The web server has its own [server].shutdown_timeout_seconds.",
            )),
            value_fn: |c| FieldValue::U16(c.shutdown_timeout_seconds),
        },
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
                "# Starting directory for the project browser.\n\
                 # The project browser can also initialize a plain folder as a new git repository.\n\
                 # When it does, dux seeds a commented starter .gitignore for common dependency and\n\
                 # build directories it finds (node_modules, target, ...). The candidate list is\n\
                 # built into dux and extended via pull request; it is deliberately not a setting.",
            )),
            value_fn: |c| FieldValue::OptStr(c.defaults.start_directory.clone()),
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
        ConfigEntry::Field {
            key: "copy_uncommitted_changes_by_default",
            comment: Some(CommentSource::Static(
                "# When true, creating an agent copies the project checkout's uncommitted and\n\
                 # untracked changes into the new worktree, when the checkout and the new\n\
                 # worktree are on the same commit. Files matched by .gitignore never travel.\n\
                 # Forks always copy regardless of this setting. The new-agent prompt has a\n\
                 # per-agent checkbox.",
            )),
            value_fn: |c| FieldValue::Bool(c.defaults.copy_uncommitted_changes_by_default),
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
            key: "agent_tabs_max",
            comment: Some(CommentSource::Static(
                "# Maximum number of tabs a single agent may have. All tabs are equal:\n\
                 # each one is a provider session inside the agent's single shared\n\
                 # worktree, and they all edit the same files. A launching tab resumes\n\
                 # its provider's previous conversation only when it is the sole live\n\
                 # tab running that provider; otherwise it starts fresh.\n\
                 # The \"+\" affordance stops adding tabs once an agent reaches this\n\
                 # limit. Clamped to a sane ceiling; 0 falls back to the default (20).\n\
                 # NOTE: this caps how many tabs may EXIST. In server mode a second,\n\
                 # smaller limit caps how many of them may stream at once over the web\n\
                 # UI: [server] max_websocket_tabs_per_agent (default 8). With the\n\
                 # defaults you can create 20 tabs but view at most 8 of one agent's\n\
                 # tabs simultaneously in a browser; raise that one too if you need more.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.agent_tabs_max),
        },
        ConfigEntry::Field {
            key: "status_clear_seconds",
            comment: Some(CommentSource::Static(
                "# Seconds before a transient status message auto-clears.\n# In the TUI status line this applies to success/info confirmations only;\n# busy/pending messages stay until the operation finishes, and warnings and\n# errors stay until replaced.\n# In the web UI every toast dismisses itself, and this is the base window:\n# warnings stay up twice as long and errors four times as long, so this one\n# number grades all of them.\n# Set to 0 to disable auto-clear (messages persist until the next one).",
            )),
            value_fn: |c| FieldValue::U16(c.ui.status_clear_seconds),
        },
        ConfigEntry::Field {
            key: "branch_sync_interval",
            comment: Some(CommentSource::Static(
                "# Seconds between background syncs of git branch names. The key name\n\
                 # omits the unit for backward compatibility, but the value IS in\n\
                 # seconds, like every other interval in this file.\n\
                 # Keeps dux in sync if a branch is renamed outside the app.\n\
                 # Set to 0 to disable.",
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
                "# Enable GitHub PR tracking for agent sessions.\n# Requires the `gh` CLI installed and authenticated (`gh auth login`).\n# When enabled, a PR pill is shown in the agent pane for branches with\n# an open, merged, or closed pull request. Toggle at runtime from the TUI\n# command palette, or the web UI's Preferences dialog.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.github_integration),
        },
        ConfigEntry::Field {
            key: "pr_poll_interval_seconds",
            comment: Some(CommentSource::Static(
                "# Seconds between blind GitHub PR-status safety polls.\n# Most PR updates arrive from events (pushing a branch, or focusing an\n# agent), so this is only the backstop for changes made on GitHub itself.\n# Each cycle is batched into as few GraphQL requests as possible (one per\n# host, up to ~100 PRs each) to stay cheap on your API quota.\n# Set to 0 to disable the blind poll (updates then come only from events).",
            )),
            value_fn: |c| FieldValue::U16(c.ui.pr_poll_interval_seconds),
        },
        ConfigEntry::Field {
            key: "copy_on_select",
            comment: Some(CommentSource::Static(
                "# Web UI only: auto-copy selected terminal text to the clipboard\n# (X11-style \"highlight to copy\"). When enabled, dragging a selection in\n# the browser terminal copies it; Ctrl-Shift-c / Ctrl-Insert (or Cmd-c on a\n# Mac) copy regardless. Change it at runtime from the web UI's Preferences\n# dialog.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.copy_on_select),
        },
        ConfigEntry::Field {
            key: "compose_bar",
            comment: Some(CommentSource::Static(
                "# Web UI only: on a phone, show a compose box below the terminal keys.\n# You type into it with your keyboard's autocorrect and swipe input, then\n# the Send button delivers the whole message and presses Enter for you\n# (Enter inside the box just adds a newline). While enabled, tapping the\n# terminal focuses the compose box so the soft keyboard always types into\n# it. Set to false to hide the box and type directly into the terminal.\n# Change it at runtime from the web UI's Preferences dialog.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.compose_bar),
        },
        ConfigEntry::Field {
            key: "attention_grace_seconds",
            comment: Some(CommentSource::Static(
                "# Seconds the attention indicators stay visible after dux regains your\n# attention, before the focused agent's needs-attention flag clears. Applies\n# when you return to the dux browser tab (web UI) and when your terminal\n# window regains focus (TUI). Gives you time to see which agent(s) wanted you\n# before the indicator vanishes. Set to 0 to clear the indicator immediately.\n# TUI note: requires a terminal that reports focus; under tmux, set\n# `focus-events on`. Terminals that never report focus keep the old behavior.",
            )),
            value_fn: |c| FieldValue::Usize(c.ui.attention_grace_seconds as usize),
        },
        ConfigEntry::Field {
            key: "auto_reopen_agents",
            comment: Some(CommentSource::Static(
                "# Reopen agent PTYs that were still running when dux last exited.\n# Disabled by default. Toggle project-level and agent-level opt-outs from the\n# TUI command palette, or the web UI's project and agent menus.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.auto_reopen_agents),
        },
        ConfigEntry::Field {
            key: "show_changes_pane",
            comment: Some(CommentSource::Static(
                "# Show the Changes pane (the right-hand list of changed files).\n# Set to false to hide it by default; toggle it at runtime from the TUI\n# command palette, or the web UI's Changes actions menu.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.show_changes_pane),
        },
        ConfigEntry::Field {
            key: "always_show_tab_strip",
            comment: Some(CommentSource::Static(
                "# Always show the agent tab strip, even when a session has only one tab.\n# Default false shows it only once a session has two or more tabs.\n# Toggle at runtime from the TUI command palette, or the web UI's\n# Preferences dialog.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.always_show_tab_strip),
        },
        ConfigEntry::Field {
            key: "attention_indicator",
            comment: Some(CommentSource::Static(
                "# Show an indicator when an agent asks for attention (a permission\n# prompt, a finished turn). Detected from the agent's terminal\n# notifications and bell. The TUI blinks a marker in the sidebar; the web\n# UI shows a dot, a browser-tab count, and a favicon dot. Set to false to\n# disable it everywhere.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.attention_indicator),
        },
        ConfigEntry::Field {
            key: "attention_on_bell",
            comment: Some(CommentSource::Static(
                "# Also treat a plain terminal bell as an attention request. The bell is\n# the most compatible signal (Codex falls back to it; Claude Code emits it\n# in terminal_bell mode) but can occasionally ring for mundane reasons, so\n# turn this off if you find it noisy. Has no effect when\n# attention_indicator is false.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.attention_on_bell),
        },
        ConfigEntry::Field {
            key: "disable_automated_welcome_screen",
            comment: Some(CommentSource::Static(
                "# Stop dux from showing the welcome screen by itself.\n# On the very first launch of a fresh install, dux shows a short welcome:\n# what a project is, what an agent is, and the three steps to get going.\n# It appears exactly once, and it needs no network.\n# Set to true to never have it appear on its own. Opening it deliberately\n# still works, because that is you asking rather than dux deciding.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.disable_automated_welcome_screen),
        },
        ConfigEntry::Field {
            key: "disable_release_notes",
            comment: Some(CommentSource::Static(
                "# Stop dux from showing (and fetching) the what's-new screen by itself.\n# When the running version differs from the last one you saw, dux fetches\n# the newest release notes from GitHub in the background and shows a short\n# summary once. Only the newest release is ever shown, however many you\n# skipped. If the fetch fails, nothing is shown and nothing is recorded, so\n# the notes get another chance on your next launch.\n# Set to true and dux makes no network request at startup and shows no\n# what's-new screen on its own. Opening the release notes yourself still works\n# and still fetches them: this setting controls the automatic screen, not what\n# the screen is allowed to show you.",
            )),
            value_fn: |c| FieldValue::Bool(c.ui.disable_release_notes),
        },
        ConfigEntry::Field {
            key: "pr_banner_position",
            comment: Some(CommentSource::Static(
                "# Position of the PR banner in the agent pane: \"top\" or \"bottom\".\n# Toggle at runtime from the TUI command palette, or the web UI's\n# Preferences dialog.",
            )),
            value_fn: |c| FieldValue::Str(c.ui.pr_banner_position.clone()),
        },
        ConfigEntry::Field {
            key: "agent_sort",
            comment: Some(CommentSource::Static(
                "# Agent-list sort mode, persisted across restarts and shared by the TUI\n# and the web. One of:\n#   \"active\"    (default) working / needs-attention agents float to the top\n#   \"updated\"   most recently updated first\n#   \"created\"   most recently created first\n#   \"name\"      by name, A to Z\n#   \"name_desc\" by name, Z to A\n#   \"manual\"    the web's drag-reorder order (the stored global order)\n# The TUI cycles the five non-manual modes via the \"sort-agents\" palette\n# command; the web sets it from its sidebar sort control, where a drag\n# switches it to \"manual\" automatically. Each surface offers its own subset\n# but displays whatever value the other set.",
            )),
            value_fn: |c| FieldValue::Str(c.ui.agent_sort.clone()),
        },
        ConfigEntry::Field {
            key: "theme",
            comment: Some(CommentSource::Static(
                "# Visual theme for the dux interface.\n# Built-in options include \"dux_dark\" (the default), plus any theme\n# bundled with the opaline engine, for example: \"catppuccin_mocha\",\n# \"catppuccin_frappe\", \"nord\", \"dracula\", \"gruvbox_dark\",\n# \"tokyo_night\", \"solarized_dark\", \"one_dark\", \"rose_pine\", and others.\n# To use a custom theme, drop a TOML file into <config_dir>/themes/<name>.toml\n# (with the same token format as opaline themes) and reference it here\n# by file stem. Unknown names fall back to dux_dark with a warning.\n# Use the `change-theme` command in the palette (Ctrl-p) for an interactive picker.",
            )),
            value_fn: |c| FieldValue::Str(c.ui.theme.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("capabilities"),
        ConfigEntry::Comment(
            "# Terminal capability controls: the identity dux presents to an agent,\n\
             # and which escape sequences the agent emits are forwarded onward.\n\
             # Agents pick their notification channel from the terminal they detect,\n\
             # so presenting a real identity is what makes desktop notifications work.",
        ),
        ConfigEntry::Field {
            key: "terminal_identity",
            comment: Some(CommentSource::Static(
                "# What terminal dux pretends to be when it launches an agent:\n\
                 #   \"auto\"    mirror your real terminal in the TUI (seeing through tmux),\n\
                 #             and present ghostty on the headless web server. The default.\n\
                 #   \"mirror\"  always mirror the real host terminal, seeing through tmux.\n\
                 #   \"ghostty\" / \"iterm2\"  force that identity (works well with the web UI).\n\
                 #   \"kitty\"   force kitty; this also sets TERM=xterm-kitty, which needs the\n\
                 #             kitty terminfo entry present or some programs may misrender.\n\
                 #   \"none\"    change nothing; the agent inherits dux's environment as-is.\n\
                 # Under tmux, \"auto\"/\"mirror\" strip TMUX so agents emit unwrapped\n\
                 # sequences that dux re-wraps itself. For forwarded notifications to reach\n\
                 # your terminal, tmux needs `set -g allow-passthrough on`.",
            )),
            value_fn: |c| FieldValue::Str(c.capabilities.terminal_identity.clone()),
        },
        ConfigEntry::Field {
            key: "passthrough",
            comment: Some(CommentSource::Static(
                "# TUI ONLY: forward the agent's notification, progress, and clipboard\n\
                 # escape sequences to your host terminal. The master switch for the TUI's\n\
                 # outbound passthrough; set false to keep everything the agent emits\n\
                 # inside dux. It does not affect the web UI, which uses web_notifications\n\
                 # and clipboard_passthrough below instead.",
            )),
            value_fn: |c| FieldValue::Bool(c.capabilities.passthrough),
        },
        ConfigEntry::Field {
            key: "clipboard_passthrough",
            comment: Some(CommentSource::Static(
                "# Whose OSC 52 clipboard writes reach the clipboard, on BOTH surfaces:\n\
                 #   \"focused\"  only the agent tab you are currently viewing (the default),\n\
                 #   \"always\"   any agent, even one running in the background,\n\
                 #   \"off\"      never.\n\
                 # Clipboard READ requests are never forwarded (a reply would be typed\n\
                 # back into dux). On the TUI this also requires passthrough = true; on the\n\
                 # web it gates the browser clipboard write directly (which the browser\n\
                 # additionally only permits while the tab has focus).",
            )),
            value_fn: |c| FieldValue::Str(c.capabilities.clipboard_passthrough.clone()),
        },
        ConfigEntry::Field {
            key: "hyperlinks",
            comment: Some(CommentSource::Static(
                "# Render OSC 8 hyperlinks as clickable, on both surfaces: in the TUI\n\
                 # (when your host terminal supports them) and in the web terminal\n\
                 # (http/https only). Set false to render links as plain, inert text.",
            )),
            value_fn: |c| FieldValue::Bool(c.capabilities.hyperlinks),
        },
        ConfigEntry::Field {
            key: "web_notifications",
            comment: Some(CommentSource::Static(
                "# WEB ONLY: bridge an agent's notification sequences to a browser desktop\n\
                 # notification. Fires only when the tab is in the background and only after\n\
                 # you grant permission from the web UI (dux never auto-prompts). No effect\n\
                 # on the TUI, whose host-terminal notifications are governed by passthrough.",
            )),
            value_fn: |c| FieldValue::Bool(c.capabilities.web_notifications),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("editor"),
        ConfigEntry::Field {
            key: "default",
            comment: Some(CommentSource::Static(
                "# Preferred editor for \"open in editor\": the TUI's open-worktree action\n# and the web code editor's \"Open editor\" menu (the web menu lets you pick per\n# open and is only enabled for local-access URLs; this is its fallback). Supported\n# values are matched against popular editor CLIs on PATH (for example: cursor,\n# vscode/code, zed, vscodium, sublime).",
            )),
            value_fn: |c| FieldValue::Str(c.editor.default.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("server"),
        ConfigEntry::Comment(
            "# The dux web UI is a trusted-local tool: there is no login gate. It binds\n\
             # host:port (loopback by default) and, when tailscale_enabled, also this\n\
             # machine's Tailscale address so your other tailnet devices can reach it\n\
             # (traffic is WireGuard-encrypted in transit). The in-app \"start web\n\
             # server\" flip always serves on loopback (plus Tailscale) regardless of\n\
             # host. Only run a non-loopback host on a network you trust.\n\
             #\n\
             # Three settings below decide where dux listens and who it answers.\n\
             # They do NOT override each other; they stack, and they are checked in\n\
             # this order:\n\
             #   1. host + port     — the one address dux binds. `dux server --bind\n\
             #                        IP:port` overrides both for that run.\n\
             #   2. tailscale_enabled — binds an ADDITIONAL address (this machine's\n\
             #                        Tailscale IP, same port). Never replaces host;\n\
             #                        best-effort, so a failure only warns.\n\
             #   3. allowed_hosts   — not an address at all. Once a request arrives\n\
             #                        at one of the addresses above, this is the\n\
             #                        guard on its Host header.\n\
             # So: binding is (1) plus optionally (2); (3) only ever rejects.",
        ),
        ConfigEntry::Field {
            key: "host",
            comment: Some(CommentSource::Static(
                "# Bind host for `dux server`. Must be an IP literal, not a hostname:\n\
                 #   \"127.0.0.1\": loopback only (the safe default; only this machine).\n\
                 #   \"0.0.0.0\":   every interface (reachable from the network).\n\
                 # The in-app flip ignores this and always binds loopback (+ Tailscale).\n\
                 # Override per run with `dux server --bind IP:port`.",
            )),
            value_fn: |c| FieldValue::Str(c.server.host.clone()),
        },
        ConfigEntry::Field {
            key: "port",
            comment: Some(CommentSource::Static(
                "# Bind port. dux binds host:port (and the Tailscale address:port when\n\
                 # tailscale_enabled). The default is 8080.",
            )),
            value_fn: |c| FieldValue::U16(c.server.port),
        },
        ConfigEntry::Field {
            key: "tailscale_enabled",
            comment: Some(CommentSource::Static(
                "# Opt-out Tailscale binding for LOCAL MODE. When true (the default),\n\
                 # dux detects this machine's Tailscale address (via the `tailscale ip`\n\
                 # CLI) and also listens there, so tailnet devices can open the web UI.\n\
                 # If the CLI is missing or the daemon is down, dux WARNS and serves on\n\
                 # loopback only — it never blocks. Likewise, if the Tailscale port is\n\
                 # already in use by another process, dux WARNS and serves on loopback\n\
                 # only rather than failing to start. Set false to skip detection and\n\
                 # silence that warning.\n\
                 # NOTE: a shared tailnet means OTHER people's devices can reach dux.",
            )),
            value_fn: |c| FieldValue::Bool(c.server.tailscale_enabled),
        },
        ConfigEntry::Field {
            key: "allowed_hosts",
            comment: Some(CommentSource::Static(
                "# Extra Host header values to accept on NON-same-origin requests. dux\n\
                 # always serves on host:port and accepts same-origin requests; list any\n\
                 # additional hostnames a reverse proxy or tailnet name forwards under\n\
                 # so the host guard does not reject them. Hostnames only, no scheme or\n\
                 # port. Examples:\n\
                 #   allowed_hosts = [\"box.tailnet.ts.net\"]\n\
                 #   allowed_hosts = [\"dux.example.com\"]\n\
                 # Leave empty for a plain loopback or proxy-fronted deployment.",
            )),
            value_fn: |c| FieldValue::StrList(c.server.allowed_hosts.clone()),
        },
        ConfigEntry::Field {
            key: "color",
            comment: Some(CommentSource::Static(
                "# Colored, vite-style console output for `dux server`. One of:\n\
                 #   \"auto\"   — color only when stdout is a real terminal, NO_COLOR is\n\
                 #              unset/empty, and TERM is not \"dumb\" (piped output stays\n\
                 #              plain ASCII, so logs and `| tee` capture cleanly).\n\
                 #   \"always\" — force color even when piped.\n\
                 #   \"never\"  — plain text always.\n\
                 # An unrecognized value falls back to \"auto\" with a warning. The in-app\n\
                 # \"start web server\" flip keeps its themed status screen — this only\n\
                 # affects the `dux server` CLI.",
            )),
            value_fn: |c| FieldValue::Str(c.server.color.clone()),
        },
        ConfigEntry::Field {
            key: "access_log",
            comment: Some(CommentSource::Static(
                "# Print a per-request access log line (method, path, status, latency) to\n\
                 # the `dux server` console. The /healthz probe is always skipped so a\n\
                 # health checker does not flood the log. This output is console-ONLY and\n\
                 # never written to dux.log, so piping `dux server`'s stdout captures the\n\
                 # access log. Set false to silence it.",
            )),
            value_fn: |c| FieldValue::Bool(c.server.access_log),
        },
        ConfigEntry::Field {
            key: "max_websocket_events_connections",
            comment: Some(CommentSource::Static(
                "# Maximum number of concurrent EVENTS WebSocket (/ws) connections to\n\
                 # `dux server`. This is the status/changed-files event stream every open\n\
                 # browser tab holds. Once this many are live, further connections are\n\
                 # refused with HTTP 503 until a slot frees: a safety bound against\n\
                 # connection exhaustion (a runaway reconnect loop, a tab left\n\
                 # multiplying). The normal single-operator deployment uses a handful;\n\
                 # raise it if you genuinely run many tabs/devices. A value of 0\n\
                 # PERMANENTLY blocks this connection class until the server restarts.\n\
                 # Changing this needs a server restart to take effect (a reload of the\n\
                 # running server cannot resize the cap).",
            )),
            value_fn: |c| FieldValue::Usize(c.server.max_websocket_events_connections as usize),
        },
        ConfigEntry::Field {
            key: "max_websocket_agent_connections",
            comment: Some(CommentSource::Static(
                "# Maximum number of concurrent AGENT-PTY WebSocket connections to\n\
                 # `dux server`. This is the embedded-terminal stream for an agent\n\
                 # session. Once this many are live, further connections are refused with\n\
                 # HTTP 503 until a slot frees. A value of 0 PERMANENTLY blocks this\n\
                 # connection class until the server restarts. Changing this needs a\n\
                 # server restart to take effect (a reload of the running server cannot\n\
                 # resize the cap).",
            )),
            value_fn: |c| FieldValue::Usize(c.server.max_websocket_agent_connections as usize),
        },
        ConfigEntry::Field {
            key: "max_websocket_terminal_connections",
            comment: Some(CommentSource::Static(
                "# Maximum number of concurrent TERMINAL-PTY WebSocket connections to\n\
                 # `dux server`. This is the standalone scratch-terminal stream. Once\n\
                 # this many are live, further connections are refused with HTTP 503\n\
                 # until a slot frees. A value of 0 PERMANENTLY blocks this connection\n\
                 # class until the server restarts. Changing this needs a server restart\n\
                 # to take effect (a reload of the running server cannot resize the cap).",
            )),
            value_fn: |c| FieldValue::Usize(c.server.max_websocket_terminal_connections as usize),
        },
        ConfigEntry::Field {
            key: "max_websocket_tab_connections",
            comment: Some(CommentSource::Static(
                "# Maximum number of concurrent extra-tab PTY WebSocket connections\n\
                 # across ALL agents. Tab streams draw from THIS pool, not the agent-PTY\n\
                 # pool, so a few agents each showing many tabs cannot 503 every other\n\
                 # agent's primary terminal. Once this many are live, further tab streams\n\
                 # are refused with HTTP 503 until a slot frees. A value of 0 PERMANENTLY\n\
                 # blocks all tab streams until the server restarts. Changing this needs a\n\
                 # server restart to take effect.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.max_websocket_tab_connections as usize),
        },
        ConfigEntry::Field {
            key: "max_websocket_tabs_per_agent",
            comment: Some(CommentSource::Static(
                "# Maximum concurrent live extra-tab PTY streams a SINGLE agent may\n\
                 # hold, checked BEFORE a permit is taken from the shared tab pool above.\n\
                 # A per-agent fairness sub-quota so one agent showing many tabs cannot\n\
                 # monopolize that pool and starve other agents' tabs. Once an agent hits\n\
                 # this many live tab streams, further ones for THAT agent are refused with\n\
                 # HTTP 503 until one closes. A value of 0 PERMANENTLY blocks all tab\n\
                 # streams until the server restarts.\n\
                 # This is a CONCURRENT-VIEWERS cap, not a limit on how many tabs an\n\
                 # agent may have — that is [ui] agent_tabs_max (default 20). The two\n\
                 # differ on purpose: creating a tab is cheap, streaming one is not.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.max_websocket_tabs_per_agent as usize),
        },
        ConfigEntry::Field {
            key: "title",
            comment: Some(CommentSource::Static(
                "# Display name for THIS dux instance in the web UI. It is shown as\n\
                 # the browser tab title and as the brand wordmark at the top of the\n\
                 # projects pane (the version stays on the line below). Give each\n\
                 # instance a distinct value (for example \"dux #1\" or \"dux (prod)\")\n\
                 # so several dux tabs/servers are easy to tell apart. An empty or\n\
                 # whitespace-only value falls back to \"dux\".",
            )),
            value_fn: |c| FieldValue::Str(c.server.title.clone()),
        },
        ConfigEntry::Field {
            key: "favicon",
            comment: Some(CommentSource::Static(
                "# Favicon color for THIS dux instance, so several dux tabs are easy to\n\
                 # tell apart. Empty (the default) keeps the original full-color yellow\n\
                 # duck. Otherwise one of the curated tint colors, which recolors a flat\n\
                 # duck silhouette in the browser tab: violet, blue, sky, cyan, teal,\n\
                 # green, amber, orange, red, pink, rose.\n\
                 # An unrecognized value falls back to the default duck.",
            )),
            value_fn: |c| FieldValue::Str(c.server.favicon.clone()),
        },
        ConfigEntry::Field {
            key: "shutdown_timeout_seconds",
            comment: Some(CommentSource::Static(
                "# Seconds the web server (dux server, or a server flipped from the TUI)\n\
                 # waits for agents and companion terminals to exit after SIGTERM on\n\
                 # shutdown, before force-killing (SIGKILL) any stragglers. Set to 0 to\n\
                 # skip the wait and force-kill immediately; values above 600 are clamped.\n\
                 # A second Ctrl-c/SIGTERM during the wait forces an immediate exit.\n\
                 # The TUI quit path uses the top-level shutdown_timeout_seconds instead.",
            )),
            value_fn: |c| FieldValue::U16(c.server.shutdown_timeout_seconds),
        },
        ConfigEntry::Field {
            key: "search_index_max_files",
            comment: Some(CommentSource::Static(
                "# Maximum number of files the web editor's \"Search files...\" index will\n\
                 # collect in a single flat walk of the worktree. The file tree is a lazy,\n\
                 # per-directory browser and is never capped; this bounds only the search\n\
                 # index, where an incomplete result on a very large repo (for example a\n\
                 # built target/ directory) is an acceptable tradeoff for a bounded\n\
                 # response. Set to 0 to disable the cap entirely.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.search_index_max_files),
        },
        ConfigEntry::Field {
            key: "tree_list_max_concurrency",
            comment: Some(CommentSource::Static(
                "# Maximum number of /files/tree directory listings the web editor may run\n\
                 # concurrently across all sessions. Each listing does one blocking read_dir\n\
                 # off the server's async reactor; this protects the server's blocking-thread\n\
                 # pool from a burst of tree requests (for example several tabs expanding\n\
                 # directories at once) starving other blocking work such as git operations\n\
                 # and file reads/writes. A request beyond the limit waits for a free slot\n\
                 # rather than being refused. Set to 0 to disable the bound entirely.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.tree_list_max_concurrency as usize),
        },
        ConfigEntry::Field {
            key: "release_notes_max_concurrency",
            comment: Some(CommentSource::Static(
                "# Maximum number of release-notes fetches the web server may run at once.\n\
                 # The app menu's \"What's new...\" entry asks GitHub for this version's\n\
                 # release notes, and that is a blocking HTTPS round trip run off the\n\
                 # server's async reactor, so a burst of clicks (or several browser tabs\n\
                 # asking at the same time) could otherwise tie up the blocking-thread pool\n\
                 # that git operations and file reads also use. A request beyond the limit\n\
                 # waits for a free slot rather than being refused, and because the notes\n\
                 # are cached for six hours the waiter usually answers straight from cache.\n\
                 # Small on purpose: every caller gets the same answer. Set to 0 to disable\n\
                 # the bound entirely.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.release_notes_max_concurrency as usize),
        },
        ConfigEntry::Field {
            key: "file_drop_max_bytes",
            comment: Some(CommentSource::Static(
                "# Largest single file you can drop onto a terminal or agent pane in the web\n\
                 # UI, in bytes. Default 104857600, which is 100 MiB. dux sets this limit\n\
                 # explicitly because the web framework's own default is 2 MB, small enough\n\
                 # to reject an ordinary screenshot from a high-resolution display. A file\n\
                 # over the limit is refused with a message saying so, and nothing is\n\
                 # written. Set to 0 to switch file drop off entirely: uploads are refused\n\
                 # and the browser stops offering the drop target. Read at startup, so a\n\
                 # change needs a server restart.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.file_drop_max_bytes),
        },
        ConfigEntry::Field {
            key: "file_drop_max_concurrency",
            comment: Some(CommentSource::Static(
                "# How many dropped-file uploads the web server will accept at the same\n\
                 # time. The slot is taken before the upload's body is read, which is what\n\
                 # makes this bound how much upload dux holds in memory at once rather than\n\
                 # merely queueing the work: a request body is buffered in full before the\n\
                 # code handling it starts. With the default size limit above, the worst\n\
                 # case is roughly 200 MiB. An upload beyond the limit waits for a free slot\n\
                 # rather than being refused. Set to 0 and it clamps to 1, since no slots at\n\
                 # all would stall every drop forever; use file_drop_max_bytes = 0 to switch\n\
                 # the feature off. Read at startup, so a change needs a server restart.",
            )),
            value_fn: |c| FieldValue::Usize(c.server.file_drop_max_concurrency as usize),
        },
        ConfigEntry::Blank,
        ConfigEntry::Keys,
        ConfigEntry::Blank,
        ConfigEntry::Macros,
    ]
}

fn render_config(config: &Config, bindings: &crate::keybindings::RuntimeBindings) -> String {
    let mut out = String::new();
    for entry in config_schema() {
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
                    FieldValue::StrList(list) => {
                        let _ = writeln!(out, "{key} = {}", render_string_list(&list));
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

/// Render a config through the canonical commented renderer, deriving the
/// keybinding labels from the config's own `[keys]` so the documented bindings
/// match what the file actually binds.
///
/// This is the function handed to `dux_core::config_write::set_canonical_renderer`,
/// which is how `dux serve` — a surface with no access to `RuntimeBindings` —
/// still creates a fully-commented config on first run.
pub fn render_config_documented(config: &Config) -> String {
    let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
    render_config(config, &bindings)
}

/// Install [`render_config_documented`] as the process-wide canonical renderer.
///
/// Call this before any code path that can CREATE `config.toml`. Both entry
/// points do: the TUI through `ensure_config`, and `dux server` through its
/// bootstrap project-sync. Idempotent.
pub fn install_canonical_renderer() {
    dux_core::config_write::set_canonical_renderer(render_config_documented);
}

// ---------------------------------------------------------------------------
// dux config restore-docs
// ---------------------------------------------------------------------------

/// The result of re-applying the commented template to an existing config.
#[derive(Debug)]
pub struct RestoredConfig {
    /// The full text of the restored file. Not yet written anywhere.
    pub text: String,
    /// Orphaned sections that were removed, as dotted paths.
    pub dropped: Vec<String>,
    /// Unknown keys carried over verbatim, as dotted paths.
    pub preserved: Vec<String>,
    /// Unknown keys the merge could not place anywhere, as dotted paths. These
    /// are absent from `text`; naming them is what keeps the loss from being
    /// silent. Empty for every config the canonical renderer can produce.
    pub unplaceable: Vec<String>,
}

impl RestoredConfig {
    /// Whether restoring would leave the file byte-identical.
    pub fn is_noop(&self, original_raw: &str) -> bool {
        self.text == original_raw
    }
}

/// Re-apply the fully-commented canonical template to an EXISTING config's raw
/// text, keeping every value the file carries.
///
/// This is the read-only half of `dux config restore-docs`: it returns the text
/// to write and what changed, and never touches the filesystem, so the CLI can
/// preview it and the tests can exercise it without a temp directory.
///
/// # Safety contract
///
/// * An unparseable file is REFUSED with an error naming the parse failure. It
///   deliberately does not fall through to a defaults-based regeneration —
///   silently replacing a broken file with defaults is exactly the data loss
///   this feature exists to prevent.
/// * Unknown keys are preserved verbatim unless they sit under
///   [`dux_core::config_write::ORPHANED_CONFIG_SECTIONS`], which are reported as
///   dropped.
/// * The result is re-parsed and compared against the config parsed from the
///   input. A mismatch aborts with an error rather than writing, so a renderer
///   bug can never silently rewrite a user's settings.
pub fn restore_documentation(raw: &str) -> Result<RestoredConfig> {
    let original: DocumentMut = raw
        .parse()
        .context("config.toml is not valid TOML, so its values cannot be read back safely")?;

    let config: Config = toml::from_str(raw)
        .context("config.toml parses as TOML but not as a dux config, so its values cannot be read back safely")?;

    let rendered_text = render_config_documented(&config);
    let mut rendered: DocumentMut = rendered_text
        .parse()
        .context("the canonical config template did not render valid TOML (this is a dux bug)")?;

    let report = dux_core::config_write::merge_unmanaged_keys(&mut rendered, &original);
    let text = rendered.to_string();

    // Self-check: the restore must be a FIXED POINT of the renderer. Rendering
    // the config we just wrote has to reproduce the very same text, which can
    // only happen if every value survived the round trip.
    //
    // This is deliberately not a raw `reparsed == config` equality check. The
    // canonical template MATERIALIZES defaults that a bare file leaves implicit
    // — most visibly `[keys]`, which gains every default binding (that is the
    // point: a keys section with no keys never tells the user rebinding is
    // possible). Struct equality would flag those as changes and refuse every
    // real restore. The fixed-point check tolerates materialized defaults while
    // still catching an actual altered or lost value, because a changed value
    // renders differently.
    let reparsed: Config = toml::from_str(&text)
        .context("the restored config did not parse back as a dux config; refusing to write it")?;
    if render_config_documented(&reparsed) != rendered_text {
        anyhow::bail!(
            "restoring the documentation would have changed a setting's value; refusing to write. \
             This is a dux bug — please report it, and note that your config.toml has not been modified."
        );
    }

    Ok(RestoredConfig {
        text,
        dropped: report.dropped,
        preserved: report.preserved,
        unplaceable: report.unplaceable,
    })
}

/// Persist the in-memory `Config` to disk using surgical edits via `toml_edit`.
///
/// If the config file already exists, it is parsed as a TOML document and only
/// the keys that differ from the on-disk version are updated.  User comments,
/// formatting, and unknown keys are preserved.  If the file does not yet exist,
/// a fresh canonical config is rendered instead.
///
/// This wrapper is also deprecated: callers that used the TUI `save_config`
/// bypassed the `ConfigWriteQueue` gate. All runtime writes must route through
/// the queue; the only legitimate callers of this wrapper are the TUI bootstrap
/// helpers (`persist_runtime_projects_to_config_and_store`,
/// `sync_config_projects_with_store`) which are sync-direct by design.
#[deprecated(
    note = "route config writes through ConfigWriteQueue; sync-direct callers must #[allow(deprecated)]"
)]
#[allow(deprecated)] // internal delegation: body calls deprecated core fns (patch_config_file, write_config_secure)
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
        // 0600 perms: this file may hold secrets such as [env] tokens, so it must
        // not be group/world readable (shared with the config writer's patch path
        // so first-creation and later saves agree).
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
         # Values may reference existing environment variables with $VAR or ${VAR}.\n\
         #\n\
         # `id` is generated by dux and is how a project is matched to its agents and\n\
         # worktrees in the database. It must be UNIQUE. If you add a project by hand,\n\
         # do NOT copy an existing block's id: give it a fresh UUID, or delete the id\n\
         # line and let dux generate one on next start. Two projects sharing an id is\n\
         # an identity conflict and dux will refuse to start until you fix it.\n",
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
    if let Some(hint) = &config.install_hint {
        out.push_str("# Hint shown to the user when the provider command is not found on PATH.\n");
        out.push_str(&format!(
            "install_hint = \"{}\"\n",
            escape_toml_string(hint)
        ));
    }
    out.push_str(
        "# Controls whether the mouse wheel and PgUp/PgDn scroll dux's own host\n\
         # scrollback or get forwarded to the provider. Tri-state:\n\
         #   (unset) = auto: forward to the child only when it owns the screen and\n\
         #             wants the wheel (a fullscreen alt-screen, mouse-aware app like\n\
         #             an agent's renderer); otherwise scroll dux host scrollback.\n\
         #   true    = always forward scroll + page keys to the child.\n\
         #   false   = never forward; always use dux host scrollback.\n\
         # Leave this key absent for auto. Uncomment to pin a value.\n",
    );
    match config.forward_scroll {
        Some(value) => out.push_str(&format!("forward_scroll = {value}\n")),
        None => out.push_str("# forward_scroll = true\n"),
    }
    out.push_str(
        "# What a DRAGGED AND DROPPED file's path looks like when the web UI writes\n\
         # it into this provider's prompt. This affects the browser and nothing else,\n\
         # which is what the \"web_\" prefix is here to say: in the terminal UI,\n\
         # dropping a file onto the window is your terminal emulator's job and dux is\n\
         # not involved at all.\n\
         # Four forms:\n\
         #   bare              = the path exactly as it is on disk. Nothing added,\n\
         #                       nothing escaped.\n\
         #   single_quoted     = wrapped in 'single quotes', with an apostrophe\n\
         #                       inside it escaped the POSIX way.\n\
         #   double_quoted     = wrapped in \"double quotes\", with a double quote\n\
         #                       or backslash inside it escaped.\n\
         #   backslash_escaped = no quotes, with shell-significant characters\n\
         #                       escaped one by one, so My File.png goes out as\n\
         #                       My\\ File.png. This is what several terminal\n\
         #                       emulators emit when you drop a file on them.\n\
         # Which one is right depends on how this CLI reads a pasted path, and the\n\
         # CLIs genuinely differ. Some take the whole pasted string and only strip\n\
         # quotes off it, so quoting buys nothing and actively corrupts a path\n\
         # containing an apostrophe. Others lex the text with shell rules and accept\n\
         # it only if it comes out as a single word, so a path with a space in it is\n\
         # quietly ignored unless it is quoted or escaped. The values dux ships were\n\
         # measured against each CLI rather than guessed.\n\
         # Two combinations are known to fail, and neither is fixable from dux's\n\
         # side: single-quoting a path that contains an apostrophe breaks Claude\n\
         # Code, and any form carrying a backslash is mangled by the same CLI's own\n\
         # unescaping step.\n\
         # Getting it wrong is not usually a breakage: the normal symptom is that the\n\
         # file is not attached automatically and its path is left sitting in the\n\
         # prompt as ordinary text, which you can still work with.\n\
         # An absent key, or a value dux does not recognize, means bare.\n",
    );
    match &config.web_dragdrop_paste {
        Some(value) => out.push_str(&format!(
            "web_dragdrop_paste = \"{}\"\n",
            escape_toml_string(value)
        )),
        None => out.push_str("# web_dragdrop_paste = \"bare\"\n"),
    }
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
#[allow(deprecated)] // tests call the deprecated save_config wrapper directly to verify its behaviour
mod tests {
    use indexmap::IndexMap;

    use super::*;

    /// Render config using default keybinding labels (for tests that don't need custom bindings).
    fn render_config_default(config: &Config) -> String {
        let bindings =
            crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default());
        render_config(config, &bindings)
    }

    // -----------------------------------------------------------------------
    // dux config restore-docs
    //
    // The fixture is a COPY of a real user's config.toml (209 lines, 64
    // settings, zero comments) taken from a cold review. It is the exact shape
    // this feature exists for: a file born through the plain writer, which the
    // comment-preserving patch path then carried forward forever without ever
    // adding the documentation back.
    // -----------------------------------------------------------------------

    fn bare_user_config() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bare_user_config.toml"
        );
        std::fs::read_to_string(path).expect("read bare user config fixture")
    }

    #[test]
    fn the_fixture_really_is_undocumented() {
        // If this ever fails, the fixture stopped being the thing under test and
        // every assertion below is measuring nothing.
        let raw = bare_user_config();
        assert!(
            !raw.contains('#'),
            "fixture must contain zero comments, or it is not a bare config"
        );
    }

    #[test]
    fn restore_docs_adds_comments_to_a_bare_config() {
        let raw = bare_user_config();
        let restored = restore_documentation(&raw).expect("restore");

        assert!(
            restored.text.contains('#'),
            "restored config gained no comments"
        );
        // Not just a stray comment: the file must actually be documented. The
        // canonical renderer emits a comment for essentially every setting.
        let comment_lines = restored
            .text
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count();
        assert!(
            comment_lines > 100,
            "expected a thoroughly documented file, got {comment_lines} comment lines"
        );
    }

    #[test]
    fn restore_docs_keeps_every_value_from_a_bare_config() {
        let raw = bare_user_config();
        let before: Config = toml::from_str(&raw).expect("fixture parses");
        let restored = restore_documentation(&raw).expect("restore");
        let after: Config = toml::from_str(&restored.text).expect("restored parses");

        // The whole settings tree, compared as one value, EXCEPT the two places
        // the canonical template deliberately materializes an implicit default.
        // Normalizing those two and then comparing everything else is a much
        // stronger check than a handful of spot assertions.
        let mut normalized = after.clone();
        normalized.keys.bindings = before.keys.bindings.clone();
        for (name, provider) in normalized.providers.commands.iter_mut() {
            if before.providers.commands[name]
                .resume_wait_timeout_ms
                .is_none()
                && provider.resume_wait_timeout_ms == Some(0)
            {
                provider.resume_wait_timeout_ms = None;
            }
        }
        assert_eq!(normalized, before, "restore changed a setting");

        // The two materialized defaults must be semantically neutral.
        // 1. `[keys]` gains every default binding: an absent binding already
        //    MEANS the default, so writing it changes nothing about behaviour —
        //    it just makes the file say that rebinding is possible.
        let default_bindings = render_config_default(&Config::default());
        for action in ["quit", "new_agent", "open_palette"] {
            assert!(
                after.keys.bindings.contains_key(action),
                "[keys] should have been materialized with {action}"
            );
            assert!(
                default_bindings.contains(action),
                "sanity: {action} is a real default binding"
            );
        }
        // 2. `resume_wait_timeout_ms` is written as 0 where it was absent, and
        //    the engine treats None and 0 identically (both disable the hung
        //    -resume window).
        assert_eq!(
            after.providers.commands["claude"].resume_wait_timeout_ms,
            Some(0)
        );
        assert_eq!(
            after.providers.commands["opencode"].resume_wait_timeout_ms,
            Some(3000),
            "a real, non-default timeout must be carried through unchanged"
        );

        // And spelled out for the categories the brief calls out as real user
        // data, so a failure says WHICH kind of data was lost.
        assert_eq!(after.projects.len(), 6);
        assert_eq!(
            after.projects[0].id, "f4f758b6-daf9-4116-bc55-025fddbe1822",
            "a project's generated identifier must survive"
        );
        assert_eq!(after.projects[0].name.as_deref(), Some("dux"));

        // Macros, including a multi-line body.
        assert_eq!(after.macros.entries.len(), 8);
        let multiline = after
            .macros
            .entries
            .get("Create a Pull Request")
            .expect("multi-line macro survives");
        assert!(
            multiline.text.contains('\n'),
            "the macro body lost its newlines"
        );
        assert!(
            multiline
                .text
                .contains("Example line 2 of the placeholder body"),
            "a later line of the multi-line body was lost: {:?}",
            multiline.text
        );

        // Providers with their argument lists, including a user-added provider
        // that is not one of dux's defaults.
        let claude = after.providers.commands.get("claude").expect("claude");
        assert_eq!(
            claude.resume_args.as_deref(),
            Some(&["--continue".to_string()][..])
        );
        assert!(
            after.providers.commands.contains_key("cline"),
            "a user-added provider must survive"
        );

        // Free-form values that are easy to mangle when re-rendering.
        assert_eq!(after.server.title, "dux @ workstation");
        assert_eq!(
            after.defaults.start_directory.as_deref(),
            Some("/home/user/code")
        );
    }

    #[test]
    fn restore_docs_drops_orphaned_sections_and_reports_them() {
        let raw = bare_user_config();
        assert!(raw.contains("[auth]"), "fixture precondition");
        assert!(raw.contains("[server.acme]"), "fixture precondition");

        let restored = restore_documentation(&raw).expect("restore");

        assert!(
            !restored.text.contains("[auth]"),
            "orphaned [auth] survived"
        );
        assert!(
            !restored.text.contains("acme"),
            "orphaned [server.acme] survived"
        );
        // Reported, not silent.
        assert_eq!(restored.dropped, vec!["auth", "server.acme"]);
    }

    #[test]
    fn restore_docs_preserves_unknown_keys_that_are_not_on_the_drop_list() {
        let raw = bare_user_config();
        // The fixture carries three retired [server] keys that are NOT on the
        // drop list. They must be carried over, not quietly deleted.
        for key in [
            "listen_addrs",
            "insecure_allow_remote",
            "dangerously_listen_http",
        ] {
            assert!(raw.contains(key), "fixture precondition: {key}");
        }

        let restored = restore_documentation(&raw).expect("restore");

        for key in [
            "listen_addrs",
            "insecure_allow_remote",
            "dangerously_listen_http",
        ] {
            assert!(
                restored.text.contains(key),
                "unknown key {key} was dropped:\n{}",
                restored.text
            );
        }
        assert!(
            restored
                .preserved
                .contains(&"server.listen_addrs".to_string()),
            "preserved list: {:?}",
            restored.preserved
        );
    }

    #[test]
    fn restore_docs_refuses_an_unparseable_config() {
        let broken = "[server]\nport = = 8080\n[[[nope\n";
        let err = restore_documentation(broken).expect_err("must refuse");
        let message = format!("{err:#}");
        assert!(
            message.contains("not valid TOML"),
            "the refusal must say why: {message}"
        );
    }

    #[test]
    fn restore_docs_refuses_a_config_that_is_valid_toml_but_not_a_dux_config() {
        // Valid TOML, wrong types. Regenerating from defaults here would wipe
        // the user's real settings, so this must refuse too.
        let wrong = "[server]\nport = \"not a number\"\n";
        let err = restore_documentation(wrong).expect_err("must refuse");
        let message = format!("{err:#}");
        assert!(
            message.contains("not as a dux config"),
            "the refusal must say why: {message}"
        );
    }

    #[test]
    fn restore_docs_is_a_noop_on_an_already_canonical_config() {
        // The existing behaviour must not regress: a file the canonical renderer
        // produced is already fully documented, so there is nothing to restore.
        let raw = render_config_default(&Config::default());
        let restored = restore_documentation(&raw).expect("restore");
        assert!(
            restored.is_noop(&raw),
            "a canonical config should restore to itself"
        );
        assert!(restored.dropped.is_empty());
        assert!(restored.preserved.is_empty());
    }

    #[test]
    fn restore_docs_is_idempotent() {
        let raw = bare_user_config();
        let once = restore_documentation(&raw).expect("first restore");
        let twice = restore_documentation(&once.text).expect("second restore");
        assert_eq!(
            twice.text, once.text,
            "restoring twice must not keep changing the file"
        );
        assert!(
            twice.dropped.is_empty(),
            "the orphans were already dropped the first time"
        );
    }

    #[test]
    fn a_config_created_through_the_core_writer_is_born_documented() {
        // The regression this pins: `save_config_with` is the path the WEB uses
        // (`dux serve` bootstrap project-sync). It used to write a bare document
        // when the file was missing, and because the later patch path preserves
        // comments but never ADDS them, such a config stayed bare forever.
        install_canonical_renderer();

        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let mut config = Config::default();
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

        dux_core::config_write::save_config_with(
            &config_path,
            &config,
            dux_core::config_write::Durability::NoFsync,
        )
        .expect("save");

        let written = std::fs::read_to_string(&config_path).expect("read back");
        let comment_lines = written
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count();
        assert!(
            comment_lines > 100,
            "a freshly created config must be documented, got {comment_lines} comment lines"
        );
        // And it is still a correct config, not just prose.
        let parsed: Config = toml::from_str(&written).expect("reparse");
        assert_eq!(parsed.projects.len(), 1);
        assert_eq!(parsed.projects[0].id, "project-1");
    }

    #[test]
    fn a_restored_config_keeps_its_comments_through_an_ordinary_save() {
        // The other half of the story: restoring the documentation is worthless
        // if the very next save strips it again. The patch path is
        // comment-preserving, and this proves it end to end on a RESTORED file.
        let raw = bare_user_config();
        let restored = restore_documentation(&raw).expect("restore");

        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, &restored.text).expect("seed restored config");

        // An ordinary value change, saved the way the app saves.
        let mut config: Config = toml::from_str(&restored.text).expect("parse restored");
        config.ui.left_width_pct = 31;
        dux_core::config_write::patch_config_file_with(
            &config_path,
            &config,
            dux_core::config_write::Durability::NoFsync,
        )
        .expect("patch");

        let after = std::fs::read_to_string(&config_path).expect("read back");
        let parsed: Config = toml::from_str(&after).expect("reparse");
        assert_eq!(
            parsed.ui.left_width_pct, 31,
            "the value change did not land"
        );
        // The documentation survived the save.
        let comment_lines = after
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count();
        assert!(
            comment_lines > 100,
            "an ordinary save stripped the restored documentation ({comment_lines} left)"
        );
        // And so did the user's real data.
        assert_eq!(parsed.projects.len(), 6);
        assert_eq!(parsed.macros.entries.len(), 8);
    }

    #[test]
    fn every_empty_table_in_a_fresh_config_ships_an_example() {
        // A user who has no projects, no macros, and no env must still be able
        // to learn the syntax from the file itself, without leaving it.
        let rendered = render_config_default(&Config::default());
        let config: Config = toml::from_str(&rendered).expect("fresh config parses");
        assert!(config.env.is_empty(), "precondition: [env] is empty");
        assert!(config.macros.entries.is_empty(), "precondition: no macros");
        assert!(config.projects.is_empty(), "precondition: no projects");

        // Each empty table is followed by a commented example of its own shape.
        assert!(
            rendered.contains("# EDITOR = \"true\""),
            "[env] has no example"
        );
        assert!(
            rendered.contains("# \"Review\" = { text ="),
            "[macros] has no example"
        );
        assert!(
            rendered.contains("# [[projects]]"),
            "[[projects]] has no example"
        );
        // An empty LIST is the same dead end: allowed_hosts = [] teaches nothing
        // about what may go in it.
        assert!(
            rendered.contains("allowed_hosts = []"),
            "precondition: allowed_hosts is empty by default"
        );
        assert!(
            rendered.contains("#   allowed_hosts = ["),
            "allowed_hosts has no example of the list format"
        );
    }

    #[test]
    fn rendered_server_section_is_local_only() {
        let toml = render_default_config();
        assert!(toml.contains("host = \"127.0.0.1\""));
        assert!(toml.contains("allowed_hosts"));
        assert!(!toml.contains("[server.acme]"));
        assert!(!toml.contains("listen_addrs"));
        assert!(!toml.contains("[auth]"));
    }

    #[test]
    fn ensure_config_rejects_non_ip_host() {
        let mut config = Config::default();
        config.server.host = "example.com".to_string();
        let err = validate_server_host(&config).expect_err("a non-IP host must be rejected");
        assert!(
            err.to_string().contains("example.com"),
            "the error must name the bad host: {err}"
        );
    }

    #[test]
    fn ensure_config_accepts_ip_hosts() {
        let mut config = Config::default();
        config.server.host = "0.0.0.0".to_string();
        validate_server_host(&config).expect("0.0.0.0 is a valid host");
        config.server.host = "127.0.0.1".to_string();
        validate_server_host(&config).expect("loopback is a valid host");
    }

    #[test]
    fn default_config_is_commented_and_complete() {
        let rendered = render_default_config();
        assert!(rendered.contains("# dux configuration"));
        assert!(rendered.contains("[defaults]"));
        assert!(rendered.contains("provider = \"claude\""));
        assert!(rendered.contains("enable_randomized_pet_name_by_default = false"));
        assert!(rendered.contains("pull_before_creating_agent_by_default = true"));
        assert!(rendered.contains("copy_uncommitted_changes_by_default = true"));
        assert!(!rendered.contains("prompt_for_name"));
        assert!(rendered.contains("[providers.claude]"));
        assert!(rendered.contains("[providers.codex]"));
        assert!(rendered.contains("[providers.copilot]"));
        assert!(
            !rendered.contains("oneshot_args"),
            "the removed AI-commit oneshot keys must no longer be rendered"
        );
        assert!(
            !rendered.contains("oneshot_output"),
            "the removed AI-commit oneshot keys must no longer be rendered"
        );
        assert!(rendered.contains("resume_args = "));
        assert!(rendered.contains("[terminal]"));
        assert!(rendered.contains("command = "));
        assert!(rendered.contains("args = []"));
        assert!(rendered.contains("[startup_command_terminal]"));
        assert!(rendered.contains("command = \"$SHELL\""));
        assert!(rendered.contains("args = [\"-l\", \"-c\"]"));
        assert!(rendered.contains("[ui]"));
        assert!(rendered.contains("agent_scrollback_lines = 10000"));
        assert!(rendered.contains("pr_poll_interval_seconds = 180"));
        assert!(rendered.contains("empty_project_separator_min_projects = 5"));
        assert!(rendered.contains("copy_on_select = true"));
        assert!(rendered.contains("compose_bar = true"));
        assert!(rendered.contains("attention_grace_seconds = 3"));
        assert!(rendered.contains("auto_reopen_agents = false"));
        assert!(rendered.contains("always_show_tab_strip = false"));
        assert!(rendered.contains("attention_indicator = true"));
        assert!(rendered.contains("attention_on_bell = true"));
        // The two first-load screens are on by default, so both opt-outs render
        // false. Keep the negative names exactly as spelled.
        assert!(rendered.contains("disable_automated_welcome_screen = false"));
        assert!(rendered.contains("disable_release_notes = false"));
        assert!(rendered.contains("staged_pane_height_pct = "));
        assert!(rendered.contains("commit_pane_height_pct = "));
        assert!(rendered.contains("[capabilities]"));
        assert!(rendered.contains("terminal_identity = \"auto\""));
        assert!(rendered.contains("passthrough = true"));
        assert!(rendered.contains("clipboard_passthrough = \"focused\""));
        assert!(rendered.contains("hyperlinks = true"));
        assert!(rendered.contains("web_notifications = true"));
        assert!(rendered.contains("[editor]"));
        assert!(rendered.contains("default = \"cursor\""));
        assert!(rendered.contains("[server]"));
        assert!(rendered.contains("host = \"127.0.0.1\""));
        assert!(rendered.contains("port = 8080"));
        assert!(rendered.contains("tailscale_enabled = true"));
        assert!(rendered.contains("allowed_hosts = []"));
        assert!(
            !rendered.contains("bind = "),
            "renderer must not emit the deprecated bind key"
        );
        assert!(!rendered.contains("listen_addrs"));
        assert!(!rendered.contains("insecure_allow_remote"));
        assert!(!rendered.contains("dangerously_listen_http"));
        assert!(rendered.contains("color = \"auto\""));
        assert!(rendered.contains("access_log = true"));
        assert!(rendered.contains("max_websocket_events_connections = 32"));
        assert!(rendered.contains("max_websocket_agent_connections = 32"));
        assert!(rendered.contains("max_websocket_terminal_connections = 64"));
        assert!(rendered.contains("max_websocket_tab_connections = 64"));
        assert!(rendered.contains("search_index_max_files = 50000"));
        assert!(rendered.contains("tree_list_max_concurrency = 8"));
        assert!(rendered.contains("release_notes_max_concurrency = 2"));
        assert!(rendered.contains("file_drop_max_bytes = 104857600"));
        assert!(rendered.contains("file_drop_max_concurrency = 2"));
        assert!(rendered.contains("agent_tabs_max = 20"));
        assert!(rendered.contains("title = \"dux\""));
        // Assert the active key (not a commented-out line) so a regression that
        // emits favicon only as a comment is caught.
        assert!(rendered.lines().any(|l| l.trim() == "favicon = \"\""));
        // The favicon comment documents the curated tint set (recolors a flat duck
        // silhouette), not the removed hex/URL inputs.
        assert!(rendered.contains("curated tint colors"));
        assert!(rendered.contains("duck silhouette in the browser tab"));
        assert!(!rendered.contains("863bff"));
        assert!(!rendered.contains("[auth]"));
        assert!(!rendered.contains("[server.acme]"));
        assert!(rendered.contains("[keys]"));
        assert!(rendered.contains("show_terminal_keys = true"));
        assert!(rendered.contains("move_down = "));
        assert!(rendered.contains("quit = "));
        assert!(
            !rendered.contains("commit_prompt"),
            "the removed AI-commit prompt key must no longer be rendered"
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

    /// The AI commit-message feature was removed, but an existing user config may
    /// still carry its now-obsolete keys: `defaults.commit_prompt` and the
    /// per-provider `oneshot_args` / `oneshot_output`. Loading must tolerate them
    /// (the structs are `#[serde(default)]`, never `deny_unknown_fields`) instead
    /// of erroring, so an upgrade does not break startup.
    #[test]
    fn config_with_removed_ai_commit_keys_still_loads() {
        let parsed: Config = toml::from_str(
            r#"
[defaults]
provider = "claude"
commit_prompt = """
Write a commit message for the staged diff.
"""

[providers.claude]
command = "claude"
oneshot_args = ["--bare", "-p", "{prompt}"]
oneshot_output = "stdout"

[providers.codex]
command = "codex"
oneshot_args = ["exec", "-o", "{tempfile}", "{prompt}"]
oneshot_output = "tempfile"
"#,
        )
        .expect("a config carrying the removed AI-commit keys must still load");

        // The surviving provider fields parse normally; the obsolete keys are
        // simply ignored.
        assert_eq!(parsed.defaults.provider, "claude");
        assert_eq!(
            parsed.providers.get("claude").map(|c| c.command.as_str()),
            Some("claude")
        );
        assert_eq!(
            parsed.providers.get("codex").map(|c| c.command.as_str()),
            Some("codex")
        );
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
    fn old_config_missing_copy_uncommitted_changes_defaults_to_true() {
        let parsed: Config = toml::from_str(
            r#"
[defaults]
provider = "claude"
start_directory = "/tmp"
enable_randomized_pet_name_by_default = false
pull_before_creating_agent_by_default = true
"#,
        )
        .expect("config should parse");

        assert!(parsed.defaults.copy_uncommitted_changes_by_default);
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
    fn shutdown_timeout_keys_render_in_correct_sections() {
        let rendered = render_default_config();
        let lines: Vec<&str> = rendered.lines().collect();

        // The top-level key must render before the first table header, or TOML
        // would bind it to a table.
        let root_line = lines
            .iter()
            .position(|l| l.trim() == "shutdown_timeout_seconds = 30")
            .expect("top-level shutdown_timeout_seconds line present");
        let defaults_line = lines
            .iter()
            .position(|l| l.trim() == "[defaults]")
            .expect("[defaults] header present");
        assert!(
            root_line < defaults_line,
            "top-level shutdown_timeout_seconds must render before [defaults]:\n{rendered}"
        );

        // The server key must render under [server].
        let server_line = lines
            .iter()
            .position(|l| l.trim() == "[server]")
            .expect("[server] header present");
        let server_key_line = lines
            .iter()
            .enumerate()
            .skip(server_line + 1)
            .position(|(_, l)| l.trim() == "shutdown_timeout_seconds = 30")
            .map(|p| p + server_line + 1);
        assert!(
            server_key_line.is_some(),
            "[server].shutdown_timeout_seconds must render under [server]:\n{rendered}"
        );

        // And both parse back to their defaults.
        let parsed: Config = toml::from_str(&rendered).expect("rendered default parses");
        assert_eq!(parsed.shutdown_timeout_seconds, 30);
        assert_eq!(parsed.server.shutdown_timeout_seconds, 30);
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
            install_hint: None,
            forward_scroll: None,
            web_dragdrop_paste: None,
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
            install_hint: None,
            forward_scroll: None,
            web_dragdrop_paste: None,
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
                    install_hint: None,
                    forward_scroll: None,
                    web_dragdrop_paste: None,
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
                    install_hint: None,
                    forward_scroll: None,
                    web_dragdrop_paste: None,
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
    fn default_opencode_provider_uses_continue_resume() {
        let providers = default_provider_commands();
        let opencode = providers.iter().find(|(n, _)| *n == "opencode").unwrap();
        let cfg = &opencode.1;
        assert_eq!(cfg.command, "opencode");
        assert!(cfg.resume_args.is_some());
    }

    #[test]
    fn default_provider_commands_excludes_retired_gemini() {
        let providers = default_provider_commands();
        assert_eq!(providers.len(), 4, "four providers ship as defaults");
        assert!(
            providers.iter().all(|(name, _)| *name != "gemini"),
            "gemini was retired and must not ship as a default provider"
        );
    }

    #[test]
    fn prune_retired_key_actions_drops_generate_commit_message() {
        // A binding for the retired action would abort startup at validate_keys...
        let mut keys = KeysConfig::default();
        keys.bindings.insert(
            "generate_commit_message".to_string(),
            vec!["ctrl-g".to_string()],
        );
        assert!(
            validate_keys(&keys).is_err(),
            "precondition: the retired action fails validate_keys"
        );

        // ...so the load-time migration prunes it from an existing config.
        let mut doc: DocumentMut =
            "[keys]\ngenerate_commit_message = [\"ctrl-g\"]\nquit = [\"ctrl-q\"]\n"
                .parse()
                .expect("parse doc");

        let changed = prune_retired_key_actions(&mut doc);

        assert!(changed, "the retired action binding should be pruned");
        assert!(
            doc["keys"].get("generate_commit_message").is_none(),
            "the retired action must be removed from [keys]"
        );
        assert!(
            doc["keys"].get("quit").is_some(),
            "a live binding must be preserved"
        );
    }

    #[test]
    fn prune_retired_key_actions_noop_without_keys_table() {
        let mut doc: DocumentMut = "[server]\nhost = \"127.0.0.1\"\n"
            .parse()
            .expect("parse doc");
        assert!(
            !prune_retired_key_actions(&mut doc),
            "no [keys] table means nothing to prune"
        );
    }

    #[test]
    fn default_copilot_provider_disables_resume() {
        let providers = default_provider_commands();
        let copilot = providers.iter().find(|(n, _)| *n == "copilot").unwrap();
        let cfg = &copilot.1;
        assert_eq!(cfg.command, "copilot");
        assert_eq!(cfg.resume_args, None);
        assert!(!cfg.supports_session_resume());
    }

    #[test]
    fn ensure_defaults_adds_opencode_and_copilot_but_not_retired_gemini() {
        let mut providers = ProvidersConfig {
            commands: indexmap::IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: Some(vec!["--continue".to_string()]),
                    resume_wait_timeout_ms: None,
                    install_hint: None,
                    forward_scroll: None,
                    web_dragdrop_paste: None,
                },
            )]),
        };

        providers.ensure_defaults();

        assert!(
            providers.get("opencode").is_some(),
            "opencode should be added"
        );
        assert!(
            providers.get("gemini").is_none(),
            "gemini was retired and must not be re-added as a default"
        );
        assert!(providers.get("codex").is_some(), "codex should be added");
        assert!(
            providers.get("copilot").is_some(),
            "copilot should be added"
        );
        assert_eq!(providers.get("opencode").unwrap().command, "opencode");
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

    #[test]
    fn ensure_config_first_creation_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            lock_path: root.join("dux.lock"),
            worktrees_root: root.join("worktrees"),
            root,
        };
        crate::config::ensure_config(&paths).expect("ensure");
        let mode = std::fs::metadata(&paths.config_path)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "first-created config must be 0600, got {mode:o}"
        );
    }

    #[test]
    fn ensure_config_prunes_stock_gemini_from_existing_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            lock_path: root.join("dux.lock"),
            worktrees_root: root.join("worktrees"),
            root,
        };

        // Seed an existing config that still ships the stock gemini provider,
        // rendered exactly as dux would have written it. The stock block matcher
        // now lives in core (`config_migrate`); this end-to-end test just needs
        // the block on disk to prove `ensure_config` prunes AND persists it.
        let stock_gemini = ProviderCommandConfig {
            command: "gemini".to_string(),
            args: Vec::new(),
            resume_args: Some(vec!["--resume".to_string()]),
            resume_wait_timeout_ms: None,
            install_hint: Some("brew install gemini-cli".to_string()),
            forward_scroll: None,
            web_dragdrop_paste: None,
        };
        let mut body = render_default_config();
        render_provider_config(&mut body, "gemini", &stock_gemini);
        fs::write(&paths.config_path, &body).expect("seed config");
        assert!(
            fs::read_to_string(&paths.config_path)
                .unwrap()
                .contains("[providers.gemini]"),
            "precondition: the seeded config carries a gemini block"
        );

        let config = ensure_config(&paths).expect("ensure");

        // The pruned block must be gone from disk and not re-added in memory.
        let saved = fs::read_to_string(&paths.config_path).expect("read");
        assert!(
            !saved.contains("[providers.gemini]"),
            "stock gemini block should be pruned from the persisted config: {saved}"
        );
        assert!(
            config.providers.get("gemini").is_none(),
            "gemini must not be re-added as a default after pruning"
        );
        assert!(
            config.providers.get("claude").is_some(),
            "other providers must survive the prune"
        );
    }
}

#[cfg(test)]
mod web_dragdrop_paste_render_tests {
    use super::*;

    /// The generated config must SHOW the setting with its shipped value and
    /// explain itself in place, per the "config file is the documentation" tenet.
    /// Rendered and parsed back rather than asserted from the format string, so a
    /// broken escape shows up as a failing test instead of an unparseable config.
    #[test]
    fn generated_config_documents_and_carries_web_dragdrop_paste() {
        let config = Config::default();
        let rendered = render_config_documented(&config);

        // The value dux ships for each provider, written out rather than implied.
        assert!(
            rendered.contains("web_dragdrop_paste = \"bare\""),
            "the bare providers must render their value"
        );
        assert!(
            rendered.contains("web_dragdrop_paste = \"single_quoted\""),
            "codex must render its measured value"
        );
        // All four forms are named in the comment, so a user picking one never
        // has to leave the file to find out what the options are.
        for form in [
            "bare",
            "single_quoted",
            "double_quoted",
            "backslash_escaped",
        ] {
            assert!(
                rendered.contains(&format!("#   {form}")),
                "the comment must list the {form} form"
            );
        }
        // The web-only reason, which is why the key carries a `web_` prefix.
        assert!(
            rendered.contains("terminal emulator's job"),
            "the comment must say why this is web-only"
        );
        // What a wrong value costs, so nobody reads it as dangerous.
        assert!(
            rendered.contains("not attached automatically"),
            "the comment must say what getting it wrong looks like"
        );

        let parsed: Config = toml::from_str(&rendered).expect("rendered config must parse");
        assert_eq!(
            parsed.providers.commands["codex"].resolved_web_dragdrop_paste(),
            dux_core::config::WebDragDropPaste::SingleQuoted
        );
        assert_eq!(
            parsed.providers.commands["claude"].resolved_web_dragdrop_paste(),
            dux_core::config::WebDragDropPaste::Bare
        );
    }
}
