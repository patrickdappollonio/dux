//! Load-time config migrations, applied by `config::load_config` in memory at
//! every entrypoint (the TUI, `dux server`, and the web bootstrap), because a
//! migrated key can configure the server itself. Two kinds:
//!
//! - Deprecated-key migrations: an old key is rewritten to its replacement
//!   (`[server] bind` to host/port, `[defaults] prompt_for_name` to the inverse
//!   `enable_randomized_pet_name_by_default`).
//! - Retired-provider pruning: an untouched stock block for a provider dux no
//!   longer ships is removed so its picker stops offering it. A user-customized
//!   block of the same name is preserved, because config wins for explicit
//!   preferences.
//!
//! These operate on a `toml_edit::DocumentMut` and return whether the document
//! changed. Persisting the migrated document to disk is a surface concern the
//! TUI handles; `load_config` only applies the result in memory.

use anyhow::{Result, bail};
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::config::ProviderCommandConfig;
use crate::config_write::{ensure_table, remove_table_key_item};

/// Apply every load-time config migration to `doc`, returning whether it
/// changed. Called from `config::load_config` (in memory, every entrypoint) and
/// from the TUI's `ensure_config` (which then persists the change). Retired
/// KEYBINDING actions are NOT handled here: they only matter to the TUI's
/// `validate_keys`, so that pruning stays TUI-side.
pub fn apply_load_migrations(doc: &mut DocumentMut) -> Result<bool> {
    let deprecations_changed = apply_config_deprecations(doc)?;
    let retired_changed = prune_retired_providers(doc);
    Ok(deprecations_changed || retired_changed)
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
    /// What to write instead, said where a value carried over from it is
    /// shown.
    replace_with: &'static str,
}

const DEPRECATED_CONFIG_KEYS: &[DeprecatedConfigKeyRule] = &[
    DeprecatedConfigKeyRule {
        old: DeprecatedConfigKey {
            section: "defaults",
            key: "prompt_for_name",
        },
        action: DeprecatedConfigKeyAction::Replace {
            migrate: migrate_prompt_for_name,
        },
        replace_with: "defaults.enable_randomized_pet_name_by_default (its opposite)",
    },
    DeprecatedConfigKeyRule {
        old: DeprecatedConfigKey {
            section: "server",
            key: "bind",
        },
        action: DeprecatedConfigKeyAction::Replace {
            migrate: migrate_server_bind,
        },
        replace_with: "server.host and server.port",
    },
    DeprecatedConfigKeyRule {
        old: DeprecatedConfigKey {
            section: "server",
            key: "tailscale_enabled",
        },
        action: DeprecatedConfigKeyAction::Replace {
            migrate: migrate_tailscale_enabled,
        },
        replace_with: "server.tailscale",
    },
];

/// Every deprecated key in `doc` the load migrations cannot carry over, each
/// tried on its own (so one failure never hides another), as its dotted key
/// and the sentence the migration fails with.
pub fn load_migration_failures(doc: &DocumentMut) -> Vec<(Vec<String>, String)> {
    DEPRECATED_CONFIG_KEYS
        .iter()
        .filter_map(|rule| {
            let mut alone = doc.clone();
            apply_config_deprecations_with(&mut alone, std::slice::from_ref(rule))
                .err()
                .map(|error| {
                    (
                        vec![rule.old.section.to_string(), rule.old.key.to_string()],
                        format!("{error:#}"),
                    )
                })
        })
        .collect()
}

/// Migrate the one deprecated key `[section] key` in `doc`, if it is there
/// and a migration rule carries its value: the value is written under the
/// key that replaced it (unless `doc` already sets that one) and the
/// deprecated key is removed. A value its rule cannot read is left as it is,
/// for the caller to remove or keep.
pub fn carry_over_deprecated_key(doc: &mut DocumentMut, section: &str, key: &str) {
    let Some(rule) = DEPRECATED_CONFIG_KEYS
        .iter()
        .find(|rule| rule.old.section == section && rule.old.key == key)
    else {
        return;
    };
    let mut migrated = doc.clone();
    if apply_config_deprecations_with(&mut migrated, std::slice::from_ref(rule)).is_ok() {
        *doc = migrated;
    }
}

/// A value the load migrations write in place of a deprecated key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarriedOver {
    /// The path of the setting the value lands in.
    pub key: Vec<String>,
    /// The deprecated key's section and name.
    pub old_section: &'static str,
    pub old_key: &'static str,
    /// The deprecated key's value as the file writes it.
    pub old_value: String,
    /// What to write instead, for the sentence.
    pub replace_with: &'static str,
}

