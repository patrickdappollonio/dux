//! Config-to-SQLite project reconciliation, the core-owned application of the
//! "config wins for explicit project preferences" tenet. Runs at every
//! entrypoint's startup (the TUI bootstrap AND the headless `dux serve`
//! bootstrap) and on config reload, so config-only `[[projects]]`, config-edited
//! preferences, and identity-conflict validation are honored regardless of
//! surface.
//!
//! Split of responsibility: the DECISION (validate identity, merge per field,
//! adopt config-only projects, write store-only projects back as portable
//! config) lives here; PERSISTING the reconciled config to disk is a surface
//! concern (the TUI renders the full commented template, the web a plain one),
//! so the driver takes a `persist` hook rather than hardcoding a renderer.
//!
//! Tenets honored (see CLAUDE.md):
//! - Config wins for the safe explicit preference fields (`name`,
//!   `default_provider`, `auto_reopen_agents`, `startup_command`, `env`); a
//!   missing config field is filled from SQLite instead.
//! - Runtime-derived git/agent state is never written into config; in
//!   particular saved config OMITS `leading_branch` (it may be parsed from an
//!   older config to repair SQLite, but reconciled config drops it).
//! - Hard conflicts are reserved for identity ambiguity: duplicate ids/paths
//!   within one store, the same id pointing at different expanded paths, or the
//!   same expanded path under different ids.

use anyhow::Result;

use crate::config::{Config, ProjectConfig, expand_path};
use crate::storage::SessionStore;

/// Reconcile the config's `[[projects]]` with the SQLite store, in place.
///
/// Applies the "config wins" tenet: for each project present in BOTH stores the
/// safe preference fields are merged (config value wins, else SQLite fills the
/// gap); a config-only project is adopted into SQLite; a store-only project is
/// written back into config in portable form. `persist(config)` is called to
/// save the config to disk whenever the reconciled config changed (or the store
/// was repaired from config-only projects, to keep the on-disk config
/// normalized). Identity conflicts within either store, or across the two, abort
/// with a descriptive error and no writes.
pub fn reconcile_config_projects<P>(
    config: &mut Config,
    session_store: &SessionStore,
    mut persist: P,
) -> Result<()>
where
    P: FnMut(&Config) -> Result<()>,
{
    validate_project_records("config.toml", &config.projects)?;
    let mut stored = session_store.load_projects()?;
    validate_project_records("SQLite", &stored)?;

    let mut changed_config = false;
    let mut changed_store = false;
    let mut merged = config.projects.clone();

    for (index, cfg_project) in config.projects.iter().enumerate() {
        match stored.iter().position(|stored_project| {
            stored_project.id == cfg_project.id
                || same_expanded_project_path(stored_project, cfg_project)
        }) {
            Some(stored_index) => {
                let stored_project = &stored[stored_index];
                let (merged_config_project, merged_stored_project) =
                    merge_project_records(cfg_project, stored_project)?;
                if &merged_config_project != cfg_project {
                    merged[index] = merged_config_project;
                    changed_config = true;
                }
                if &merged_stored_project != stored_project {
                    session_store.upsert_project_at(&merged_stored_project, stored_index as i64)?;
                    stored[stored_index] = merged_stored_project;
                    changed_store = true;
                }
            }
            None => {
                session_store.upsert_project_at(cfg_project, index as i64)?;
                changed_store = true;
                stored.push(cfg_project.clone());
            }
        }
    }

    for stored_project in stored {
        let exists = merged.iter().any(|cfg_project| {
            cfg_project.id == stored_project.id
                || same_expanded_project_path(cfg_project, &stored_project)
        });
        if !exists {
            let mut portable = stored_project;
            portable.path = portable_project_path(&portable.path);
            merged.push(portable);
            changed_config = true;
        }
    }

    if changed_config || changed_store {
        // Keep the on-disk config normalized after adopting config-only projects
        // into the store OR editing a config-authoritative field.
        config.projects = merged;
        persist(config)?;
    }
    Ok(())
}

/// Reject identity-ambiguous project records within one store BEFORE
/// reconciliation touches anything: two entries with the same id, or two entries
/// whose expanded paths collide. `source` names the store ("config.toml" or
/// "SQLite") for the error.
pub fn validate_project_records(source: &str, projects: &[ProjectConfig]) -> Result<()> {
    for (index, project) in projects.iter().enumerate() {
        for other in projects.iter().skip(index + 1) {
            if project.id == other.id {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project id \"{}\". Remove or rename one [[projects]] entry, then restart dux.",
                    project.id
                );
            }
            if same_expanded_project_path(project, other) {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project path \"{}\". Remove one duplicate project entry, then restart dux.",
                    expanded_project_path(project).unwrap_or_else(|| project.path.clone())
                );
            }
        }
    }
    Ok(())
}

