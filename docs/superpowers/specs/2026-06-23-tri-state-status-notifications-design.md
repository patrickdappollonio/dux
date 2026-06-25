# Tri-State Status Notifications — Design

Date: 2026-06-23
Branch: `server-mode`
Status: Proposed (awaiting user review)

## 1. Problem

dux shows indeterminate ("loading"/`Busy`) status while a background action runs,
and is supposed to replace it with a determinate final state (success or failure)
when the action finishes. Several operations leak: they show a pending status that
is never replaced, so the TUI status line eventually flips to a spurious
"timed out — check dux.log" warning (~20s) and the web shows a loading toast that
spins forever.

A three-surface audit (TUI status line, web server `WireStatus` stream, web React
client) found that **every leak has the same shape: a keyed `Busy` is emitted, but
the paired final lands on a different key (usually the anonymous slot) or no final
is emitted at all on some code path.** The full inventory is in Appendix A.

The reported symptom — "Removing worktree for agent …" never dismisses on the web —
is the most visible instance, and it leaks on the *normal* path (not just an edge),
because of a client-side key-routing bug.

### Root cause is structural, not a set of isolated bugs

The current contract is: "a `Busy` and its eventual final carry the same key, by
convention." Nothing enforces the convention. The busy is emitted at one site (when
a worker is spawned) and the final at a completely different site (a worker-completion
handler, on another thread, and on the web across a serialization boundary). Because
the two sites are independent, the key/slot can — and repeatedly does — drift. The
fix must make the busy and its outcomes a **single object** so they cannot drift, and
must **remove the ability to emit a bare busy** so the object is the only path.

## 2. Goal

1. Fix every confirmed leak in Appendix A so each pending status is replaced by a
   success/failure final (or an explicit dismissal) on every code path.
2. Restructure the notification API so that **code can no longer express a pending
   status without also declaring how it ends.** A developer adding a new operation
   must declare its success and failure (or an explicit "clear with no replacement")
   at the point the pending state is created; the key is shared by construction.
3. Make any residual leak loud: log it at runtime and fail it in CI.

### Non-goal (explicitly rejected)

A literal compile-time `#[must_use]` RAII token that is "redeemed" by the final is
**not** pursued. A pending status is created when a worker is spawned and resolved
much later in a different function, thread, and (web) process. No single-scope RAII
value spans that gap; a Drop-based check would fire at the wrong scope. We get the
same practical guarantee by (a) making the tri-state object the *only* constructor of
a pending status, and (b) enforcing resolution with a CI test harness plus runtime
logging.

## 3. Design

### 3.1 The `StatusOp` object (closures baked in at creation)

A single object bundles the pending state with both outcomes. It is generic over the
operation's success result `T` and error `E`, and the outcome messages are closures so
they can include runtime detail (commit counts, error text):

```rust
/// Tri-state status for one operation. Constructing one forces you to declare
/// the pending message AND both outcomes; you cannot show a pending status
/// without them. The key is owned here and reused for the pending and the
/// final, so the two can never drift onto different slots.
pub struct StatusOp<T, E> {
    key: StatusKey,
    pending: String,
    success: Box<dyn FnOnce(&T) -> Final + Send>,
    failure: Box<dyn FnOnce(&E) -> Final + Send>,
}

/// What replaces the pending status.
pub enum Final {
    /// Replace the spinner with a transient success / persistent failure line.
    Message { tone: StatusTone, text: String },
    /// Deliberately dismiss the pending status with NO replacement message.
    /// Reads in code review as "I do not care about a final message here, empty
    /// is fine" — the explicit, greppable escape hatch.
    Clear,
}
```

Builder (so all three are required before the op is usable):

```rust
StatusOp::new(key, "Pulling…")              // key + pending
    .on_success(|r: &PullResult| Final::message_info(
        format!("Pulled {} commits. Press ^U to push.", r.count)))
    .on_failure(|e: &PullError| Final::message_error(
        format!("Pull failed: {e}")));
```

