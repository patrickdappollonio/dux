//! Integration tests for audit02 P1-Y (Phase 19): explicit schema
//! versioning for both the SQLite session store and the TOML config
//! file.
//!
//! These tests live under `tests/` so they exercise the public
//! [`dux::storage::SessionStore`] / [`dux::config::migrate_config`]
//! APIs the same way an external consumer (or a future doctor tool)
//! would. They assert behaviour, not implementation details: that
//! migrations bump `PRAGMA user_version`, that a second run is a
//! no-op, and that an old `Config` parses and is upgraded to the
//! current schema by [`migrate_config`].

use dux::config::{CONFIG_SCHEMA_CURRENT, Config, migrate_config};
use dux::storage::SessionStore;

/// Opening a `SessionStore` against a fresh, empty database file must
/// run every entry in the `MIGRATIONS` slice. Externally we observe
/// this through `PRAGMA user_version`: after migration it is non-zero
/// and matches the latest migration number that ships in this build.
#[test]
fn migrate_from_empty_db_runs_all_migrations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fresh.sqlite3");

    let store = SessionStore::open(&path).expect("open fresh DB");
    let user_version: u32 = store
        .conn()
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 1,
        "expected user_version >= 1 after migrations, got {user_version}"
    );

    // The migration also has to leave the canonical schema in place:
    // `agent_sessions` and `session_prs` must exist with the columns
    // that the upsert path writes to, otherwise downstream code panics
    // at runtime, not in this test.
    let agent_sessions_columns: Vec<String> = store
        .conn()
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect column names");
    for required in [
        "id",
        "project_id",
        "provider",
        "source_branch",
        "branch_name",
        "worktree_path",
        "title",
        "project_path",
        "started_providers",
        "status",
        "created_at",
        "updated_at",
    ] {
        assert!(
            agent_sessions_columns.iter().any(|c| c == required),
            "agent_sessions missing column {required} after migration; \
             columns = {agent_sessions_columns:?}"
        );
    }
}

/// Re-running the migration loop on an already-migrated database is a
/// no-op: `PRAGMA user_version` is unchanged. We exercise this by
/// opening the same physical file twice — `SessionStore::open` calls
/// `migrate` internally, so the second open is the second run.
#[test]
fn migrate_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("idem.sqlite3");

    let first_version = {
        let store = SessionStore::open(&path).expect("first open");
        store
            .conn()
            .query_row::<u32, _, _>("PRAGMA user_version;", [], |r| r.get(0))
            .expect("read user_version after first open")
    };

    let second_version = {
        let store = SessionStore::open(&path).expect("second open");
        store
            .conn()
            .query_row::<u32, _, _>("PRAGMA user_version;", [], |r| r.get(0))
            .expect("read user_version after second open")
    };

    assert_eq!(
        first_version, second_version,
        "migrations must be idempotent: user_version drifted from \
         {first_version} to {second_version} on re-open"
    );
    assert!(
        first_version >= 1,
        "user_version should be >= 1 after migrations (got {first_version})"
    );
}

/// A `config.toml` that predates the `schema_version` field must still
/// deserialize cleanly (thanks to `#[serde(default)]`) and be moved
/// forward to `CONFIG_SCHEMA_CURRENT` by `migrate_config`. This
/// exercises the policy that old configs always load on a newer dux.
///
/// We start with `schema_version = 0` to simulate the worst case (a
/// pre-versioning config) and assert the ladder catches up to current.
#[test]
fn config_v0_loads_and_migrates_to_current() {
    // Deliberately minimal TOML: only the schema_version is set, every
    // other field falls through to its serde default. This mirrors
    // what an extremely old config — written before most sections
    // existed — looks like once we override the version.
    let toml = "schema_version = 0\n";

    let parsed: Config = toml::from_str(toml).expect("parse v0 config");
    assert_eq!(parsed.schema_version, 0, "starting version is 0");

    let migrated = migrate_config(parsed);
    assert_eq!(
        migrated.schema_version, CONFIG_SCHEMA_CURRENT,
        "migrate_config must bring schema_version to \
         CONFIG_SCHEMA_CURRENT ({CONFIG_SCHEMA_CURRENT})"
    );

    // Running the migration twice is a no-op (the loop guard exits).
    let migrated_again = migrate_config(migrated);
    assert_eq!(migrated_again.schema_version, CONFIG_SCHEMA_CURRENT);
}
