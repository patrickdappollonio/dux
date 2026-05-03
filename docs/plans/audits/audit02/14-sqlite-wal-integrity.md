# Phase 14: sqlite WAL + integrity check + periodic backup

> Maps to: **P1-W**.

## Goal
Harden `sessions.sqlite3` against spot-VM preemption: enable WAL
journaling, run `PRAGMA integrity_check` on open, and create a
periodic `.backup` to `sessions.sqlite3.bak`. Today the DB opens with
the default rollback journal and no recovery path.

## Pre-conditions
- Phase 00 baseline green.
- Independent of other phases.

## Files to touch
- `src/storage.rs` — set PRAGMAs on `Connection::open`, add backup helper.
- `src/app/workers.rs` — periodic backup worker.
- `src/config.rs` — `[storage] backup_interval_minutes = 30` config.
- `tests/storage_integration.rs` — integrity assertions.

## Steps

### 14.1 — PRAGMAs on connection open
`src/storage.rs` — locate `Connection::open(...)` (audit02 references
`storage.rs:22`). Wrap:
```rust
fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 134217728;        -- 128 MiB
        PRAGMA wal_autocheckpoint = 1000;
        PRAGMA foreign_keys = ON;
    "#)?;
    // Integrity check on open. If corrupt, fail fast — operator fixes via backup.
    let mut stmt = conn.prepare("PRAGMA integrity_check;")?;
    let result: String = stmt.query_row([], |r| r.get(0))?;
    if result != "ok" {
        anyhow::bail!("sqlite integrity check failed: {result}; restore from {}.bak",
                      path.display());
    }
    Ok(conn)
}
```

### 14.2 — Periodic backup
`src/storage.rs`:
```rust
pub fn backup_to(&self, dst: &Path) -> Result<()> {
    let dst_conn = Connection::open(dst)?;
    self.conn.lock().expect("storage poisoned")
        .backup(rusqlite::DatabaseName::Main, &dst_conn, None)?;
    Ok(())
}
```
`src/app/workers.rs` — spawn a backup worker on app init:
```rust
pub fn spawn_backup_worker(storage: Arc<Storage>, paths: DuxPaths, interval: Duration) {
    std::thread::Builder::new().name("storage-backup".into()).spawn(move || {
        let dst = paths.root.join("sessions.sqlite3.bak");
        loop {
            std::thread::sleep(interval);
            match storage.backup_to(&dst) {
                Ok(()) => tracing::debug!(target: "dux::storage", path = %dst.display(), "backup ok"),
                Err(e) => tracing::warn!(target: "dux::storage", err = %e, "backup failed"),
            }
        }
    }).ok();
}
```
Use Phase 09's `tracing::*` macros if Phase 09 is merged; else
`crate::logger::*` shims.

### 14.3 — Config field
`src/config.rs`:
```rust
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StorageConfig {
    /// Minutes between automatic sessions.sqlite3 backups.
    /// Set to 0 to disable. Default: 30.
    #[serde(default = "default_backup_interval")]
    pub backup_interval_minutes: u32,
}
fn default_backup_interval() -> u32 { 30 }
```
Render in `[storage]` section of canonical config TOML.

### 14.4 — Tests
`tests/storage_integration.rs`:
```rust
#[test]
fn open_sets_wal_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.sqlite3");
    let storage = Storage::open(&path).unwrap();
    let mode: String = storage.conn().query_row("PRAGMA journal_mode;", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");
}
#[test]
fn integrity_check_failure_returns_error_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("corrupt.sqlite3");
    // Write bogus bytes
    std::fs::write(&path, b"not a sqlite db").unwrap();
    let result = Storage::open(&path);
    assert!(result.is_err());
}
#[test]
fn backup_to_produces_valid_db() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src.sqlite3");
    let dst = tmp.path().join("dst.sqlite3");
    let storage = Storage::open(&src).unwrap();
    storage.backup_to(&dst).unwrap();
    let _ = Storage::open(&dst).unwrap();   // can re-open
}
```

## Validation
- `cargo test storage_integration` green.
- Manual: launch dux, observe `sessions.sqlite3-wal` and `-shm` files
  appear next to `sessions.sqlite3`.
- After 30 min, verify `sessions.sqlite3.bak` exists and is non-empty.
- Simulate corruption: stop dux, `dd if=/dev/zero of=…/sessions.sqlite3 bs=1 count=10`,
  start dux — should refuse with the integrity-check error pointing at `.bak`.

## Acceptance criteria
- [ ] `storage.rs::open_connection` sets WAL + synchronous + integrity check.
- [ ] `Storage::backup_to(dst)` implemented using rusqlite's online
      backup API.
- [ ] `spawn_backup_worker` runs on app init when interval > 0.
- [ ] `StorageConfig::backup_interval_minutes` config field with
      sensible default (30).
- [ ] 3 integration tests pass.
- [ ] PR: `feat(storage): WAL + integrity check + periodic backup (P1-W)`.

## Known pitfalls
- WAL adds two extra files (`-wal`, `-shm`) — backup procedures must
  copy all three. The Online Backup API handles this automatically;
  hot-copy via `cp` does not.
- `PRAGMA mmap_size` improves read perf but on tiny VMs it's
  unnecessary. Default 128 MiB is fine; expose if users complain.
- `wal_autocheckpoint = 1000` checkpoints when WAL exceeds 1000 pages
  (~4 MB at default page size). For dux's write rate this is generous.
- Don't enable `synchronous = OFF` — risk of data loss on power-loss.
  `NORMAL` is the right balance.
- Backup `Connection::open` at the destination path will create a fresh
  empty DB if the file doesn't exist; that's intentional.

## References
- audit02 P1-W.
- SQLite WAL mode: https://www.sqlite.org/wal.html
- rusqlite Online Backup API: https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html#method.backup