`Final::Clear` is used where a final message would be noise (e.g. an op whose result
is shown another way). It is a method call the reader sees, not an omission.

### 3.2 Resolution runs where the result is in scope (no typed registry)

The object bridges dispatch→completion by running its closure **where the typed result
already exists**, then carrying back only the formatted `Final`. This avoids storing
heterogeneous `StatusOp<T,E>` values in one registry (no `dyn Any`, no downcasts).

- **Async worker ops.** A dispatch primitive

  ```rust
  fn spawn_status_op<T, E>(
      &mut self,
      op: StatusOp<T, E>,
      work: impl FnOnce() -> Result<T, E> + Send + 'static,
  )
  ```

  emits the keyed pending (`op.key`, `op.pending`) immediately, spawns `work`, and on
  the worker thread computes `op.success(&t)` / `op.failure(&e)` into a `Final`. That
  `Final` (with `op.key`) rides back through the operation's existing `WorkerEvent`
  variant in a new `status_final` field. The engine's central
  `process_worker_event` match performs its domain mutation exactly as today and then
  emits the carried keyed final. The closures are defined at the dispatch site and
  moved into the worker; nothing is stored between begin and resolve.

- **Synchronous / inline ops** (most web command replies). `begin` and `resolve` are in
  the same scope, so the object is constructed, the pending returned (or shown), the
  work run inline, and `op.resolve(outcome)` produces the keyed final right there.

**Single assumption this imposes:** success/failure text formats from the worker's own
`Result<T, E>` plus context captured at dispatch, *not* from engine state mutated after
the worker completes. The audit confirms today's messages already build this way (the
result payloads — `PullCompleted`, `AgentLaunchReadyData`, `WorktreeRemoveCompleted`,
etc. — already carry what the text needs), so this is not a practical limitation. If a
future op truly needs post-mutation state in its message, it captures the needed inputs
into the closure at dispatch or uses the inline path.

### 3.3 Enforcement: seal the raw busy emitters

The guarantee comes from removing every other way to show a pending status:

- TUI: `App::set_busy` is removed. `set_info` / `set_warning` / `set_error` remain for
  genuinely one-shot terminal messages (no pending phase), but `StatusTone::Busy` is no
  longer reachable through them.
- Engine: `StatusUpdate::busy` is removed. The `EventReaction::Status` path no longer
  accepts a `Busy` tone except as the output of a `StatusOp` begin.
- Web: `WireStatus` with `tone == "busy"` can only be produced by `StatusOp` /
  the keyed begin helper; the free `WireStatus::keyed(.., "busy", ..)` and
  `WireStatus::new("busy", ..)` constructors are made private / removed from the public
  surface.
- The low-level `KeyedStatusController::set(.., Busy, ..)` becomes crate-private and is
  called only by the `StatusOp` machinery.

After this, the compiler rejects any new "show a spinner" that did not go through a
`StatusOp` (which requires `.on_success`/`.on_failure`). That is the practical
"you cannot code the bad scenario" outcome.

### 3.4 One key constructor

`status_keys` currently exposes prefix constants that each call site interpolates
(`format!("{DELETE_PREFIX}:{id}")`) — the literal cause of leaks B2/B3/B4 (a final that
built a slightly different key or no key). Replace the prefixes with typed constructors:

```rust
pub fn delete(session_id: &str) -> StatusKey
pub fn create(project_id: &str) -> StatusKey
pub fn launch(session_id: &str) -> StatusKey
pub fn push(worktree_path: &str) -> StatusKey
pub fn pull_project(project_id: &str) -> StatusKey
pub fn pull_session(repo: &str) -> StatusKey
pub fn checkout_default(project_id: &str) -> StatusKey
pub fn add_project_checkout(path: &str) -> StatusKey
pub fn pr_lookup(project_id: &str) -> StatusKey
pub fn commit_msg(session_id: &str) -> StatusKey
```

Because a `StatusOp` owns one `StatusKey`, the pending and final cannot use different
keys even if a developer tried.

### 3.5 Web client single-channel routing (the reported-bug fix)

