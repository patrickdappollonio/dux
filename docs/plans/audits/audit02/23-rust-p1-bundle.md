# Phase 23: Rust P1 hygiene bundle

> Maps to: **P1-I** (`--` separator in git args), **P1-J** (NamedTempFile + 0600), **P1-K** (ensure_column allowlist), **P1-L** (serial_test for env-var tests), **P1-M** (petname fallback).

## Goal
Five small Rust hygiene fixes; bundle in one PR.

## Pre-conditions
- Phase 00 baseline green.
- Phase 19 (schema versioning) merged — P1-K is the deprecation step
  for `ensure_column`.

## Files to touch
- `src/git.rs` — P1-I, P1-M.
- `src/provider.rs` — P1-J.
- `src/storage.rs` — P1-K.
- `src/config.rs` — P1-L.
- `Cargo.toml` — add `serial_test` dev-dep.

## Steps

### 23.1 — P1-I: `--` in git arg passing
For every `Command::new("git")` chain that takes user-controlled
positional args (branch names, refspecs, paths from worktrees), insert
`--` before the user-controlled arg:
```rust
// Before
.args(["switch", branch_name])
// After
.args(["switch", "--", branch_name])
```
Sites (audit02 P1-I lists these):
- `src/git.rs:206-225` (`create_worktree_existing_branch`)
- `src/git.rs:261-273` (`create_worktree_from_start_point`)
- `src/git.rs:765-783` (`rename_branch`)
- `src/git.rs:144-159` (`switch_branch`)
- `src/git.rs:165-200` (`branch_exists`)

Read each fn before patching — some may already use `--` or have
non-positional args where it doesn't belong.

For `git branch -m -- old new`, verify the syntax — git accepts
`branch -m -- old new` since 2.x.

### 23.2 — P1-J: NamedTempFile + 0600 in provider.rs
`src/provider.rs:23-32, :49-66`. Replace `tempfile::tempdir`+`std::fs::File::create`
with:
```rust
use std::io::Write;
use tempfile::Builder;

let mut tmpfile = Builder::new()
    .prefix(&format!("dux-{name}-"))
    .suffix(".txt")
    .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))
    .tempfile()?;
tmpfile.write_all(prompt.as_bytes())?;
tmpfile.flush()?;
let path = tmpfile.path().to_owned();
// ... cmd.output() ...
// On any return path, `tmpfile` Drop unlinks. No manual remove_file needed.
```
Drop the `let _ = std::fs::remove_file(&path);` — `NamedTempFile`
handles it; this also fixes the leak on `read_to_string` error.

Additionally: replace nanos-based name (P2-12) with `Uuid::new_v4()`:
```rust
let prefix = format!("dux-{name}-{}", uuid::Uuid::new_v4().simple());
```

### 23.3 — P1-K: `ensure_column` allowlist
`src/storage.rs:431-443`. After Phase 19 makes `ensure_column`
deprecated, also tighten its inputs:
```rust
fn ensure_column(conn: &Connection, table: &str, column: &str, sql_type: &str) -> Result<()> {
    fn is_safe_ident(s: &str) -> bool {
        !s.is_empty()
            && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn is_safe_sql_type(s: &str) -> bool {
        const ALLOWED: &[&str] = &["TEXT", "INTEGER", "REAL", "BLOB", "NULL"];
        let upper = s.to_ascii_uppercase();
        ALLOWED.iter().any(|t| upper == *t || upper.starts_with(&format!("{t} ")))
    }
    if !is_safe_ident(table) || !is_safe_ident(column) || !is_safe_sql_type(sql_type) {
        anyhow::bail!("ensure_column: rejected unsafe inputs ({table}, {column}, {sql_type})");
    }
    // ... existing logic ...
}
```
Add `#[deprecated(note = "schema changes should go through src/storage/migrations/ — see Phase 19")]`.

### 23.4 — P1-L: serial_test for env-var tests
`Cargo.toml`:
```toml
[dev-dependencies]
serial_test = "3"
```

`src/config.rs:2775-2814` env-var tests:
```rust
use serial_test::serial;

#[test]
#[serial]
fn test_var_expansion_1() {
    unsafe { std::env::set_var("DUX_TEST_VAR_1", "abc"); }
    // ...
    unsafe { std::env::remove_var("DUX_TEST_VAR_1"); }
}
```
The `#[serial]` attribute serializes all marked tests; no parallel
race on global env.

### 23.5 — P1-M: petname fallback
`src/git.rs:786`. Replace `petname::petname(2,"-").expect("...")`:
```rust
fn random_agent_name() -> String {
    petname::petname(2, "-").unwrap_or_else(|| {
        format!("agent-{}", uuid::Uuid::new_v4().simple())
    })
}
```

### 23.6 — Tests
- `cargo test` should still be green; add no new tests for these
  hygiene fixes unless behavior changes (`is_safe_ident` deserves a
  unit test).
- Add to `src/storage.rs`:
  ```rust
  #[test]
  fn ensure_column_rejects_injection() {
      // Manufacture in-memory connection
      let conn = Connection::open_in_memory().unwrap();
      conn.execute_batch("CREATE TABLE t (id INTEGER);").unwrap();
      assert!(ensure_column(&conn, "t", "x; DROP TABLE t; --", "TEXT").is_err());
      assert!(ensure_column(&conn, "t", "x", "TEXT; DROP TABLE t; --").is_err());
      assert!(ensure_column(&conn, "t", "ok_name", "TEXT").is_ok());
  }
  ```

## Validation
- `cargo test` green.
- `cargo clippy --all-targets --all-features -- -D warnings` green.
- `cargo +nightly miri test` (optional) — sanity check no UB introduced.

## Acceptance criteria
- [x] All git invocation sites with user-controlled positional args
      use `--`.
- [x] `provider.rs` uses `NamedTempFile` (mode 0600); no `remove_file` calls.
- [x] `ensure_column` allowlists table/column/sql_type; rejects unsafe.
- [x] `serial_test = "3"` in dev-deps; env-var tests marked `#[serial]`.
- [x] `petname` panic replaced with `unwrap_or_else(...)` (`src/git.rs:742`).
- [x] New `ensure_column_rejects_injection` test passes.
- [x] PR: `chore(rust): P1 hygiene bundle (P1-I/J/K/L/M)` — landed via PR #2.

## Known pitfalls
- The `--` separator must not be added to git invocations that don't
  accept it (e.g., `git config`). Audit each site individually.
- `tempfile::Builder::permissions` may behave differently on Windows
  (we're WSL2-only, but the API is cross-platform).
- `ensure_column` test must not actually leak SQL — use
  `Connection::open_in_memory()`.
- `serial_test` adds a small CI overhead; one mutex per `#[serial]`
  group. Acceptable.

## References
- audit02 P1-I, P1-J, P1-K, P1-L, P1-M.
