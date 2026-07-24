//! Load-time config migrations, applied by `config::load_config` IN MEMORY at
//! EVERY entrypoint (the TUI, `dux server`, and the web bootstrap). Two kinds:
//!
//! - Deprecated-key migrations: an old key is rewritten to its replacement
//!   (`[server] bind` -> host/port, `[defaults] prompt_for_name` -> the inverse
//!   `enable_randomized_pet_name_by_default`). The migrated key configures the
//!   SERVER itself, so applying it only in the TUI's `ensure_config` silently
//!   dropped a non-loopback `bind` under `dux serve`; applying it in
//!   `load_config` fixes that.
//! - Retired-provider pruning: an untouched stock block for a provider dux no
//!   longer ships (gemini) is removed so its picker stops offering it. A
//!   user-CUSTOMIZED block of the same name is preserved (config wins for
//!   explicit preferences).
//!
//! These operate on a `toml_edit::DocumentMut` and return whether the document
//! changed. The TUI ADDITIONALLY persists the migrated document to disk (a
//! surface concern); `load_config` only applies the result in memory.

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
    },
    DeprecatedConfigKeyRule {
        old: DeprecatedConfigKey {
            section: "server",
            key: "bind",
        },
        action: DeprecatedConfigKeyAction::Replace {
            migrate: migrate_server_bind,
        },
    },
];

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
        // Not a valid IP:port -- nothing safe to migrate; let the new defaults
        // apply. (An invalid bind would have failed the resolver anyway.)
        //
        // NOTE this also drops a hostname `bind` (e.g. "localhost:9000"):
        // `SocketAddr` only parses literal IP:port. That is NOT a regression --
        // the OLD resolver also parsed `bind` with `SocketAddr::from_str` and
        // rejected hostnames (no DNS), so a hostname bind never worked.
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

// ---------------------------------------------------------------------------
// Retired providers
//
// A retired provider once shipped as a default but no longer does. It is no
// longer rendered into new configs and no longer re-added by
// `ProvidersConfig::ensure_defaults`. So existing users do not keep a dead stock
// block forever, an untouched stock block is pruned from their config on load. A
// user who customized the block (or added one back later) keeps it -- config
// wins for explicit preferences.
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