Independently of the Rust refactor, the web client must stop discarding the key on the
synchronous command-result channel:

- `crates/dux-web/web/src/lib/types.ts`: add `key?: string | null` to `CommandStatus`.
- `crates/dux-web/web/src/lib/store.ts` `onCommandResult`: call
  `showStatusToast(status.key, status.tone, status.message)` instead of `undefined`.

This makes the synchronous command channel correlate exactly like the async `status`
channel, closing the entire "command-reply busy stranded on the anonymous toast" class
— including the worktree-delete spinner on its normal path. The server already
serializes `WireStatus.key`, so no server change is needed for this piece.

### 3.6 Guardrails (Layer 2 — make residual leaks loud)

1. **Runtime logging.** When `KeyedStatusController::tick` upgrades a timed-out `Busy`
   to a `Warning`, log the leaked key + original message at error level to `dux.log`.
   A leak becomes diagnosable instead of silent.
2. **Pairing test harness.** A table-driven test in `dux-core` enumerates every keyed
   operation and, for each, drives dispatch → simulated worker completion across
   `Ok`, `Err`, and the known edge outcomes (session-already-gone, `SessionMissing`,
   `StartupAutoReopen`, `ResumeFallback`), asserting the controller holds **no residual
   `Busy` for that key** afterward. A new operation that forgets a final fails CI.
3. **No bare-busy test.** A source-level test (or `#[deny]`-style guard) asserts the
   sealed constructors stay sealed, so the enforcement in 3.3 cannot be silently undone.

## 4. Affected components

- `crates/dux-core/src/statusline.rs` — `StatusOp`, `Final`, `StatusKey`; controller
  `set` becomes crate-private; tick logs leaked keys.
- `crates/dux-core/src/engine/events.rs` — `StatusUpdate` busy removal; `status_final`
  on completion reactions; engine emits keyed finals on all paths (B1/B5/B6).
- `crates/dux-core/src/engine/command.rs`, `spawn_worker.rs` — `spawn_status_op`
  primitive; dispatch sites construct `StatusOp`s.
- `crates/dux-core/src/wire.rs` — typed `status_keys`; `drive_*_followup` emit keyed
  finals on every branch (B2/B3/B4); sealed busy `WireStatus` constructors.
- `crates/dux-core/src/worker.rs` — `status_final` fields on keyed completion events.
- `crates/dux-tui/src/app/mod.rs`, `workers.rs`, `sessions.rs` — remove `set_busy`;
  keyed-op success paths apply the engine's keyed final, not anonymous `set_info`
  (A1/A2/A3); guard reload-config busy on actual spawn (A4).
- `crates/dux-web/web/src/lib/{types.ts,store.ts}` — command-result key routing (3.5).

## 5. Testing strategy

- Unit tests for `StatusOp`/`Final` builder and resolution (success, failure, clear).
- The pairing harness in §3.6.2 (the primary regression gate).
- A web client test (`storeStatusToasts.test.ts`) asserting a keyed command-result
  busy is dismissed by its matching-key async final (reproduces and locks the
  worktree-delete fix).
- Per-leak regression tests for the Appendix A items that need bespoke setup
  (session-already-gone delete; reload double-trigger; create/launch edge variants).
- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## 6. Rollout order (for the implementation plan)

1. Web client key fix (§3.5) — smallest, fixes the reported symptom immediately, ships
   independently of the Rust refactor.
2. Typed `status_keys` constructors (§3.4) — mechanical, no behavior change.
3. `StatusOp` / `Final` / `StatusKey` types + controller sealing (§3.1, §3.3) behind the
   existing call sites.
4. `spawn_status_op` + `status_final` round-trip (§3.2); migrate operations one family
   at a time (delete, pull/push, create/launch, checkout, pr-lookup, commit-msg,
   project persistence), each with its pairing test.
5. Guardrails (§3.6): tick logging + the full pairing harness + the sealed-constructor
   test.
6. Docs: update `docs/server-mode-summary.md` and any status-system notes to describe
   the `StatusOp` contract (the summary doc currently describes the *prior* keyed
   convention, which this supersedes).

