# Schema policy

dux persists user data to two places that have versioned schemas:

1. The SQLite database `sessions.sqlite3`, governed by the `MIGRATIONS`
   slice in `src/storage.rs` and the SQL files under
   `src/storage/migrations/`.
2. The TOML file `config.toml`, governed by the `schema_version` field
   on `Config` and the `migrate_config` ladder in `src/config.rs`.

Both follow the same forward-only, append-only contract: old data must
keep loading on a newer dux build, and migrations are written once and
never edited.

## SQLite schema (`PRAGMA user_version`)

### Adding a column

- Always nullable (or `DEFAULT`-ed). A new column on a populated table
  must not break inserts written by older dux builds during a staged
  rollout.
- Pick a fresh name. **Never reuse a column name** that has been
  dropped — even if the type matches — because backups taken before
  the drop may still carry the old data.

### Renaming a column

Don't. Introduce a new column with the new name, copy data over inside
the migration, deprecate the old column in code, and only drop it after
one full release where the deprecation log line has fired.

### Dropping a column or table

Allowed only after one full release in which:

- A deprecation log line fires whenever the field is read or written.
- The release notes call out the upcoming drop.

Then the next release's migration may `DROP COLUMN` / `DROP TABLE`. This
gives external tooling (backup scripts, dashboards) a window to stop
depending on the column.

### Writing a migration

1. Create `src/storage/migrations/000N_description.sql` with the next
   integer `N`. Do not skip numbers.
2. Add `(N, include_str!("storage/migrations/000N_description.sql"))`
   to the `MIGRATIONS` slice in `src/storage.rs`.
3. The migration runs inside an implicit SQLite transaction. After it
   succeeds, the runner sets `PRAGMA user_version = N`.
4. **Never edit a previously-committed migration.** If a bug shipped in
   migration `N`, write migration `N+1` that corrects the data. Even if
   the bug means migration `N` aborted on production databases, the fix
   goes in `N+1`; reusing `N` would skip over databases where it
   succeeded.
5. Add an integration test in `tests/storage_migrations.rs` that opens
   a fresh in-memory DB, runs `MIGRATIONS`, and asserts the resulting
   schema or data shape.

### Backups

`Storage::backup_to` (added in audit02 P1-W) uses the SQLite Online
Backup API, which preserves `PRAGMA user_version` byte-for-byte. A
`.bak` file therefore carries its source schema version automatically
and re-running the migration loop against a restored backup is safe.

### `ensure_column` is deprecated

The legacy `ensure_column` helper in `src/storage.rs` is
`#[deprecated]`. It is retained for one path only: legacy databases
created before `PRAGMA user_version` was wired up that may be missing
columns the canonical schema (`0001_initial_schema.sql`) declares. New
schema additions **must** go through a numbered migration.

## Config TOML (`schema_version`)

### Field-level rules

- New fields ride a `#[serde(default = …)]` so existing configs without
  the key continue to deserialize cleanly.
- Renames follow the same flow as SQL columns: introduce the new key,
  add a migration arm in `migrate_config` that copies and clears the
  old key, and drop the old key only after one release.
- Type changes (e.g. `u32` → `Option<u32>`) require a migration arm
  even when serde would accept the old shape — the migration is the
  audit trail.

### Bumping `CONFIG_SCHEMA_CURRENT`

When you add an arm to `migrate_config` that fills in new defaults or
rewrites an old key:

1. Increment `CONFIG_SCHEMA_CURRENT` in `src/config.rs`.
2. Add a `match` arm for the previous version that does the rewrite
   and bumps `c.schema_version` to the new value.
3. The `ensure_config` loader detects the bump and saves the migrated
   form back to `config.toml`, so the next launch is a no-op.
4. Add a test in `tests/storage_migrations.rs` that loads an old
   config and asserts the migrated shape.

### Backwards-compat window

dux reads configs whose `schema_version` is at least one minor release
behind the current version. Older configs still load, but a warning
log line points the user at `dux config regenerate` so they can adopt
the latest canonical shape with their values preserved.

### Configs from the future

If `schema_version` exceeds `CONFIG_SCHEMA_CURRENT` (e.g. the user
downgraded dux), `migrate_config` returns the value unchanged and the
loader logs a warning. It does **not** rewrite the file — preserving
the user's data is more important than the canonical shape on disk.

## Review checklist

When reviewing a PR that touches either schema:

- [ ] No edits to previously-committed migration files.
- [ ] Migration number is monotonically increasing.
- [ ] `MIGRATIONS` slice has the new entry in the right slot.
- [ ] `CONFIG_SCHEMA_CURRENT` bumped iff `migrate_config` gained an arm.
- [ ] New test in `tests/storage_migrations.rs` covers the migration.
- [ ] Renames or drops have a corresponding deprecation log line.
- [ ] Release notes call out drops at least one release ahead of time.
