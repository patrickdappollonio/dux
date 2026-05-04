# Phase 19: Schema versioning — `user_version` + migration log

> Maps to: **P1-Y**.

## Goal
Today `storage.rs::ensure_column` adds nullable columns ad hoc with no
version tracking; `Config` has no `schema_version` field. The "add
nullable forever" pattern is painful at v2.0 scale and prevents safe
backwards-incompatible changes.

## Pre-conditions
- Phase 14 (sqlite hardening) merged — uses the same connection-open
  path.
- Phase 18 (session state machine) merged — its persisted-format
  change is migration #2.

## Files to touch
- `src/storage.rs` — `migrate()` rewrite.
- `src/storage/migrations/` — NEW dir, one `.sql` or `.rs` per migration.
- `src/config.rs` — `Config::schema_version` field; `migrate_config()`.
- `tests/storage_migrations.rs` — NEW.

## Steps

### 19.1 — sqlite migrations
Use `PRAGMA user_version` as the canonical schema version. Migrations
are applied in order; each increments `user_version`.

```rust
// src/storage.rs
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("migrations/0001_initial_schema.sql")),
    (2, include_str!("migrations/0002_session_state_v2.sql")),
    // (Add new migrations here, never edit existing ones.)
];

fn migrate(conn: &Connection) -> Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0))?;
    for (version, sql) in MIGRATIONS {
        if *version > current {
            conn.execute_batch(sql)?;
            conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
            tracing::info!(target: "dux::storage", version, "migration applied");
        }
    }
    Ok(())
}
```

### 19.2 — First migration captures the existing schema
Run on a current dux DB:
```bash
sqlite3 ~/.dux/sessions.sqlite3 .schema > docs/plans/audits/audit02/artifacts/19-schema.sql
```
Edit to:
- Wrap each `CREATE TABLE` with `IF NOT EXISTS`.
- Drop any `INSERT` data lines.
- Save as `src/storage/migrations/0001_initial_schema.sql`.

### 19.3 — Second migration: SessionState v2 (Phase 18)
```sql
-- 0002_session_state_v2.sql
ALTER TABLE agent_sessions ADD COLUMN state_json TEXT;
UPDATE agent_sessions SET state_json = CASE
  WHEN status = 'Active'   THEN '{"v":1,"kind":"detached","detached_at":"' || COALESCE(updated_at, datetime('now')) || '"}'
  WHEN status = 'Detached' THEN '{"v":1,"kind":"detached","detached_at":"' || COALESCE(updated_at, datetime('now')) || '"}'
  ELSE                          '{"v":1,"kind":"exited","exit_code":null,"exited_at":"' || COALESCE(updated_at, datetime('now')) || '"}'
END;
-- Keep `status` column for one release; drop in 0003.
```

### 19.4 — Replace `ensure_column`
After all migrations apply, the schema is canonical. `ensure_column`
becomes a code smell; new fields must come via a numbered migration.
Mark `ensure_column` `#[deprecated]` and route any remaining callers
to migration files.

### 19.5 — Config schema versioning
```rust
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Config {
    /// Internal schema version. Do not edit by hand — set by `dux config regenerate`.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    // ... existing fields ...
}
fn default_schema_version() -> u32 { 1 }

const CONFIG_SCHEMA_CURRENT: u32 = 2;

fn migrate_config(mut c: Config) -> Config {
    while c.schema_version < CONFIG_SCHEMA_CURRENT {
        match c.schema_version {
            0 | 1 => {
                // 1 -> 2: introduce limits + auto_resume sections (Phases 15, 16)
                if c.limits.max_panes == 0 { c.limits.max_panes = 16; }
                c.schema_version = 2;
            }
            _ => break,
        }
    }
    c
}
```
On `Config::load`, call `migrate_config` after deserialization. Save
the migrated form back to disk so the next launch is no-op.

### 19.6 — Tests
```rust
#[test]
fn migrate_from_empty_db_runs_all_migrations() { ... }

#[test]
fn migrate_idempotent() {
    let conn = open_in_memory();
    migrate(&conn).unwrap();
    let v: u32 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0)).unwrap();
    migrate(&conn).unwrap();    // second run no-op
    let v2: u32 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0)).unwrap();
    assert_eq!(v, v2);
}

#[test]
fn config_v0_loads_and_migrates_to_current() {
    let toml = "schema_version = 0\n# ... old format ...\n";
    let c: Config = toml::from_str(toml).unwrap();
    let migrated = migrate_config(c);
    assert_eq!(migrated.schema_version, CONFIG_SCHEMA_CURRENT);
}
```

### 19.7 — Backwards-compat policy doc
Add `docs/contributing/schema-policy.md`:
- New columns: always nullable; never reuse a name.
- Renames: never; introduce a new column, deprecate the old.
- Drops: only after one full release with the deprecation log line.
- Config TOML: backwards-compat for at least 1 minor version; bump
  `CONFIG_SCHEMA_CURRENT` and add a migration arm.

## Validation
- `cargo test storage_migrations` green.
- Spin up dux against a backup of an old DB; observe migrations apply.
- `dux config diff` against the current `config.toml` shows no
  unexpected changes after migration.

## Acceptance criteria
- [x] `MIGRATIONS` slice with at least 2 entries (`0001_initial_schema.sql`, `0002_session_state_v2.sql`).
- [x] `PRAGMA user_version` set after each migration.
- [x] `ensure_column` deprecated; new schema additions go via files.
- [x] `Config::schema_version` field + migration ladder (`migrate_config`).
- [x] `migrate_config` called from `Config::load`.
- [x] `docs/contributing/schema-policy.md` written.
- [x] 3 tests pass (`tests/storage_migrations.rs`).
- [x] PR: `feat(storage): explicit schema versioning (P1-Y)` — landed via PR #2.

## Known pitfalls
- Never edit a previously-committed migration. If you need to fix a
  bug introduced in migration N, write migration N+1 that corrects
  the data. Treating migrations as immutable is mandatory.
- Migrations execute inside one transaction by default; if they're
  long, surface progress (rare for dux's data sizes).
- `PRAGMA user_version` is per-database and survives backups, so
  `sessions.sqlite3.bak` (Phase 14) inherits the version automatically.
- Test on a copy of a real production-ish DB — schema-only synthetic
  fixtures miss "real data shape" surprises.

## References
- audit02 P1-Y.
- SQLite `PRAGMA user_version`: https://sqlite.org/pragma.html#pragma_user_version
- Embedded migration patterns (Diesel, sqlx, refinery) — for inspiration.