## Implementation status (2026-06-23)

**Landed (all confirmed leaks fixed, each with a test; full workspace clippy +
test suite green):**

- C1 — web client honors the `command_result` key (the reported worktree-delete
  spinner). `1473dab`
- B1 — delete resolves its busy when the session was already removed. `6f8e0e3`
- Guardrail — the controller logs the offending key when a busy times out. `d4c40aa`
- A1 — create-agent success/failure finals are keyed so they replace the busy. `fd5c1c2`
- A3/A4 — startup-log success and reentrant-reload no longer strand a busy. `98ba44a`
- B2/B3/B4 — web clears the keyed busy for checkout-default, add-project-checkout,
  and PR-lookup completions (key derived from the raw `WorkerEvent`). `25102f3`
- B5/B6 — web clears the create/launch busy when a launch resolves to a vanished
  session or startup auto-reopen. (latest)
- A5/A6 — TUI clears the worktree-list and session-missing race busies. (latest)
- Partial §3.4 — typed `status_keys::{create,checkout_default,add_project_checkout,
  pr_lookup,launch}` constructors so a busy and its final share one key source.

**False positive:** A2 (commit-message) — the TUI runs its own `input.rs` worker
with an anonymous busy that its anon finals pair; the keyed `commit-msg:{id}` busy
only exists on the web, where it is already cleared. No leak.

**Known residual edges (rare; now self-heal to a logged timeout warning, no
immortal spinner):** A7 (thread-spawn failure — fixing it safely would move the
busy emission onto the worker thread and disturb the FIFO busy-before-completion
ordering), B7 (begin-delete `AlreadyInFlight` unkeyed error — cosmetic, not a
busy), and PR-lookup *failure* (the failure event carries no project id).

**Not yet done — the structural prevention (§3.1–3.3):** the `StatusOp` closure
object and the sealing of the raw busy emitters (`set_busy`, `StatusUpdate::busy`,
the free busy `WireStatus` constructors), plus the §3.6.2 pairing test harness.
This is the large, invasive piece that makes a dangling busy *inexpressible*; it
is pending an explicit go-ahead because it removes public API across three crates.

## Final outcome (2026-06-25) — seal complete

Every operation that shows an indeterminate ("loading") status was migrated onto
the `StatusOp` object, and the raw busy emitters are sealed:

- **24 operations migrated** across every integration shape: pure-status
  (`push`, `open-path`), domain-ful carry-`ResolvedFinal` (`pull`), TUI-spawned
  (`rename-branch`, clipboard, startup-logs, rerun-startup), separate
  `StatusOpCompleted` with dispatch-context capture (commit-message TUI+web),
  handler-resolved `HandlerStatusOp` for post-worker/3-way outcomes (the 6
  project-persistence ops, auth-users, worktree-list, the 3 web sync ops, delete
  per-surface, the TUI checkout/branch-inspection chain), engine-resolved Multi
  sibling with progress re-emit (agent create/launch), PR-resolve, TUI reconnect,
  and server-flip / config-reload.
- **`StatusOp` shapes:** `status_op(pending).on_success(..).on_failure(..)`
  (worker-resolved), `.resolve_in_handler(|&Outcome| Final)` (handler-resolved,
  N-way), and `HandlerStatusOp::progress` (mid-flight re-emit). Ids are opaque
  and auto-minted — consumers never author a key, killing key-drift at the source.
- **Sealed:** `App::set_busy` is `#[cfg(test)]`-only; `StatusUpdate::busy` is
  `pub(crate)` (no surface crate can construct a bare busy — only the `status_op`
  module does, via `pending_status`/`progress`); the web's only production busy
  path is `WireStatus::from_update` fed by a sealed `StatusUpdate`. A dangling
  loading status is now inexpressible in surface code.