/// Merge a config project with its matching stored project, returning the
/// reconciled (config, stored) pair. Config wins for the safe preference fields
/// (a present config value overwrites the stored one; a missing config value is
/// filled from the store); `leading_branch` is repaired INTO SQLite from config
/// but DROPPED from the reconciled config (runtime-derived state never persists
/// back into portable config); `env` is config-authoritative wholesale. A
/// same-id/different-path or same-path/different-id mismatch is an identity
/// conflict and aborts.
pub fn merge_project_records(
    config_project: &ProjectConfig,
    stored_project: &ProjectConfig,
) -> Result<(ProjectConfig, ProjectConfig)> {
    let config_path = expanded_project_path(config_project);
    let stored_path = expanded_project_path(stored_project);
    if config_project.id == stored_project.id && config_path != stored_path {
        anyhow::bail!(
            "Project sync conflict for id \"{}\": config.toml points to \"{}\" but SQLite points to \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_project.id,
            config_project.path,
            stored_project.path
        );
    }
    if config_path == stored_path && config_project.id != stored_project.id {
        anyhow::bail!(
            "Project sync conflict for path \"{}\": config.toml uses id \"{}\" but SQLite uses id \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_path.unwrap_or_else(|| config_project.path.clone()),
            config_project.id,
            stored_project.id
        );
    }

    let mut merged_config = config_project.clone();
    let mut merged_stored = stored_project.clone();

    sync_config_authoritative_project_field(&mut merged_config.name, &mut merged_stored.name);
    sync_config_authoritative_project_field(
        &mut merged_config.default_provider,
        &mut merged_stored.default_provider,
    );
    if merged_stored.leading_branch.is_none() {
        merged_stored.leading_branch = merged_config.leading_branch.clone();
    }
    // Runtime-derived: never persist a leading branch back into portable config.
    merged_config.leading_branch = None;
    sync_config_authoritative_project_field(
        &mut merged_config.startup_command,
        &mut merged_stored.startup_command,
    );
    sync_config_authoritative_project_field(
        &mut merged_config.auto_reopen_agents,
        &mut merged_stored.auto_reopen_agents,
    );
    merged_stored.env = merged_config.env.clone();

    Ok((merged_config, merged_stored))
}

/// Config wins: a present config value is copied into the store; a missing
/// config value is filled from the store (so SQLite fills MISSING config fields).
fn sync_config_authoritative_project_field<T>(
    config_value: &mut Option<T>,
    stored_value: &mut Option<T>,
) where
    T: Clone,
{
    match config_value.as_ref() {
        Some(config) => {
            *stored_value = Some(config.clone());
        }
        None => {
            *config_value = stored_value.clone();
        }
    }
}

/// Whether two project records resolve to the same expanded filesystem path.
pub fn same_expanded_project_path(left: &ProjectConfig, right: &ProjectConfig) -> bool {
    expanded_project_path(left).is_some_and(|left_path| {
        expanded_project_path(right).is_some_and(|right_path| left_path == right_path)
    })
}

/// The project's `path` with env/tilde expansion applied, or `None` when it does
/// not resolve to a valid path.
pub fn expanded_project_path(project: &ProjectConfig) -> Option<String> {
    expand_path(&project.path)
}

