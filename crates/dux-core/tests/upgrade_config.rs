//! Upgrade survival for `config.toml`: an install that predates the current
//! release must load with every value the user set still in effect.
//!
//! There is deliberately no version number in `config.toml`. dux's config
//! compatibility rests on three mechanisms instead, and these tests pin all
//! three against a real old-shaped file rather than against the mechanisms'
//! internals:
//!
//! 1. **Serde defaults.** Every section carries `#[serde(default)]`, so a key
//!    added after the user's file was written simply defaults. There is no
//!    `deny_unknown_fields` anywhere, so a key dux has since RETIRED is ignored
//!    rather than failing the load.
//! 2. **Load-time migrations** (`config_migrate`), applied in memory at every
//!    entrypoint: a deprecated key is rewritten to its replacement, and an
//!    untouched stock block for a retired provider is pruned.
//! 3. **Surgical writes** (`config_write::patch_config_file`), which edit the
//!    existing document with `toml_edit` and therefore never discard a key dux
//!    does not know about.
//!
//! What these tests CANNOT cover is a true rollback: running an OLDER dux binary
//! against a config this build wrote. That needs two binaries, and only one
//! exists in a checkout. The property tested instead is forward survival plus
//! the round trip in mechanism 3, which is what an upgrade actually exercises.

use std::collections::BTreeMap;

use dux_core::config::{Config, DuxPaths, load_config};
use dux_core::config_write::{Durability, patch_config_file_with, save_config_with};

/// A `config.toml` as an older dux wrote it, hand-edited by its owner the way a
/// real one is. Every element here is load-bearing:
///
/// - `[server] bind` is a DEPRECATED key with a migration to `host`/`port`.
/// - `[server] max_websocket_connections` is a RETIRED key with no replacement.
/// - `[server.acme]` and `[auth]` are ORPHANED sections dux once wrote.
/// - `[defaults] prompt_for_name` is a deprecated key whose replacement inverts
///   its meaning.
/// - `[providers.gemini]` is the untouched stock block of a RETIRED provider.
/// - `[providers.myclaude]` is a provider the user added by hand.
/// - `[[projects]]` carries `leading_branch`, which current dux keeps in SQLite
///   and no longer writes to config.
/// - `[fork_only]` and `ui.my_experiment` are keys no dux version ever had.
/// - Nothing declares any of the many keys added since, so they must default.
const OLD_CONFIG: &str = r#"
# My dux config. Do not lose these comments.
shutdown_timeout_seconds = 17

[defaults]
provider = "codex"
prompt_for_name = false

[server]
bind = "0.0.0.0:9100"
max_websocket_connections = 12

[server.acme]
enabled = true
email = "ada@example.invalid"

[auth]
username = "ada"

[logging]
level = "debug"

[terminal]
command = "/usr/bin/fish"
args = ["-l"]

[providers.gemini]
command = "gemini"
args = []
resume_args = ["--resume"]
resume_wait_timeout_ms = 0
install_hint = "brew install gemini-cli"

[providers.myclaude]
command = "/opt/bin/claude"
args = ["--dangerously-skip-permissions"]

[ui]
left_width_pct = 33
my_experiment = "keep me"

[fork_only]
setting = 1

[[projects]]
id = "11111111-1111-4111-8111-111111111111"
path = "/home/ada/code/widget"
name = "widget"
default_provider = "codex"
leading_branch = "trunk"
custom_note = "hand added"
"#;

/// Write `raw` as the config of a throwaway dux root and load it the way every
/// entrypoint does.
fn load_old(raw: &str) -> (tempfile::TempDir, DuxPaths, Config) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, raw).expect("write config");
    let config = load_config(&paths);
    (tmp, paths, config)
}

fn paths_in(root: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: root.to_path_buf(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    }
}