- **Three enforcement layers:** (1) compile-time — the typestate builder forces
  both outcomes and the sealed constructors prevent bypass; (2) test-time — each
  migrated op carries a pairing test asserting its busy resolves; (3) runtime —
  the controller upgrades+logs any leaked busy (keyed or anonymous) at the 20s
  timeout, so even a hypothetical leak self-heals and is diagnosable in `dux.log`.

A handful of completion events that became status-only after migration were
deleted rather than kept (`PushCompleted`, `StartupCommandRerunCompleted`); the
create-progress hand-keyed fallback was dropped (a stale tick after the op
resolves is simply ignored).

## Appendix A — Confirmed leak inventory (implementation checklist)

Verified by three parallel audits of the TUI, the web `WireStatus` stream, and the web
client. Each must end with its pending status replaced/cleared on every path.

### Web client (systemic)

- **C1** `onCommandResult` discards the key (`store.ts:500`), so any synchronous
  command-reply busy strands on the anonymous toast while its async final updates a
  different toast id. Fix per §3.5. *This is the normal-path cause of the reported
  worktree-delete bug.*

### Web server (`crates/dux-core/src/wire.rs`, `engine/events.rs`)

- **B1** Delete, session-already-gone: `drive_delete_followup` returns `vec![]`
  (`wire.rs:1402`) when the session was removed before `WorktreeRemoveSucceeded`. Key
  `delete:{id}` gets no final/clear. (`WorktreeRemoveCompleted` already carries
  `our_busy_message` but the web actor never uses it.)
- **B2** PR lookup: key `pr-lookup:{id}` (`wire.rs:1185`) is never cleared when it hands
  off to create (`create:{id}`); error branch `wire.rs:1239` is unkeyed.
- **B3** Add-project "Check Out & Add": key `add-project-checkout:{path}`
  (`wire.rs:1100`); all finals unkeyed (`wire.rs:1315/1319/1320`, `events.rs:1591`).
- **B4** Checkout default branch: key `checkout-default:{id}` (`wire.rs:1051`); all
  finals unkeyed (`events.rs:1577/1591/1642`).
- **B5/B6** (edge) create `create:{id}` / launch `launch:{id}` resolving to
  `SessionMissing` / `StartupAutoReopen` / `ResumeFallback` → `vec![]`
  (`wire.rs:486/537`).
- **B7** (minor) begin-delete `AlreadyInFlight` emits an *unkeyed* error
  (`wire.rs:1375`) instead of replacing the in-flight `delete:{id}` busy.

### TUI (`crates/dux-tui/src/app/`, `engine/`)

- **A1** Create agent / fork success: keyed busy `create:{id}`
  (`command.rs:459`/`events.rs:1227`); success path writes the **anonymous** slot
  (`workers.rs:866`, also `844`/`861`) → keyed entry never replaced.
- **A2** AI commit message (engine path): keyed busy `commit-msg:{id}`
  (`command.rs:914`); completion handlers write anonymous finals (`workers.rs:246/258`).
- **A3** Open startup-command logs success: anon busy (`sessions.rs:1671`) with no final
  on success (`StartupLogArrived`, `workers.rs:511`).
- **A4** Reload config: `set_busy` runs unconditionally after `apply_reaction`
  (`mod.rs:2268`); the reentrant-reject (`command.rs:773`) and writer-busy
  (`command.rs:787`) early returns spawn no worker, so the busy is never cleared.
- **A5** (edge) Load project worktrees: final gated on the picker still being open and
  matching (`workers.rs:282-307`); a dismiss/switch race strands the anon busy.
- **A6** (edge) Agent reconnect resolving to `SessionMissing` (`workers.rs:869`) sets no
  final.
- **A7** (edge) Keyed busy enqueued before `thread::spawn` (`spawn_worker.rs:99`); a
  spawn failure returns an *unkeyed* error (`spawn_worker.rs:148`).

### Note on the stale doc comment

`StatusUpdate.key` is documented as "Ignored by the TUI today" (`events.rs:36`), but the
TUI *does* honor it now (`workers.rs:217` writes the keyed slot). The comment is stale
and is corrected as part of this work; the A1/A2 leaks are real precisely because the
TUI honors the key.
