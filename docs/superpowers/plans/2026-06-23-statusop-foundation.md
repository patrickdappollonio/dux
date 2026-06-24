# StatusOp Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a tri-state `StatusOp` object (a pending status that cannot be constructed without its success and failure outcomes) and prove it end-to-end by migrating the `push` operation, so later plans can migrate every remaining busy and finally seal the raw busy emitters.

**Architecture:** A `StatusOp<T, E>` bundles a correlation key, a pending message, and two closures (`&T -> Final`, `&E -> Final`) built where the operation is dispatched. A typestate builder forces both closures to be supplied before the op exists. A new `Engine::spawn_status_op` emits the keyed busy, runs the work on a worker thread, resolves the `Final` there (where the typed result is in scope), and ships only the resolved data back via a new `WorkerEvent::StatusOpCompleted`; the engine turns that into the keyed final (or a clear). This sidesteps any heterogeneous registry: closures run where `T`/`E` are concrete.

**Tech Stack:** Rust, the existing `dux_core` engine/worker/statusline modules, `std::thread`, `std::panic::catch_unwind`.

## Global Constraints

- Target platforms macOS + Linux only; no `#[cfg(windows)]`.
- All blocking work stays on worker threads, never the UI thread.
- Every `Busy` must be followed by a final (success/error/clear) on the same key.
- Status messages stay verbose and actionable (no terse "Pushed.").
- Verify with `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- Commit messages: plain sentences, no conventional-commit prefixes, end with the `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` trailer.

---

### Task 1: `Final` outcome type

**Files:**
- Create: `crates/dux-core/src/engine/status_op.rs`
- Modify: `crates/dux-core/src/engine/mod.rs` (add `pub mod status_op;` and re-export `Final`, `StatusOp`, `ResolvedFinal`, `status_op`)

**Interfaces:**
- Produces: `enum Final { Message { tone: StatusTone, text: String }, Clear }` with constructors `Final::info(text)`, `Final::warning(text)`, `Final::error(text)`, `Final::clear()`.

- [ ] **Step 1: Write the failing test** (in `status_op.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::statusline::StatusTone;

    #[test]
    fn final_constructors_carry_tone_and_text() {
        assert_eq!(
            Final::info("ok"),
            Final::Message { tone: StatusTone::Info, text: "ok".into() }
        );
        assert_eq!(
            Final::error("bad"),
            Final::Message { tone: StatusTone::Error, text: "bad".into() }
        );
        assert_eq!(
            Final::warning("hmm"),
            Final::Message { tone: StatusTone::Warning, text: "hmm".into() }
        );
        assert_eq!(Final::clear(), Final::Clear);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dux-core --lib status_op::tests::final_constructors_carry_tone_and_text`
Expected: FAIL to compile (`Final` not defined).

- [ ] **Step 3: Write minimal implementation** (top of `status_op.rs`)

```rust
//! The tri-state status object. A pending status cannot be constructed without
//! its success and failure outcomes (enforced by the typestate builder), so a
//! "loading" status that never resolves is inexpressible.

use crate::statusline::StatusTone;

/// What replaces a pending status when its operation finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Final {
    /// Replace the spinner with a transient success / persistent failure line.
    Message { tone: StatusTone, text: String },
    /// Deliberately dismiss the pending status with NO replacement message.
    /// Reads in review as "no final message needed here, empty is fine".
    Clear,
}

impl Final {
    pub fn info(text: impl Into<String>) -> Self {
        Final::Message { tone: StatusTone::Info, text: text.into() }
    }
    pub fn warning(text: impl Into<String>) -> Self {
        Final::Message { tone: StatusTone::Warning, text: text.into() }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Final::Message { tone: StatusTone::Error, text: text.into() }
    }
    pub fn clear() -> Self {
        Final::Clear
    }
}
```

Add to `crates/dux-core/src/engine/mod.rs` near the other `mod` lines:

```rust
pub mod status_op;
pub use status_op::{status_op, Final, ResolvedFinal, StatusOp};
```

(The `ResolvedFinal`/`StatusOp`/`status_op` names land in Tasks 2–3; add the full `pub use` now and let it fail to compile until then, OR add names incrementally — implementer's choice. If compiling incrementally, re-export only `Final` here and extend in later tasks.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dux-core --lib status_op::tests::final_constructors_carry_tone_and_text`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dux-core/src/engine/status_op.rs crates/dux-core/src/engine/mod.rs
git commit -m "Add the Final outcome type for the tri-state status object

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `ResolvedFinal` carry-back data