/// Every value the deprecated-key migrations would write into `doc`, each
/// rule tried on its own: a key that is absent before and present after.
/// A migration that fails, or drops its key without carrying a value over,
/// writes none.
pub fn carried_over(doc: &DocumentMut) -> Vec<CarriedOver> {
    let mut found = Vec::new();
    for rule in DEPRECATED_CONFIG_KEYS {
        let Some(old_value) = doc
            .get(rule.old.section)
            .and_then(Item::as_table_like)
            .and_then(|table| table.get(rule.old.key))
            .and_then(Item::as_value)
            .map(|value| {
                let mut value = value.clone();
                value.decor_mut().clear();
                value.to_string()
            })
        else {
            continue;
        };
        let mut after = doc.clone();
        if apply_config_deprecations_with(&mut after, std::slice::from_ref(rule)).is_err() {
            continue;
        }
        let Some(table) = after.get(rule.old.section).and_then(Item::as_table_like) else {
            continue;
        };
        for (key, _) in table.iter() {
            let existed = doc
                .get(rule.old.section)
                .and_then(Item::as_table_like)
                .is_some_and(|before| before.contains_key(key));
            if !existed {
                found.push(CarriedOver {
                    key: vec![rule.old.section.to_string(), key.to_string()],
                    old_section: rule.old.section,
                    old_key: rule.old.key,
                    old_value: old_value.clone(),
                    replace_with: rule.replace_with,
                });
            }
        }
    }
    found
}

/// Every retired provider's stock block in `doc` the load prunes, as its
/// dotted key and why.
pub fn retired_provider_prunes(doc: &DocumentMut) -> Vec<(Vec<String>, String)> {
    let mut pruned = doc.clone();
    let Some(before) = doc.get("providers").and_then(Item::as_table) else {
        return Vec::new();
    };
    prune_retired_providers(&mut pruned);
    let after = pruned.get("providers").and_then(Item::as_table);
    before
        .iter()
        .filter(|(name, _)| after.is_none_or(|after| !after.contains_key(name)))
        .map(|(name, _)| {
            (
                vec!["providers".to_string(), name.to_string()],
                format!(
                    "dux no longer ships the {name} provider, and drops the untouched \
                     [providers.{name}] block it once wrote when it loads the file (change any \
                     value in it to keep it)"
                ),
            )
        })
        .collect()
}

fn apply_config_deprecations(doc: &mut DocumentMut) -> Result<bool> {
    apply_config_deprecations_with(doc, DEPRECATED_CONFIG_KEYS)
}