#[test]
fn an_older_config_loads_without_error_and_keeps_every_value_the_user_set() {
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);

    // Plain values, untouched by any migration.
    assert_eq!(config.shutdown_timeout_seconds, 17);
    assert_eq!(config.defaults.provider, "codex");
    assert_eq!(config.logging.level, "debug");
    assert_eq!(config.terminal.command, "/usr/bin/fish");
    assert_eq!(config.terminal.args, vec!["-l".to_string()]);
    assert_eq!(config.ui.left_width_pct, 33);

    // The user's hand-added provider survives, arguments and all.
    let mine = config
        .providers
        .commands
        .get("myclaude")
        .expect("a hand-added provider must survive the load");
    assert_eq!(mine.command, "/opt/bin/claude");
    assert_eq!(
        mine.args,
        vec!["--dangerously-skip-permissions".to_string()]
    );

    // The project the user configured is still there, with its name and provider.
    assert_eq!(config.projects.len(), 1, "{:#?}", config.projects);
    let project = &config.projects[0];
    assert_eq!(project.id, "11111111-1111-4111-8111-111111111111");
    assert_eq!(project.path, "/home/ada/code/widget");
    assert_eq!(project.name.as_deref(), Some("widget"));
    assert_eq!(project.default_provider.as_deref(), Some("codex"));
}

#[test]
fn a_deprecated_server_bind_still_decides_where_the_server_listens() {
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);
    // The whole point of migrating in `load_config` rather than only in the TUI's
    // `ensure_config`: an operator who upgrades and runs `dux server` keeps the
    // non-loopback address they deliberately chose.
    assert_eq!(config.server.host, "0.0.0.0");
    assert_eq!(config.server.port, 9100);
}

#[test]
fn a_deprecated_prompt_for_name_is_migrated_with_its_meaning_inverted() {
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);
    // `prompt_for_name = false` means "do not ask me, just name it", which is
    // the same wish as `enable_randomized_pet_name_by_default = true`.
    assert!(config.defaults.enable_randomized_pet_name_by_default);
}

#[test]
fn a_retired_stock_provider_stops_being_offered_after_the_upgrade() {
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);
    assert!(
        !config.providers.commands.contains_key("gemini"),
        "the untouched stock gemini block must be pruned: {:?}",
        config.providers.commands.keys().collect::<Vec<_>>()
    );
    // ...while the providers dux does ship are all present.
    for shipped in ["claude", "codex", "opencode", "copilot"] {
        assert!(
            config.providers.commands.contains_key(shipped),
            "{shipped} must be restored by ensure_defaults"
        );
    }
}

#[test]
fn keys_added_since_the_old_config_was_written_fall_back_to_their_defaults() {
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);
    let fresh = Config::default();
    // Sections the old file never mentioned.
    assert_eq!(
        config.startup_command_terminal,
        fresh.startup_command_terminal
    );
    assert_eq!(config.editor, fresh.editor);
    assert_eq!(config.capabilities, fresh.capabilities);
    // Keys added inside sections the old file DID mention: the one the user set
    // wins, every sibling defaults.
    assert_eq!(config.ui.agent_tabs_max, fresh.ui.agent_tabs_max);
    assert_eq!(
        config.ui.status_clear_seconds,
        fresh.ui.status_clear_seconds
    );
    assert_eq!(
        config.server.tailscale_enabled,
        fresh.server.tailscale_enabled
    );
    assert_eq!(config.server.access_log, fresh.server.access_log);
    assert_eq!(
        config.defaults.pull_before_creating_agent_by_default,
        fresh.defaults.pull_before_creating_agent_by_default
    );
}

#[test]
fn a_retired_key_dux_no_longer_reads_does_not_fail_the_load() {
    // `[server] max_websocket_connections` was split into three per-class caps.
    // No struct in dux uses `deny_unknown_fields`, so the key is ignored rather
    // than fatal, and the operator learns about it from `dux.log` (see
    // `warn_on_removed_max_websocket_connections`). What must NOT happen is the
    // whole config falling back to defaults.
    let (_tmp, _paths, config) = load_old(OLD_CONFIG);
    assert_eq!(
        config.shutdown_timeout_seconds, 17,
        "a retired key must not cost the user the rest of their config"
    );
    // The three replacements are in force at their defaults.
    let fresh = Config::default();
    assert_eq!(
        config.server.max_websocket_events_connections,
        fresh.server.max_websocket_events_connections
    );
}