/// Re-portabilize an absolute path under `$HOME` so a store-only project written
/// back into config stays portable across machines. Falls back to the literal
/// path when it is not under the home directory (or home cannot be resolved).
pub fn portable_project_path(path: &str) -> String {
    let Some(home) = home::home_dir() else {
        return path.to_string();
    };
    let path_buf = std::path::Path::new(path);
    if let Ok(relative) = path_buf.strip_prefix(&home) {
        let relative = relative.to_string_lossy();
        if relative.is_empty() {
            "$HOME".to_string()
        } else {
            format!("$HOME/{}", relative)
        }
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionStore {
        SessionStore::open(std::path::Path::new(":memory:")).expect("in-memory store")
    }

    fn project(id: &str, path: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.to_string(),
            path: path.to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        }
    }

    #[test]
    fn a_config_only_project_is_adopted_into_sqlite() {
        let store = store();
        let mut config = Config {
            projects: vec![project("p1", "/tmp/p1")],
            ..Config::default()
        };
        let mut saved = 0;
        reconcile_config_projects(&mut config, &store, |_| {
            saved += 1;
            Ok(())
        })
        .expect("reconcile ok");

        let stored = store.load_projects().expect("load");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "p1");
        assert!(
            saved >= 1,
            "config normalized to disk after adopting the project"
        );
    }

    #[test]
    fn a_config_value_updates_sqlite() {
        let store = store();
        // The store already knows p1 with a stale name.
        let mut in_store = project("p1", "/tmp/p1");
        in_store.name = Some("old".to_string());
        store.upsert_project_at(&in_store, 0).unwrap();

        // Config sets an explicit newer name: config wins into SQLite.
        let mut cfg_project = project("p1", "/tmp/p1");
        cfg_project.name = Some("new".to_string());
        let mut config = Config {
            projects: vec![cfg_project],
            ..Config::default()
        };
        reconcile_config_projects(&mut config, &store, |_| Ok(())).expect("reconcile ok");

        let stored = store.load_projects().expect("load");
        assert_eq!(stored[0].name.as_deref(), Some("new"));
    }

    #[test]
    fn sqlite_fills_a_missing_config_field() {
        let store = store();
        let mut in_store = project("p1", "/tmp/p1");
        in_store.default_provider = Some("codex".to_string());
        store.upsert_project_at(&in_store, 0).unwrap();

        // Config omits default_provider: SQLite fills it into the config.
        let mut config = Config {
            projects: vec![project("p1", "/tmp/p1")],
            ..Config::default()
        };
        reconcile_config_projects(&mut config, &store, |_| Ok(())).expect("reconcile ok");

        assert_eq!(
            config.projects[0].default_provider.as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn merging_repairs_leading_branch_into_sqlite_but_drops_it_from_config() {
        // A MATCHED project (present in both stores) goes through the merge: the
        // config's leading_branch repairs the store's missing one, and is then
        // dropped from the in-memory config (runtime-derived state never persists
        // back into portable config; the renderer also omits it, but the merge is
        // where the in-memory drop happens). A config-only project keeps its
        // in-memory leading_branch and relies on the renderer to omit it.
        let store = store();
        store
            .upsert_project_at(&project("p1", "/tmp/p1"), 0)
            .unwrap();
        let mut cfg_project = project("p1", "/tmp/p1");
        cfg_project.leading_branch = Some("main".to_string());
        let mut config = Config {
            projects: vec![cfg_project],
            ..Config::default()
        };
        reconcile_config_projects(&mut config, &store, |_| Ok(())).expect("reconcile ok");

        assert_eq!(config.projects[0].leading_branch, None);
        let stored = store.load_projects().expect("load");
        assert_eq!(stored[0].leading_branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_store_only_project_is_written_back_into_config() {
        let store = store();
        store
            .upsert_project_at(&project("p2", "/tmp/p2"), 0)
            .unwrap();
        let mut config = Config {
            projects: Vec::new(),
            ..Config::default()
        };
        reconcile_config_projects(&mut config, &store, |_| Ok(())).expect("reconcile ok");
        assert_eq!(config.projects.len(), 1);
        assert_eq!(config.projects[0].id, "p2");
    }

    #[test]
    fn duplicate_config_ids_are_a_conflict() {
        let store = store();
        let mut config = Config {
            projects: vec![project("dup", "/tmp/a"), project("dup", "/tmp/b")],
            ..Config::default()
        };
        let err = reconcile_config_projects(&mut config, &store, |_| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate project id"), "{err}");
    }

    #[test]
    fn same_id_different_path_across_stores_is_a_conflict() {
        let store = store();
        store
            .upsert_project_at(&project("p1", "/tmp/here"), 0)
            .unwrap();
        let mut config = Config {
            projects: vec![project("p1", "/tmp/there")],
            ..Config::default()
        };
        let err = reconcile_config_projects(&mut config, &store, |_| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("config.toml points to"), "{err}");
    }

    #[test]
    fn same_path_different_id_across_stores_is_a_conflict() {
        let store = store();
        store
            .upsert_project_at(&project("stored-id", "/tmp/same"), 0)
            .unwrap();
        let mut config = Config {
            projects: vec![project("config-id", "/tmp/same")],
            ..Config::default()
        };
        let err = reconcile_config_projects(&mut config, &store, |_| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("config.toml uses id"), "{err}");
    }
}