fn apply_config_deprecations_with(
    doc: &mut DocumentMut,
    rules: &[DeprecatedConfigKeyRule],
) -> Result<bool> {
    let mut changed = false;
    for rule in rules {
        let Some(old_item) = remove_table_key_item(doc, rule.old.section, rule.old.key) else {
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

    let table = ensure_table(doc, "defaults");
    if !table.contains_key("enable_randomized_pet_name_by_default") {
        table["enable_randomized_pet_name_by_default"] = toml_edit::value(!prompt_for_name);
    }
    Ok(())
}

/// Migrate the deprecated `[server] bind` key to the new host / port shape. A
/// NON-LOOPBACK bind writes its IP into `host` and its port into `port` (so a
/// previously public bind keeps serving where the operator put it), warning so
/// the change is visible. A LOOPBACK bind is dropped silently; the new
/// loopback-host default already covers it. An empty or unparseable value is
/// dropped silently. Existing new-key values are never overwritten (the user's
/// explicit choice wins).
fn migrate_server_bind(
    doc: &mut DocumentMut,
    old: DeprecatedConfigKey,
    old_item: Item,
) -> Result<()> {
    let Some(raw) = old_item.as_value().and_then(Value::as_str) else {
        bail!(
            "unsupported config key [{}.{}]: expected a string value",
            old.section,
            old.key
        );
    };

    let Ok(addr) = raw.trim().parse::<std::net::SocketAddr>() else {
        // Not a valid IP:port, so there is nothing safe to migrate; the new
        // defaults apply. This also drops a hostname bind such as
        // "localhost:9000", which `SocketAddr` cannot parse and which the bind
        // key never supported.
        return Ok(());
    };

    if addr.ip().is_loopback() {
        // Loopback bind: the new default host is already loopback, so there is
        // nothing to carry over. Drop it silently.
        return Ok(());
    }

    // Non-loopback bind: carry the IP into `host` and the port into `port` so a
    // previously reachable bind keeps serving where the operator placed it.
    let table = ensure_table(doc, "server");
    if !table.contains_key("host") {
        table["host"] = toml_edit::value(addr.ip().to_string());
    }
    if !table.contains_key("port") {
        table["port"] = toml_edit::value(i64::from(addr.port()));
    }
    crate::logger::warn(&format!(
        "[server] migrated the deprecated `bind = \"{raw}\"` to host = \"{}\" and port = {}. \
         This server listens on a non-loopback address; only run it on a network you trust.",
        addr.ip(),
        addr.port()
    ));
    Ok(())
}

/// Migrate the deprecated boolean `[server] tailscale_enabled` to the tri-state
/// `[server] tailscale` key. `true` becomes `"yes"` and `false` becomes `"no"`:
/// a boolean says bind it or do not, and neither of them says "keep watching for
/// the interface", so neither becomes `"auto"`.
///
/// The new key wins whenever both are present, and the boolean is removed from
/// the document either way, so the next canonical save stops carrying it.
///
/// A non-boolean value is dropped silently rather than failing the load: the key
/// no longer exists, so there is nothing to be strict about, and the new key's
/// own default (`auto`) is a safe answer.
fn migrate_tailscale_enabled(
    doc: &mut DocumentMut,
    _old: DeprecatedConfigKey,
    old_item: Item,
) -> Result<()> {
    let Some(enabled) = old_item.as_value().and_then(Value::as_bool) else {
        return Ok(());
    };
    let mode = if enabled {
        crate::config::TailscaleMode::Yes
    } else {
        crate::config::TailscaleMode::No
    };
    let table = ensure_table(doc, "server");
    if table.contains_key("tailscale") {
        crate::logger::warn(&format!(
            "[server] tailscale_enabled = {enabled} is the retired boolean form of this \
             setting and your file also sets [server] tailscale, so the tailscale value is \
             what dux uses and the boolean is being dropped. Nothing else to do."
        ));
        return Ok(());
    }
    table["tailscale"] = toml_edit::value(mode.as_str());
    crate::logger::warn(&format!(
        "[server] tailscale_enabled = {enabled} has been replaced by the tri-state \
         tailscale = \"auto\" | \"yes\" | \"no\", and is being read as \"{}\" for this run. \
         Set [server] tailscale yourself to choose: \"auto\" binds your Tailscale address \
         whenever it is there and drops that one listener when it is not.",
        mode.as_str()
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// Retired providers
//
// A retired provider once shipped as a default but no longer does: it is not
// rendered into new configs and not re-added by `ProvidersConfig::ensure_defaults`.
// An untouched stock block is pruned on load so nobody keeps a dead one forever;
// a customized block of the same name is kept, since config wins for explicit
// preferences.
// ---------------------------------------------------------------------------

/// The retired providers and the exact stock block dux shipped for each (as its
/// renderer WROTE it: an absent `resume_wait_timeout_ms` was materialized to `0`,
/// and `forward_scroll` was left unset). Recognizes an untouched stock block so
/// it can be pruned while a user-customized block of the same name is preserved.
fn retired_providers() -> [(&'static str, ProviderCommandConfig); 1] {
    [("gemini", retired_stock_gemini())]
}

fn retired_stock_gemini() -> ProviderCommandConfig {
    ProviderCommandConfig {
        command: "gemini".to_string(),
        args: Vec::new(),
        resume_args: Some(vec!["--resume".to_string()]),
        // The renderer writes `resume_wait_timeout_ms = 0` for a None timeout, so
        // the stock block dux persisted parses back to `Some(0)`.
        resume_wait_timeout_ms: Some(0),
        install_hint: Some("brew install gemini-cli".to_string()),
        forward_scroll: None,
        // gemini was retired before `web_dragdrop_paste` existed, so the stock
        // block dux shipped never carried the key.
        web_dragdrop_paste: None,
    }
}

/// Remove `[providers.<name>]` tables for retired providers when they still
/// match the stock block dux shipped. A customized block (or one a user adds
/// back later) does not match and is left untouched. Returns whether the
/// document changed.
fn prune_retired_providers(doc: &mut DocumentMut) -> bool {
    let Some(providers) = doc.get_mut("providers").and_then(Item::as_table_mut) else {
        return false;
    };
    let mut changed = false;
    for (name, stock) in retired_providers() {
        let matches = providers
            .get(name)
            .and_then(Item::as_table)
            .is_some_and(|table| table_matches_provider_config(table, &stock));
        if matches {
            providers.remove(name);
            changed = true;
        }
    }
    changed
}

/// Parse a `[providers.<name>]` table (as it appears in config.toml) into a
/// `ProviderCommandConfig`, wrapping it in a standalone document to avoid table
/// header ambiguity.
fn provider_table_config(table: &Table) -> Option<ProviderCommandConfig> {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        provider: ProviderCommandConfig,
    }
    let mut doc = DocumentMut::new();
    doc.insert("provider", Item::Table(table.clone()));
    toml::from_str::<Wrapper>(&doc.to_string())
        .ok()
        .map(|wrapper| wrapper.provider)
}

/// Whether a config's provider table is the exact stock block dux shipped (so it
/// can be retired), as opposed to one the user customized (which is preserved).
/// `resume_wait_timeout_ms` is compared through `unwrap_or(0)` so an absent value
/// and an explicit `0` (semantically identical: no timeout) both match the stock.
fn table_matches_provider_config(table: &Table, stock: &ProviderCommandConfig) -> bool {
    let Some(user) = provider_table_config(table) else {
        return false;
    };
    user.command == stock.command
        && user.args == stock.args
        && user.resume_args == stock.resume_args
        && user.resume_wait_timeout_ms.unwrap_or(0) == stock.resume_wait_timeout_ms.unwrap_or(0)
        && user.install_hint == stock.install_hint
        && user.forward_scroll == stock.forward_scroll
        && user.web_dragdrop_paste == stock.web_dragdrop_paste
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(raw: &str) -> DocumentMut {
        raw.parse().expect("parse toml")
    }

    #[test]
    fn migrates_a_non_loopback_server_bind_to_host_and_port() {
        let mut d = doc("[server]\nbind = \"0.0.0.0:9000\"\n");
        assert!(apply_load_migrations(&mut d).expect("migrate"));
        assert_eq!(d["server"]["host"].as_str(), Some("0.0.0.0"));
        assert_eq!(d["server"]["port"].as_integer(), Some(9000));
        assert!(d.get("server").and_then(|s| s.get("bind")).is_none());
    }

    #[test]
    fn drops_a_loopback_server_bind_silently() {
        let mut d = doc("[server]\nbind = \"127.0.0.1:9000\"\n");
        assert!(apply_load_migrations(&mut d).expect("migrate"));
        // Loopback bind is dropped and NOT carried into host/port.
        assert!(d.get("server").and_then(|s| s.get("host")).is_none());
        assert!(d.get("server").and_then(|s| s.get("bind")).is_none());
    }

    #[test]
    fn migrates_the_legacy_tailscale_boolean_to_the_tri_state() {
        // true means "bind it" -> yes; false means "do not" -> no. Neither says
        // "keep watching", so neither becomes auto.
        let mut on = doc("[server]\ntailscale_enabled = true\n");
        assert!(apply_load_migrations(&mut on).expect("migrate"));
        assert_eq!(on["server"]["tailscale"].as_str(), Some("yes"));
        assert!(
            on.get("server")
                .and_then(|s| s.get("tailscale_enabled"))
                .is_none(),
            "the legacy key must be removed"
        );

        let mut off = doc("[server]\ntailscale_enabled = false\n");
        assert!(apply_load_migrations(&mut off).expect("migrate"));
        assert_eq!(off["server"]["tailscale"].as_str(), Some("no"));
    }

    #[test]
    fn the_new_tailscale_key_wins_when_both_keys_are_present() {
        // A file carrying both has already said what it means with the tri-state;
        // the leftover boolean is an old line, not a second opinion. It is
        // removed, and the tri-state value is untouched.
        let mut d = doc("[server]\ntailscale = \"auto\"\ntailscale_enabled = false\n");
        assert!(apply_load_migrations(&mut d).expect("migrate"));
        assert_eq!(d["server"]["tailscale"].as_str(), Some("auto"));
        assert!(
            d.get("server")
                .and_then(|s| s.get("tailscale_enabled"))
                .is_none(),
            "the legacy key must be removed even when it lost"
        );
    }

    #[test]
    fn a_non_boolean_tailscale_enabled_is_dropped_without_failing_the_load() {
        // The key no longer exists, so there is nothing to be strict about: drop
        // it and let the new key's own default answer.
        let mut d = doc("[server]\ntailscale_enabled = \"sure\"\n");
        assert!(apply_load_migrations(&mut d).expect("migrate"));
        assert!(
            d.get("server").and_then(|s| s.get("tailscale")).is_none(),
            "nothing is invented from a value dux cannot read"
        );
        assert!(
            d.get("server")
                .and_then(|s| s.get("tailscale_enabled"))
                .is_none()
        );
    }

    #[test]
    fn prunes_the_untouched_stock_gemini_block() {
        let mut d = doc(
            "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\ninstall_hint = \"brew install gemini-cli\"\n",
        );
        assert!(apply_load_migrations(&mut d).expect("migrate"));
        assert!(d.get("providers").and_then(|p| p.get("gemini")).is_none());
    }

    #[test]
    fn keeps_a_customized_gemini_block() {
        let mut d = doc(
            "[providers.gemini]\ncommand = \"/opt/my-gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\n",
        );
        // Only the customized block is present; nothing else to migrate.
        assert!(!apply_load_migrations(&mut d).expect("migrate"));
        assert!(d.get("providers").and_then(|p| p.get("gemini")).is_some());
    }

    #[test]
    fn no_migrations_leaves_the_document_unchanged() {
        let mut d = doc("[server]\nhost = \"127.0.0.1\"\nport = 8080\n");
        assert!(!apply_load_migrations(&mut d).expect("migrate"));
    }
}