**Files:**
- Modify: `crates/dux-core/src/engine/status_op.rs`

**Interfaces:**
- Consumes: `Final` (Task 1).
- Produces: `struct ResolvedFinal { key: String, outcome: Final }` with `ResolvedFinal::new(key, outcome)` and `ResolvedFinal::error(key, text)` (the panic fallback). It is `Clone + Debug + Send`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn resolved_final_error_builds_a_keyed_error_message() {
    let r = ResolvedFinal::error("push:/a", "boom");
    assert_eq!(r.key, "push:/a");
    assert_eq!(r.outcome, Final::error("boom"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dux-core --lib status_op::tests::resolved_final_error_builds_a_keyed_error_message`
Expected: FAIL to compile (`ResolvedFinal` not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
/// The resolved outcome shipped back from a worker thread: the operation's key
/// plus the `Final` produced by running the matching success/failure closure
/// where the typed result was in scope. Plain data so it crosses the worker
/// channel (and serialization, for any future wire use) without closures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFinal {
    pub key: String,
    pub outcome: Final,
}

impl ResolvedFinal {
    pub fn new(key: impl Into<String>, outcome: Final) -> Self {
        Self { key: key.into(), outcome }
    }
    /// Panic fallback used by `spawn_status_op` when the work closure unwinds:
    /// the success/failure closures never ran, so synthesise a keyed error.
    pub fn error(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self { key: key.into(), outcome: Final::error(text) }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dux-core --lib status_op::tests::resolved_final_error_builds_a_keyed_error_message`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dux-core/src/engine/status_op.rs
git commit -m "Add ResolvedFinal, the worker-to-engine status carry-back

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Typestate `StatusOp` builder + `resolve`

**Files:**
- Modify: `crates/dux-core/src/engine/status_op.rs`

**Interfaces:**
- Consumes: `Final`, `ResolvedFinal`.
- Produces:
  - `fn status_op(key: impl Into<String>, pending: impl Into<String>) -> NeedsSuccess`
  - `NeedsSuccess::on_success<T>(self, f: impl FnOnce(&T) -> Final + Send + 'static) -> NeedsFailure<T>`
  - `NeedsFailure<T>::on_failure<E>(self, f: impl FnOnce(&E) -> Final + Send + 'static) -> StatusOp<T, E>`
  - `StatusOp<T, E>::key(&self) -> &str`
  - `StatusOp<T, E>::pending_status(&self) -> crate::engine::StatusUpdate` (a keyed `Busy`)
  - `StatusOp<T, E>::resolve(self, result: &Result<T, E>) -> ResolvedFinal`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn status_op_resolves_success_and_failure_with_its_key() {
    // success
    let op = status_op("push:/a", "Pushing…")
        .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
        .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
    assert_eq!(op.key(), "push:/a");
    let pending = op.pending_status();
    assert_eq!(pending.tone, crate::statusline::StatusTone::Busy);
    assert_eq!(pending.key.as_deref(), Some("push:/a"));
    let resolved = op.resolve(&Ok::<u32, String>(3));
    assert_eq!(resolved, ResolvedFinal::new("push:/a", Final::info("Pushed 3 commits.")));

    // failure (fresh op; resolve consumes self)
    let op = status_op("push:/a", "Pushing…")
        .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
        .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
    let resolved = op.resolve(&Err::<u32, String>("nope".into()));
    assert_eq!(resolved, ResolvedFinal::new("push:/a", Final::error("Push failed: nope")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dux-core --lib status_op::tests::status_op_resolves_success_and_failure_with_its_key`
Expected: FAIL to compile (`status_op` not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
use crate::engine::StatusUpdate;
use std::marker::PhantomData;

/// Entry point: a key + pending message, awaiting its success closure. You
/// cannot obtain a `StatusOp` without passing through `on_success` then
/// `on_failure`, so both outcomes are always declared.
pub fn status_op(key: impl Into<String>, pending: impl Into<String>) -> NeedsSuccess {
    NeedsSuccess { key: key.into(), pending: pending.into() }
}

pub struct NeedsSuccess {
    key: String,
    pending: String,
}

impl NeedsSuccess {
    pub fn on_success<T, F>(self, f: F) -> NeedsFailure<T>
    where
        F: FnOnce(&T) -> Final + Send + 'static,
    {
        NeedsFailure {
            key: self.key,
            pending: self.pending,
            on_success: Box::new(f),
            _t: PhantomData,
        }
    }
}

pub struct NeedsFailure<T> {
    key: String,
    pending: String,
    on_success: Box<dyn FnOnce(&T) -> Final + Send>,
    _t: PhantomData<fn(&T)>,
}

impl<T> NeedsFailure<T> {
    pub fn on_failure<E, F>(self, f: F) -> StatusOp<T, E>
    where
        F: FnOnce(&E) -> Final + Send + 'static,
    {
        StatusOp {
            key: self.key,
            pending: self.pending,
            on_success: self.on_success,
            on_failure: Box::new(f),
        }
    }
}

/// A fully-specified tri-state status. Carries its key, pending text, and the
/// two outcome closures. Resolve it where the typed `Result` is in scope.
pub struct StatusOp<T, E> {
    key: String,
    pending: String,
    on_success: Box<dyn FnOnce(&T) -> Final + Send>,
    on_failure: Box<dyn FnOnce(&E) -> Final + Send>,
}

impl<T, E> StatusOp<T, E> {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The keyed `Busy` to show while the operation runs.
    pub fn pending_status(&self) -> StatusUpdate {
        StatusUpdate::busy(self.pending.clone()).with_key(self.key.clone())
    }

    /// Run the matching closure for the outcome and return the keyed result.
    pub fn resolve(self, result: &Result<T, E>) -> ResolvedFinal {
        let outcome = match result {
            Ok(t) => (self.on_success)(t),
            Err(e) => (self.on_failure)(e),
        };
        ResolvedFinal::new(self.key, outcome)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dux-core --lib status_op::tests::status_op_resolves_success_and_failure_with_its_key`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dux-core/src/engine/status_op.rs crates/dux-core/src/engine/mod.rs
git commit -m "Add the typestate StatusOp builder and its resolve step

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `EventReaction::ClearStatus` on both surfaces

**Files:**
- Modify: `crates/dux-core/src/engine/events.rs` (add `ClearStatus(String)` variant + its `reaction_name` arm)
- Modify: `crates/dux-tui/src/app/workers.rs` (handle it in `apply_reaction`)
- Modify: `crates/dux-web/src/engine_actor.rs` (handle it in the worker-event drain)

**Interfaces:**
- Consumes: nothing new.
- Produces: `EventReaction::ClearStatus(String)` — the engine's way to say "dismiss the keyed status with no replacement". The TUI clears the keyed entry; the web emits a `StatusCleared` via the emitter.

- [ ] **Step 1: Write the failing test** (TUI, in `crates/dux-tui/src/app/workers.rs` tests)

```rust
#[test]
fn clear_status_reaction_dismisses_the_keyed_entry() {
    use crate::statusline::StatusTone;
    let mut app = crate::app::test_support::test_app(crate::app::test_support::default_bindings());
    app.status.set(
        std::time::Instant::now(),
        Some("push:/a".to_string()),
        StatusTone::Busy,
        "Pushing…",
    );
    app.apply_reaction(dux_core::engine::EventReaction::ClearStatus("push:/a".into()));
    assert!(
        app.status.snapshot().iter().all(|s| s.key.as_deref() != Some("push:/a")),
        "ClearStatus must remove the keyed entry"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dux-tui --lib clear_status_reaction_dismisses_the_keyed_entry`
Expected: FAIL to compile (`ClearStatus` not a variant).

- [ ] **Step 3: Write minimal implementation**

In `events.rs`, add to the `EventReaction` enum (near `Status`):

```rust
    /// Dismiss a keyed status with no replacement message (the `Final::Clear`
    /// outcome of a `StatusOp`). The TUI removes the keyed entry; the web emits
    /// a `StatusCleared` frame for the key.
    ClearStatus(String),
```

In `events.rs` `reaction_name` (the debug-name match), add:

```rust
            EventReaction::ClearStatus(_) => "ClearStatus",
```

In `crates/dux-tui/src/app/workers.rs` `apply_reaction`, add an arm next to `EventReaction::Status`:

```rust
            EventReaction::ClearStatus(key) => {
                self.status.clear(&key, None);
            }
```

In `crates/dux-web/src/engine_actor.rs`, in the worker-event drain (alongside the existing `if let Some(key) = busy_key_to_clear` block), add:

```rust
            if let dux_core::engine::EventReaction::ClearStatus(key) = &reaction {
                thread_status_tx.clear(key.clone());
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dux-tui --lib clear_status_reaction_dismisses_the_keyed_entry`
Expected: PASS.

- [ ] **Step 5: Run the web + core suites to confirm no match-exhaustiveness breaks**

Run: `cargo test -p dux-core -p dux-web --lib`
Expected: PASS (fix any non-exhaustive `match reaction` the new variant surfaces by adding a `ClearStatus` arm that returns `vec![]`/`EventReaction::Nothing` as appropriate).

- [ ] **Step 6: Commit**

```bash
git add crates/dux-core/src/engine/events.rs crates/dux-tui/src/app/workers.rs crates/dux-web/src/engine_actor.rs
git commit -m "Add an EventReaction::ClearStatus for the Final::Clear outcome

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: `spawn_status_op` primitive + `WorkerEvent::StatusOpCompleted`

**Files:**
- Modify: `crates/dux-core/src/worker.rs` (add the `StatusOpCompleted` variant)
- Modify: `crates/dux-core/src/engine/status_op.rs` (add `ResolvedFinal::into_reaction`)
- Modify: `crates/dux-core/src/engine/spawn_worker.rs` (add `Engine::spawn_status_op`)
- Modify: `crates/dux-core/src/engine/events.rs` (handle `StatusOpCompleted`)

**Interfaces:**
- Consumes: `StatusOp`, `ResolvedFinal`, `Final`, `EventReaction::ClearStatus`/`Status`.
- Produces:
  - `WorkerEvent::StatusOpCompleted { resolved: ResolvedFinal }`
  - `ResolvedFinal::into_reaction(self) -> EventReaction`
  - `Engine::spawn_status_op<T, E>(&mut self, op: StatusOp<T, E>, work: F) where F: FnOnce() -> Result<T, E> + Send + 'static, T: Send + 'static, E: Send + 'static` returning `EventReaction` (the keyed pending `Status`).

- [ ] **Step 1: Write the failing test** (in `status_op.rs` tests)

```rust
#[test]
fn resolved_final_into_reaction_maps_message_and_clear() {
    use crate::engine::EventReaction;
    match ResolvedFinal::new("k", Final::info("done")).into_reaction() {
        EventReaction::Status(s) => {
            assert_eq!(s.key.as_deref(), Some("k"));
            assert_eq!(s.tone, crate::statusline::StatusTone::Info);
            assert_eq!(s.message, "done");
        }
        other => panic!("expected Status, got {other:?}"),
    }
    match ResolvedFinal::new("k", Final::clear()).into_reaction() {
        EventReaction::ClearStatus(k) => assert_eq!(k, "k"),
        other => panic!("expected ClearStatus, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dux-core --lib status_op::tests::resolved_final_into_reaction_maps_message_and_clear`
Expected: FAIL to compile (`into_reaction` not defined).

- [ ] **Step 3: Write minimal implementation**

In `status_op.rs`:

```rust
impl ResolvedFinal {
    /// Translate the carried outcome into the engine reaction that applies it.
    pub fn into_reaction(self) -> crate::engine::EventReaction {
        use crate::engine::{EventReaction, StatusUpdate};
        match self.outcome {
            Final::Message { tone, text } => EventReaction::Status(
                StatusUpdate { tone, message: text, key: Some(self.key) },
            ),
            Final::Clear => EventReaction::ClearStatus(self.key),
        }
    }
}
```

In `worker.rs`, add to `WorkerEvent`:

```rust
    /// A `spawn_status_op` worker finished and carried back its resolved final
    /// (the success/failure message or a clear, already keyed).
    StatusOpCompleted {
        resolved: crate::engine::ResolvedFinal,
    },
```

In `events.rs` `process_worker_event`, add:

```rust
            WorkerEvent::StatusOpCompleted { resolved } => resolved.into_reaction(),
```

In `spawn_worker.rs`, add to `impl Engine` (mirroring the panic-safety of `spawn_command_worker`):

```rust
    /// Dispatch a keyed tri-state operation: emit its pending Busy, run `work`
    /// off-thread, resolve the success/failure closure where the typed result
    /// is in scope, and ship the keyed final back via `StatusOpCompleted`. The
    /// returned reaction is the pending Busy to apply now.
    pub fn spawn_status_op<T, E, F>(
        &mut self,
        op: crate::engine::StatusOp<T, E>,
        work: F,
    ) -> EventReaction
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        use std::panic::AssertUnwindSafe;
        let pending = op.pending_status();
        let key = op.key().to_string();
        let tx = self.worker_tx.clone();
        thread::Builder::new()
            .name("dux-status-op".into())
            .spawn(move || {
                let resolved = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let result = work();
                    op.resolve(&result)
                })) {
                    Ok(r) => r,
                    Err(payload) => {
                        let reason = format_panic_payload(payload);
                        crate::logger::error(&format!("status-op worker panicked: {reason}"));
                        crate::engine::ResolvedFinal::error(key, format!("Worker panicked: {reason}"))
                    }
                };
                let _ = tx.send(crate::worker::WorkerEvent::StatusOpCompleted { resolved });
            })
            .map(|_| EventReaction::Status(pending))
            .unwrap_or_else(|e| {
                // Spawn itself failed: never emitted the busy, so report the
                // failure as a one-shot keyed error instead.
                EventReaction::Status(
                    crate::engine::StatusUpdate::error(format!(
                        "Could not start background worker: {e}"
                    ))
                    .with_key(op_key_fallback()),
                )
            });
    }
```

NOTE for the implementer: the `unwrap_or_else` closure cannot reuse `op`/`key` (both moved into the thread closure on the `Ok` path, and `key` is moved even on the type level). Capture a second clone before the spawn:

```rust
        let pending = op.pending_status();
        let key = op.key().to_string();
        let key_for_spawn_fail = key.clone();
        let tx = self.worker_tx.clone();
        // … in the unwrap_or_else, build the error with `key_for_spawn_fail`
        // and drop the `op_key_fallback()` placeholder above.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dux-core --lib status_op::tests::resolved_final_into_reaction_maps_message_and_clear`
Expected: PASS.

- [ ] **Step 5: Add an integration test for the round-trip** (in `spawn_worker.rs` or `status_op.rs` tests, using `test_engine()`)

```rust
#[test]
fn spawn_status_op_emits_pending_then_resolves_via_worker() {
    use crate::engine::{status_op, Final};
    use crate::statusline::StatusTone;
    let (mut engine, _tmp) = crate::engine::test_support::test_engine();
    let op = status_op("op:1", "Working…")
        .on_success(|n: &u32| Final::info(format!("Did {n}.")))
        .on_failure(|e: &String| Final::error(e.clone()));
    let pending = engine.spawn_status_op(op, || Ok::<u32, String>(2));
    match pending {
        EventReaction::Status(s) => {
            assert_eq!(s.tone, StatusTone::Busy);
            assert_eq!(s.key.as_deref(), Some("op:1"));
        }
        other => panic!("expected pending Status, got {other:?}"),
    }
    // Drain the worker's completion (it runs on a thread; block briefly).
    let ev = engine.worker_rx.recv().expect("completion event");
    let reaction = engine.process_worker_event(ev);
    match reaction {
        EventReaction::Status(s) => {
            assert_eq!(s.key.as_deref(), Some("op:1"));
            assert_eq!(s.message, "Did 2.");
        }
        other => panic!("expected resolved Status, got {other:?}"),
    }
}
```

- [ ] **Step 6: Run it**

Run: `cargo test -p dux-core --lib spawn_status_op_emits_pending_then_resolves_via_worker`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/dux-core/src/engine/status_op.rs crates/dux-core/src/engine/spawn_worker.rs crates/dux-core/src/engine/events.rs crates/dux-core/src/worker.rs
git commit -m "Add spawn_status_op and the StatusOpCompleted round-trip

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Migrate `push` to `StatusOp`

**Files:**
- Modify: `crates/dux-core/src/engine/command.rs` (the `Command::Push` arm)
- Modify: `crates/dux-core/src/engine/events.rs` (remove the `WorkerEvent::PushCompleted` handler arm)
- Modify: `crates/dux-core/src/worker.rs` (remove the `PushCompleted` variant)
- Modify: any test referencing `PushCompleted` (search first)

**Interfaces:**
- Consumes: `spawn_status_op`, `status_op`, `Final`, `status_keys::push`.
- Produces: no new public interface; `Command::Push` now routes through `spawn_status_op`.

- [ ] **Step 1: Add `status_keys::push` constructor** (in `crates/dux-core/src/wire.rs`, mirroring the existing typed constructors)

```rust
    /// Push operation key, parameterised by worktree path.
    pub fn push(worktree_path: &str) -> String {
        format!("{PUSH_PREFIX}:{worktree_path}")
    }
```

- [ ] **Step 2: Find every `PushCompleted` reference**

Run: `rg -n "PushCompleted" crates/`
Expected: the `command.rs` panic_event, the `events.rs` handler, the `worker.rs` variant, and any tests. Note them; all must go or move to the new path.

- [ ] **Step 3: Write/adjust the failing test**

Update the existing push test (found at `crates/dux-core/src/wire.rs` push test, ~`wire.rs:5439`, named around `push_completed_*`) to instead drive `Command::Push` and assert the pending is keyed and the resolved final is keyed. Concretely, add this test in `command.rs` tests (or adapt the existing one):

```rust
#[test]
fn push_routes_through_status_op_with_matching_key() {
    use crate::statusline::StatusTone;
    let (mut engine, _tmp) = crate::engine::test_support::test_engine();
    let wt = std::path::PathBuf::from("/tmp/does-not-exist-wt");
    let key = format!("push:{}", wt.to_string_lossy());
    let pending = engine
        .apply(crate::engine::Command::Push { worktree_path: wt })
        .expect("push dispatch");
    match pending {
        crate::engine::EventReaction::Status(s) => {
            assert_eq!(s.tone, StatusTone::Busy);
            assert_eq!(s.key.as_deref(), Some(key.as_str()));
        }
        other => panic!("expected pending Busy, got {other:?}"),
    }
    // The worker will fail (bogus path); its resolved final must carry the key.
    let ev = engine.worker_rx.recv().expect("completion");
    match engine.process_worker_event(ev) {
        crate::engine::EventReaction::Status(s) => {
            assert_eq!(s.key.as_deref(), Some(key.as_str()));
            assert_eq!(s.tone, StatusTone::Error);
        }
        other => panic!("expected keyed error final, got {other:?}"),
    }
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p dux-core --lib push_routes_through_status_op_with_matching_key`
Expected: FAIL (still old path / key assertions differ), or compile error once `PushCompleted` is removed in Step 5.

- [ ] **Step 5: Rewrite the `Command::Push` arm** in `command.rs`:

```rust
            Command::Push { worktree_path } => {
                let key = crate::wire::status_keys::push(&worktree_path.to_string_lossy());
                let op = crate::engine::status_op(key, "Pushing to remote…")
                    .on_success(|_: &()| {
                        Final::info(
                            "Pushed to remote successfully. Your changes are now available to collaborators.",
                        )
                    })
                    .on_failure(|e: &String| Final::error(format!("Push to remote failed: {e}")));
                let wt = worktree_path.clone();
                Ok(self.spawn_status_op(op, move || {
                    crate::git::push(&wt).map(|_| ()).map_err(|e| format!("{e:#}"))
                }))
            }
```

Ensure `Final` and `status_op` are imported in `command.rs` (e.g. `use crate::engine::{status_op, Final};` or fully-qualify).

- [ ] **Step 6: Remove the dead `PushCompleted`**

Delete the `WorkerEvent::PushCompleted { key, result } => match result { … }` arm in `events.rs` and the `PushCompleted { … }` variant in `worker.rs`. Re-run `rg -n "PushCompleted" crates/` and remove any remaining references (old tests).

- [ ] **Step 7: Run the push test + full core suite**

Run: `cargo test -p dux-core --lib push_routes_through_status_op_with_matching_key`
Expected: PASS.
Run: `cargo test -p dux-core --lib`
Expected: PASS (update/remove any other `PushCompleted` test that no longer compiles).

- [ ] **Step 8: Verify the web push path still resolves**

The web push goes through `wire_statuses_from_reaction` for the pending `Status` and the resolved `Status`/`ClearStatus` through the drain. Run: `cargo test -p dux-web --lib` and a manual check that `Command::Push` over the wire still surfaces a keyed busy then a keyed final. Expected: PASS.

- [ ] **Step 9: Full verification + commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p dux-core -p dux-web -p dux-tui --lib
git add -A
git commit -m "Route push through the StatusOp object and drop PushCompleted

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Status (2026-06-24)

**Foundation complete and proven; opaque ids landed; 3 ops migrated.**

- `Final`/`ResolvedFinal`/`StatusOp` + `spawn_status_op` round-trip + `EventReaction::ClearStatus` (`3962efe`, `f8cd44e`).
- **Opaque ids:** `status_op(pending)` mints its own monotonic id; consumers never author or see a key. Killed the `status_keys` discipline at the source (a developer cannot mismatch a key they never touch).
- Migrated and committed: **push** (pure-status), **pull** (domain-ful carry-`ResolvedFinal`), **open-path** (pure-status). Both patterns have a working reference.
- Whole workspace clippy-clean; 705 core + 780 tui + 173 web + 289 client tests pass.

**Remaining busy emitters (all the hard/bulk ones now):**
- Engine rich-view ops: `create` (command.rs:459, + progress re-emit events.rs:1231), `commit-message` (command.rs:942) — completion is surface-specific view logic, use carry-`ResolvedFinal`.
- Web sync ops (wire.rs): launch (1084), checkout-default (1143), add-project-checkout (1192), pr-lookup (1277), delete (1473) — emitted synchronously through `apply_wire`; the op's pending rides the command-result, the final the status stream.
- TUI `set_busy` (~25 sites in sessions.rs/mod.rs/input.rs/auth_users.rs): each is dispatched TUI-side and resolved in a worker-completion handler; migrate to a `StatusOp` whose resolution runs in that handler (clipboard, rename-branch, reconnect, server-flip, project-persistence, auth-users, load-worktrees, etc.).

**Final step (only after every site above is migrated):** delete `App::set_busy`,
`StatusUpdate::busy`, and the free busy `WireStatus` constructors; make
`KeyedStatusController::set(.., Busy, ..)` crate-private; add the §3.6.2 pairing
harness. This is the step that makes a dangling busy *inexpressible*.

Note: the universal busy-timeout guardrail (`d4c40aa` + the anonymous-slot
coverage) already makes a dangling busy *impossible at runtime* — any unpaired
busy self-heals to a logged warning in 20s — so the remaining migration is about
the stronger compile-time guarantee, not about live leaks.

**Remaining for full sealing (each a mechanical application of the patterns
above):** migrate commit-message (engine/web), create, the launch/reconnect
family, delete/worktree-removal, the web checkout/add-project/pr-lookup ops,
open-path, clipboard-copy, branch-rename, auth-users, project-persistence, and
the ~15 TUI `set_busy` call sites; then **seal** by making
`KeyedStatusController::set(.., Busy, ..)` crate-private and deleting
`App::set_busy`, `StatusUpdate::busy`, and the free busy `WireStatus`
constructors; finally add the §3.6.2 pairing test harness.

## Architectural finding (from the push migration)

`push` is a *pure-status* op: its completion emits only a status, so it fits
`spawn_status_op` directly. **Most other ops are domain-ful** — their completion
also mutates engine state and/or fans out follow-up reactions (e.g. `pull` clears
the in-flight guard, updates project branch state, and emits `ReloadChangedFiles`;
`create`/`delete`/`launch` rebuild views and persist records). For those, the
simple `spawn_status_op` (which emits *only* a status) is the wrong shape.

The general pattern for a domain-ful op is **carry the `ResolvedFinal` alongside
the existing domain event**:

1. At dispatch, build the `StatusOp` (declaring both outcomes) and emit
   `op.pending_status()` as the busy. Move `op` into the worker.
2. In the worker, after producing the typed result, call
   `op.resolve(&result)` and put the resulting `ResolvedFinal` into a new field
   on the op's existing `WorkerEvent` (e.g. `PullCompleted { …, status: ResolvedFinal }`).
3. In the completion handler, do the domain work as today, then emit the carried
   final: `EventReaction::Multi(vec![ resolved.into_reaction(), <domain follow-ups> ])`
   (or just `resolved.into_reaction()` when there are no follow-ups).

This keeps the three states declared together at the dispatch site (the sealing
guarantee) while leaving domain logic in the engine handler. Plans 2–5 below use
this variant for every op except trivially pure-status ones.

## Out of scope (subsequent plans)

Each is a self-contained follow-on plan that reuses Tasks 1–5 and follows the Task 6
pattern (pure-status) or the carry-`ResolvedFinal` variant above (domain-ful):

- **Plan 2 — pull + commit-message** (engine `spawn_command_worker` keyed ops).
- **Plan 3 — create/launch family** (`DispatchCreateAgentRequest`, reconnect) — larger because the completion does domain work; the worker carries `StatusOpCompleted` *in addition to* the existing `AgentLaunchReady*` events, or the `AgentLaunchReady*` events gain a `ResolvedFinal` field.
- **Plan 4 — delete/worktree-removal + the web-only checkout/add-project/pr-lookup ops.**
- **Plan 5 — TUI anonymous busies** (the ~15 `set_busy` call sites): each becomes a `StatusOp` whose resolution runs in its existing worker-completion handler.
- **Plan 6 — Seal the raw emitters.** Only after every site above is migrated: make `KeyedStatusController::set(.., Busy, ..)` crate-private, delete `App::set_busy`, delete `StatusUpdate::busy`, and remove/privatise the free busy `WireStatus` constructors. Add the §3.6.2 pairing test harness that drives every keyed op through ok/err/edge and fails CI on any residual open busy. This is the task that makes a dangling busy inexpressible.

## Self-Review

- **Spec coverage:** Tasks 1–5 build the `StatusOp`/`Final`/`ResolvedFinal` object and round-trip (spec §3.1, §3.2); Task 6 proves it on `push`. The sealing (§3.3) and pairing harness (§3.6.2) are explicitly deferred to Plan 6 once all sites migrate — sealing before migration would not compile.
- **Placeholder scan:** the only forward reference is the `pub use` in Task 1 naming types from Tasks 2–3; Task 1 Step 3 calls this out and offers the incremental-export alternative. The `op_key_fallback()` placeholder in Task 5 Step 3 is explicitly corrected in the following NOTE (use `key_for_spawn_fail`).
- **Type consistency:** `Final`, `ResolvedFinal`, `StatusOp<T,E>`, `status_op`, `NeedsSuccess`, `NeedsFailure<T>`, `spawn_status_op`, `WorkerEvent::StatusOpCompleted`, `EventReaction::ClearStatus`, `ResolvedFinal::into_reaction`, `status_keys::push` are used consistently across tasks.