#[test]
fn writing_after_an_upgrade_keeps_the_users_unmanaged_keys_and_comments() {
    // The round trip an upgrade really performs: load the old file, change one
    // setting, save through the surgical patch path, and load it again.
    let (_tmp, paths, mut config) = load_old(OLD_CONFIG);
    config.ui.left_width_pct = 41;
    patch_config_file_with(&paths.config_path, &config, Durability::NoFsync)
        .expect("patch the existing config");

    let saved = std::fs::read_to_string(&paths.config_path).expect("read back");

    // Keys no dux version ever had, at the top level, inside a known section,
    // and inside an array-of-tables entry.
    assert!(saved.contains("[fork_only]"), "{saved}");
    assert!(saved.contains("my_experiment = \"keep me\""), "{saved}");
    assert!(saved.contains("custom_note = \"hand added\""), "{saved}");
    // The user's own comment.
    assert!(saved.contains("Do not lose these comments."), "{saved}");

    let reloaded = load_config(&paths);
    assert_eq!(reloaded.ui.left_width_pct, 41, "the edit must stick");
    assert_eq!(
        reloaded.shutdown_timeout_seconds, 17,
        "an unrelated value must not move"
    );
    assert_eq!(reloaded.projects.len(), 1);
    assert_eq!(reloaded.projects[0].path, "/home/ada/code/widget");
    assert!(
        reloaded.providers.commands.contains_key("myclaude"),
        "the hand-added provider must survive the write"
    );
}

#[test]
fn the_migrated_values_are_what_gets_persisted_not_the_deprecated_keys() {
    let (_tmp, paths, config) = load_old(OLD_CONFIG);
    save_config_with(&paths.config_path, &config, Durability::NoFsync).expect("save");
    let saved = std::fs::read_to_string(&paths.config_path).expect("read back");

    // The surgical writer emits the migrated `host`/`port`. The deprecated key is
    // still in the file (the writer only ADDS what it knows; only the TUI's
    // `ensure_config` removes it), so the load-time migration is what has to keep
    // being correct, and it does: reloading is idempotent.
    assert!(saved.contains("host = \"0.0.0.0\""), "{saved}");
    assert!(saved.contains("port = 9100"), "{saved}");

    let reloaded = load_config(&paths);
    assert_eq!(reloaded.server.host, "0.0.0.0");
    assert_eq!(reloaded.server.port, 9100);
    assert!(reloaded.defaults.enable_randomized_pet_name_by_default);
}

#[test]
fn a_second_load_of_a_migrated_config_is_stable() {
    // Idempotence matters more than usual here: `load_config` runs at every
    // entrypoint AND on every config reload, so a migration that flip-flopped
    // would flip-flop many times per session.
    let (_tmp, paths, first) = load_old(OLD_CONFIG);
    let second = load_config(&paths);
    assert_eq!(first, second);
}

#[test]
fn an_empty_config_file_is_treated_as_all_defaults_rather_than_an_error() {
    // The other end of the upgrade range: a file truncated to nothing (a botched
    // editor save, a full disk) must not stop dux from starting.
    let (_tmp, _paths, config) = load_old("");
    let mut fresh = Config::default();
    fresh.providers.ensure_defaults();
    assert_eq!(
        config.providers.commands.len(),
        fresh.providers.commands.len()
    );
    assert_eq!(
        config.shutdown_timeout_seconds,
        fresh.shutdown_timeout_seconds
    );
}

#[test]
fn an_old_config_with_env_maps_keeps_both_the_global_and_the_project_environment() {
    // `[env]` and per-project `env` are user data that a lost key would silently
    // change the behaviour of an agent's shell, so pin them across the round trip.
    let raw = r#"
[env]
GLOBAL_ONE = "1"

[[projects]]
id = "22222222-2222-4222-8222-222222222222"
path = "/home/ada/code/other"

[projects.env]
PROJECT_ONE = "2"
"#;
    let (_tmp, paths, config) = load_old(raw);
    assert_eq!(
        config.env,
        BTreeMap::from([("GLOBAL_ONE".to_string(), "1".to_string())])
    );
    assert_eq!(
        config.projects[0].env,
        BTreeMap::from([("PROJECT_ONE".to_string(), "2".to_string())])
    );

    patch_config_file_with(&paths.config_path, &config, Durability::NoFsync).expect("patch");
    let reloaded = load_config(&paths);
    assert_eq!(reloaded.env, config.env);
    assert_eq!(reloaded.projects[0].env, config.projects[0].env);
}
