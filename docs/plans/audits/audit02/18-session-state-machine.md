# Phase 18: Session state machine (typestate)

> Maps to: **P1-Z**.

## Goal
Today there are three sources of truth for session liveness:
1. `SessionStatus = Active | Detached | Exited` enum (`model.rs:62`)
2. `providers: HashMap<String, PtyClient>` membership
3. `child.try_wait()` exit signal

They drift. Today's bugs: an `Exited` session can have a running PTY
in the providers map; an `Active` session can have a child that
already returned `Some(_)` from `try_wait()`. Force all transitions
through one function.

## Pre-conditions
- Phase 00 baseline green.
- Phase 17 (App decomposition) merged — state machine lives in
  `RuntimeState`.

## Files to touch
- `src/model.rs` — replace `SessionStatus` with rich enum.
- `src/app/state/runtime.rs` (post-Phase-17) — owner of state transitions.
- `src/app/sessions.rs` — call sites switch to transition fns.
- `src/storage.rs` — serde for new enum, with migration from old.
- `tests/session_state.rs` — NEW.

## Steps

### 18.1 — New enum
```rust
// src/model.rs
pub enum SessionState {
    /// Created but never spawned.
    Created { created_at: DateTime<Utc> },
    /// Spawn in flight.
    Spawning { since: DateTime<Utc> },
    /// PTY live and accepting input.
    Live {
        pty_handle: PtyHandle,           // strong-typed wrapper, not PtyClient directly
        spawned_at: DateTime<Utc>,
        last_active_at: DateTime<Utc>,
    },
    /// Pane detached but pty still alive (child running).
    Detached {
        pty_handle: PtyHandle,
        detached_at: DateTime<Utc>,
    },
    /// Child exited; no pty.
    Exited {
        exit_code: Option<i32>,
        exited_at: DateTime<Utc>,
    },
}
```
Note: `PtyHandle` is a thin wrapper that owns the `PtyClient` AND
encodes "alive" at the type level. Construction is private to
`runtime.rs` so external code can't bypass.

### 18.2 — Transition functions
```rust
impl RuntimeState {
    pub fn spawn(&mut self, id: SessionId, ...) -> Result<()> {
        // current must be Created or Exited
        // sets state to Spawning; emits worker job
    }
    pub fn on_spawn_succeeded(&mut self, id: &SessionId, pty: PtyClient) -> Result<()> {
        // current must be Spawning
        // sets state to Live, builds PtyHandle
    }
    pub fn on_spawn_failed(&mut self, id: &SessionId, err: anyhow::Error) -> Result<()> { ... }
    pub fn detach(&mut self, id: &SessionId) -> Result<()> {
        // Live -> Detached
    }
    pub fn reattach(&mut self, id: &SessionId) -> Result<()> {
        // Detached -> Live
    }
    pub fn on_child_exit(&mut self, id: &SessionId, code: Option<i32>) -> Result<()> {
        // Live | Detached -> Exited (drops PtyHandle)
    }
    pub fn delete(&mut self, id: &SessionId) -> Result<()> {
        // any -> removed from map
    }
}
```
Each fn enforces "current state in {…}" and returns `Err(anyhow!("illegal transition: {} -> spawn", current.name()))` otherwise.

### 18.3 — Eliminate the `providers` HashMap
Replace `providers: HashMap<SessionId, PtyClient>` with iteration over
sessions whose state is `Live | Detached`. The `PtyHandle` is inside
the state variant; access only via the transition functions.

### 18.4 — Migrate storage
`Storage::upsert_session` previously serialized `SessionStatus` as a
short tag string. Now serialize `SessionState` keeping only the
*persistable* fields (timestamps, exit_code, NOT pty_handle):
```rust
#[derive(Serialize, Deserialize)]
enum PersistedSessionState {
    Created { created_at: DateTime<Utc> },
    Spawning { since: DateTime<Utc> },
    Detached { detached_at: DateTime<Utc> },     // restored as Detached on reload
    Exited { exit_code: Option<i32>, exited_at: DateTime<Utc> },
}
```
Notice `Live` is NOT persistable — it implies a running PtyClient.
On reload, `Live` becomes `Detached` (auto-resume Phase 15 then
optionally re-spawns).

`From<SessionState>` and `TryFrom<PersistedSessionState>` handle the
conversion. **Migration**: read old `status: String`; map "Active" →
`Detached`, "Detached" → `Detached`, "Exited" → `Exited`. One-shot
SQL to populate the new column on first boot under new schema.

### 18.5 — Tests
```rust
#[test]
fn cannot_detach_a_created_session() {
    let mut rt = RuntimeState::new();
    rt.create_session(...);
    let r = rt.detach(&id);
    assert!(matches!(r, Err(_)));
}
#[test]
fn spawn_succeeded_only_from_spawning() { ... }
#[test]
fn child_exit_drops_pty_handle() { ... }
#[test]
fn live_session_not_persisted_as_live() {
    let st: PersistedSessionState = SessionState::Live{..}.try_into().unwrap();
    assert!(matches!(st, PersistedSessionState::Detached{..}));
}
```

## Validation
- `cargo test session_state` green.
- `cargo clippy -D warnings` green.
- Manual: spawn, detach, reattach, kill child, observe state in dux
  via diagnostics overlay (or doctor — Phase 20). Each transition is
  a discrete step.

## Acceptance criteria
- [ ] `SessionState` enum (5 variants) replaces `SessionStatus`.
- [ ] All transitions go through `RuntimeState::*` fns; no direct
      enum mutation outside that module.
- [ ] `providers: HashMap` removed (PtyHandle lives inside the state).
- [ ] Storage serializes via `PersistedSessionState`; migration from
      old text status validated by a fixture.
- [ ] 4 tests pass.
- [ ] PR: `refactor(model): explicit session state machine (P1-Z)`.

## Known pitfalls
- The `Live` → `Detached` on persist semantics may surprise users
  ("I left it running and it shows detached on restart"). Document.
- `PtyHandle` wrapper is the typestate gate; do NOT make it `Clone`
  or expose a `&mut PtyClient` getter — that defeats the purpose.
- Drop-order in the enum variant matters: when a `Live` state goes
  out of scope (delete), `PtyHandle::drop` must kill the child + join
  the reader thread (Phase 05).
- Migration step must be idempotent — second run sees the new
  serialization and is a no-op.

## References
- audit02 P1-Z.
- Typestate pattern (Cliff Biffle): https://cliffle.com/blog/rust-typestate/
