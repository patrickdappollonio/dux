# Phase 05: pty.rs poison-tolerance + reader thread join + unsafe doc

> Maps to: **P0-E** (3 `.expect("terminal mutex poisoned")` panics), **P1-G** (reader thread leaked on `PtyClient::Drop`), **P1-H** (`unsafe BorrowedFd::borrow_raw` SAFETY relies on `#[allow(dead_code)]` field).

## Goal
Eliminate panic paths in the PTY rendering pipeline: a poisoned mutex on
the render path currently aborts the whole TUI, leaving the lockfile
flock'd and the operator's terminal in raw mode. Also: the reader thread
spawned at `PtyClient::spawn` is never joined on drop, leaking fds and
memory; and the `unsafe BorrowedFd::borrow_raw` is sound today only
because of an `#[allow(dead_code)]`-marked field a future cleanup could
remove.

## Pre-conditions
- Phase 00 baseline green.
- Independent of Phases 01–04.

## Files to touch
- `src/pty.rs` — three call sites + Drop + unsafe block.
- `tests/pty_integration.rs` — add a poison-recovery test (optional but valued).

## Steps

### 5.1 — Replace `.expect()` with `.lock().ok()` (P0-E)
At `src/pty.rs:277`, `:289`, `:295`:
```rust
// Before
let mut term = self.terminal.lock().expect("terminal mutex poisoned");

// After
let Ok(mut term) = self.terminal.lock() else {
    crate::logger::error("pty: terminal mutex poisoned; rendering empty snapshot");
    return /* sentinel: empty grid / Err / whatever the surrounding fn expects */;
};
```
The sentinel return type depends on the function. For `snapshot_into`
it's likely `Default::default()` or "no-op". Read each call site
before patching — don't blanket `?` something that's `()`.

Other lock sites in `pty.rs` already handle poison via `if let Ok(...)`
(`:413`, `:460-464`) — use those as the model.

### 5.2 — Join the reader thread on Drop (P1-G)
Today `PtyClient::spawn` calls `thread::spawn(reader_loop)` (line ~168)
and discards the `JoinHandle`. On `Drop` (line 509-513) the child is
killed, but the reader thread retains `Arc` clones until the master fd
closes — it eventually exits but the join never happens, fd accounting
drifts.

Fix:
```rust
pub struct PtyClient {
    // ... existing fields ...
    reader_handle: Option<std::thread::JoinHandle<()>>,  // NEW
    // ... existing master: Box<dyn MasterPty + Send>, etc.
}

impl PtyClient {
    pub fn spawn(...) -> anyhow::Result<Self> {
        // ... existing setup ...
        let reader_handle = std::thread::Builder::new()
            .name(format!("pty-reader-{}", session_label))
            .spawn(reader_loop)?;
        Ok(Self {
            // ...
            reader_handle: Some(reader_handle),
        })
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        // Kill child first so reader sees EOF.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        // Drop the master to close the fd, breaking reader_loop.
        // (master is already in self; explicit drop happens in field
        // tear-down. We just ensure reader_handle is joined after.)
        if let Some(h) = self.reader_handle.take() {
            // Cap join time so a stuck reader can't hang dux shutdown.
            let _ = h.join();  // OR: thread-with-timeout pattern
        }
    }
}
```

For a stuck-reader cap, use the `crossbeam-channel` recv-timeout pattern
or wrap in `thread::spawn(|| h.join())` and `recv_timeout(2s)`. Keep
simple unless integration tests show actual hangs.

### 5.3 — Document and assert the unsafe (P1-H)
At `src/pty.rs:496` (currently `unsafe { BorrowedFd::borrow_raw(raw_fd) }`),
add a doc comment + debug_assert:
```rust
// SAFETY: `raw_fd` was obtained from `self.master.as_raw_fd()`.
// The `master: Box<dyn MasterPty + Send>` field at line 74 keeps the
// underlying file descriptor alive for the entire lifetime of `self`.
// **DO NOT remove the `master` field** — it is currently marked
// `#[allow(dead_code)]` because no other code touches it, but its
// presence is load-bearing for this unsafe block.
debug_assert!(self.child.as_ref().is_some(),
    "PtyClient::child dropped before borrow_raw — fd may be invalid");
let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
```

Better: replace `#[allow(dead_code)]` on the `master` field with a
`#[doc = "kept alive to back the BorrowedFd at line 496 — do not remove"]`
comment + `let _ = &self.master;` somewhere it'll show up in builds.
Less tribal-knowledge fragile.

### 5.4 — Optional poison-recovery integration test
`tests/pty_integration.rs`:
```rust
#[test]
fn render_after_terminal_mutex_poison_does_not_panic() {
    let client = PtyClient::spawn(/* fake child */).unwrap();
    // Force a poison: lock mutex in a thread that panics.
    let term = client.terminal_for_test();
    std::thread::spawn(move || {
        let _g = term.lock().unwrap();
        panic!("intentional");
    }).join().unwrap_err();
    // Now snapshot should not panic.
    let snap = client.snapshot();
    assert!(snap.is_empty() || snap.is_default());
}
```
`terminal_for_test` is a `#[cfg(test)]` accessor.

## Validation
- `cargo test --test pty_integration` green.
- `cargo clippy --all-targets -- -D warnings` green.
- Manual: trigger a panic in the reader thread (e.g., a fault-injection
  cfg) and verify dux logs an error and continues with an empty pane,
  rather than tearing down.

## Acceptance criteria
- [x] Three `.expect("terminal mutex poisoned")` replaced with
  log-and-return sentinel.
- [x] `PtyClient::reader_handle: Option<JoinHandle<()>>` field present.
- [x] `Drop for PtyClient` joins the handle (with optional timeout).
- [x] Unsafe block at `pty.rs:~496` (now `:558`) has SAFETY comment naming
  the `master` field as the keep-alive.
- [x] `master` field uses `#[doc]` + `let _ = &self.master;` instead of
  `#[allow(dead_code)]`.
- [x] Optional: poison-recovery integration test added (`tests/pty_integration.rs`).
- [x] PR: `fix(pty): poison-tolerance + reader join + unsafe SAFETY doc` — landed via PR #2.

## Known pitfalls
- A 2 s join timeout on the reader is generous on healthy paths but
  bounds shutdown time on stuck PTYs. Don't use a 30 s timeout — that
  blocks Ctrl-C dux exit.
- Some `terminal.lock()` sites are inside hot render paths; the
  `Ok(...)` else-branch must stay branch-predictor-friendly. Don't
  build a `String` for every error log on the slow path.
- Tests that intentionally panic threads need `cargo test`'s default
  abort-on-panic disabled per-test; use `panic = "unwind"` (default
  test profile already does this).

## References
- audit02 P0-E, P1-G, P1-H.
- portable-pty README on reader thread expectations.
- Rust 2024 edition: `BorrowedFd::borrow_raw` SAFETY contract.
