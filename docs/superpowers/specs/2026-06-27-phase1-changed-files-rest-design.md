# Phase 1: changed files over REST + events

Date: 2026-06-27
Status: Draft for review (revised after adversarial review)
Parent: [REST-first architecture](./2026-06-27-rest-first-architecture-design.md)

## Why this is phase 1

It fixes the originally reported bug (the changes pane stuck on "Loading
changes..." forever, and the desktop+phone clobber) and is the first end-to-end
proof of the REST + events pattern every later phase reuses.

## Root cause being fixed

The changes pane shows a spinner whenever the broadcast ViewModel's global
`watched_session_id` differs from the client's locally selected session. The
spinner is rendered in `crates/dux-web/web/src/components/ChangedFiles.tsx:274-288`,
gated by the pure helper `shouldShowChangedFiles`
(`crates/dux-web/web/src/lib/changedFiles.ts:75`). Because `watched_session_id`
is single, global engine state (field at `crates/dux-core/src/engine/mod.rs:138`,
mutated by `set_watched_session` at `mod.rs:965`), a second client (or the auto
`selectSession(null)` on a pruned terminal, or a reconnect race) overwrites it
and the loser never recovers.

After phase 1 the changed-files data no longer depends on any global watch. Each
client GETs the session it selected; a per-session event tells it when to
refetch.

## Scope

In:
- `/ws/events` socket: subscribe/unsubscribe, per-connection interest, carrying
  resource-change events only, run as a single `select!` loop.
- `GET /api/v1/sessions/:id/changes`.
- A `ChangesService`: per-session cache (incl. error entries), single-flight
  compute, a **SQLite-persisted per-session `rev`** chokepoint, interest-driven
  supervised poller, `invalidate(session_id)` for git mutations, event emission.
- **Status-toast scoping (the F2 leak fix):** a `scope` field on the engine's
  `WireStatus`, per-`/ws`-connection origin correlation, and per-connection
  delivery filtering, so one client's operation toasts stop appearing on every
  client. (See "Status-toast scoping" below.) Phase 1's dux-core touches are
  therefore: the status-path scope plumbing (`StatusScope` on
  `StatusUpdate`/`WireStatus`/`KeyedWireStatus`/`ResolvedFinal` + a transient
  `current_origin`), and the `changes_rev` storage table/accessor. Both are
  additive.
- Frontend: a `changes` store slice fed by REST + events; migrate **all** five
  changed-files consumers off `viewModel.changed_files`; suppress non-owned
  scoped toasts.

Out (later phases): re-homing `/api/git/*` and `/api/file/*` under `/api/v1`
(kept as-is in phase 1, but called from the new invalidation path); removing the
ViewModel `changed_files` field (cutover); PTY split; deep-linking;
commit-message snapshot per-session; moving status events onto `/ws/events`
(in Phase 1 they stay on the legacy `/ws`, just scope-filtered).

## Backend design

### EventBus (`crates/dux-web/src/event_bus.rs`)

Named `event_bus.rs` to avoid colliding with the existing
`crates/dux-core/src/engine/events.rs`. Held in `AppState` as `Arc<EventBus>`
beside `engine` (a pure web-layer concern; no change to dux-core or the engine
actor — this revises the architecture spec's earlier "on the actor" phrasing).

```rust
pub enum Event {
    Resource { event: String, id: Option<String>, rev: Option<u64> },
    // Phase 1 carries resource-change events only. Status/toast events stay on
    // the legacy /ws (scope-filtered in Phase 1) and may move onto /ws/events
    // in a later phase.
}
// Note: there is NO bus variant for lag recovery. A lagged connection
// synthesizes its catch-up frames locally (see the /ws/events Lagged handler);
// putting a "resync" on the broadcast bus would fan one slow connection's
// recovery out to every connection and could itself fill the buffer.
pub struct EventBus {
    tx: tokio::sync::broadcast::Sender<Event>,        // EVENT_BUS_CAPACITY = 1024
    interest: Mutex<HashMap<String /*topic*/, usize>>,
}
impl EventBus {
    pub fn subscribe(&self) -> broadcast::Receiver<Event>;
    pub fn emit(&self, ev: Event);
    pub fn add_interest(&self, topic: &str);
    pub fn drop_interest(&self, topic: &str);         // saturating; logs on underflow
    pub fn interested_sessions(&self) -> Vec<String>;
}
```

The `rev` chokepoint is NOT on the EventBus; it is a SQLite-persisted per-session
counter owned by the `ChangesService` (see below), so it survives restarts with
no wall-clock dependency.

### `/ws/events` handler (`server.rs`, gated)

At upgrade: run `same_origin_allowed`, acquire a `ws_semaphore` permit, set
`max_message_size(MAX_WS_MESSAGE_SIZE)`, read `recheck_user` for the revocation
loop. (No `connection_id` is needed on this socket; status scoping operates on
the legacy `/ws` connection that actually carries toasts — see "Status-toast
scoping".)

The connection runs as **one `tokio::select!` loop** over: inbound frames, the
`EventBus` receiver, and the recheck timer. One owner of the subscription
`HashSet<String>`; one cleanup path on loop exit drains all held fine topics.
There is no separate forwarder task (this removes the double-decrement and
forwarder-dies-but-handler-lives leaks).

- On `subscribe` to `session:<id>:changes`: validate the session exists
  (`engine.session_worktree(id).await`); if unknown, ignore. Else `if
  set.insert(topic) { bus.add_interest(topic) }`. Enforce per-frame (<=64) and
  per-connection (<=64) topic caps.
- On `unsubscribe`: `if set.remove(topic) { bus.drop_interest(topic) }`.
- On a `Resource` event from the bus: forward only if the topic is in this
  connection's set.
- On `RecvError::Lagged(n)`: log and continue (never `break`); then, for every
  fine topic in this connection's set, write a catch-up frame **directly to this
  connection's own WS sink** (a synthetic `session.changes {id}` with the current
  cached rev) inside the `select!` loop. Do not route catch-up through the
  broadcast bus — that would fan one slow connection's recovery out to every
  connection and could itself fill the buffer.

### ChangesService (`crates/dux-web/src/changes.rs`)

```rust
enum Cached { Ok { rev: u64, prev: (Vec<ChangedFileView>, Vec<ChangedFileView>) },
              Err { rev: u64, at: Instant, message: String } }
pub struct ChangesService {
    engine: EngineHandle,
    bus: Arc<EventBus>,
    cache: Mutex<HashMap<String, Cached>>,
    inflight: Mutex<HashMap<String, tokio::sync::watch::Receiver<bool>>>, // single-flight (see below)
}
```

**`rev` source (SQLite-persisted, via the engine).** The project's only SQLite
layer is **`rusqlite`** (synchronous `Connection`, `crates/dux-core/src/storage.rs`);
there is no `sqlx`/async pool. The engine owns that `Connection` and serializes
all access on its actor thread, so the rev counter lives there, not in the web
layer (this also avoids multi-connection `SQLITE_BUSY`). Concretely (a second,
small dux-core touch alongside `WireStatus.scope`):
- Add a housekeeping table via `SessionStore` (`storage.rs`; the struct is
  `SessionStore { conn: Connection }`), separate from the session records:
  `CREATE TABLE IF NOT EXISTS changes_rev (session_id TEXT PRIMARY KEY, rev INTEGER NOT NULL)`.
- Add `SessionStore::next_changes_rev(&self, session_id: &str) -> rusqlite::Result<u64>`,
  a single atomic upsert run with `self.conn.query_row(...)`:
  `INSERT INTO changes_rev(session_id, rev) VALUES(?1, 1) ON CONFLICT(session_id) DO UPDATE SET rev = rev + 1 RETURNING rev`
  (`RETURNING` is supported by the bundled SQLite in `rusqlite` 0.39). Delete the
  row when the session is deleted (extend the existing session-delete path).
- Expose `EngineHandle::next_changes_rev(session_id) -> u64` (async wrapper over
  one actor round-trip, like `session_worktree`). `ChangesService` calls it.

This is the single chokepoint; it is monotonic per session and survives restart
with no wall-clock dependency.

`compute(session_id)` is **async, two-stage** (the resolve cannot run inside
`spawn_blocking`):
1. `let worktree = PathBuf::from(self.engine.session_worktree(id).await.ok_or(GitError::SessionNotFound)?)`
   on the async thread (mirrors `resolve_worktree` in `git_routes.rs:70`).
   `session_worktree` returns `Option<String>`, so use `.ok_or(...)?` — a bare `?`
   on an `Option` inside a `Result`-returning fn does not compile on stable Rust.
2. `spawn_blocking(move || dux_core::git::changed_files(&worktree))` for the git
   work, then **sort** staged and unstaged by `(path, status)` before comparison
   (the function returns git-order, not sorted; sorting makes change-detection
   stable and avoids spurious or missed events).

**Single-flight (lost-wakeup-safe):** `compute_cached(id)` takes the `inflight`
lock. If an entry exists, clone its `watch::Receiver<bool>`, drop the lock, and
`rx.wait_for(|done| *done).await`, then read and return from the cache. Otherwise
create a `watch::channel(false)`, insert the receiver, drop the lock, run the
compute, store the result in the cache, **remove the inflight entry under the
lock**, then `sender.send(true)`. A `watch` receiver observes the final value
even if it starts waiting after the send (unlike `tokio::sync::Notify`, whose
`notify_waiters()` is lost to any waiter that arrives after it fires — that
pattern would hang late waiters). This bounds concurrent git to one per session
regardless of how many GETs or poll ticks arrive.

The cleanup (remove the inflight entry + `sender.send(true)`) MUST run on **every**
exit path, including future cancellation (an HTTP client disconnect or request
timeout drops the compute future at its `.await`). Wrap it in a drop guard
(`scopeguard::defer!` or a small `impl Drop`); a happy-path-only sequence would,
on cancellation, strand the inflight entry. **Waiter recovery:** after
`rx.wait_for(|done| *done).await`, a waiter MUST re-check the cache; if the owner
was cancelled before storing (cache still absent for this session), the waiter
falls through and starts its own compute (loop) rather than returning an
empty-cache error. So a cancelled owner never propagates a spurious error to the
others — they simply re-elect a new owner.

**Change detection + rev:** after a successful compute, compare the sorted lists
to the cached `prev`. If different (or no entry), `let rev = self.engine.next_changes_rev(id).await;`
store `Ok { rev, prev }`, and `bus.emit(session.changes { id, rev })`. If a
session was previously in `Err` and now succeeds, emit even if the lists match
(so an error-state client recovers). **Store conditionally:** re-read the entry
under the lock before writing and keep the entry with the higher `rev` (a slow
compute must not overwrite a newer one).

**Errors:** on git error, store `Err { rev: self.engine.next_changes_rev(id).await, at, message }` (an error
is cached, with a short TTL e.g. 2s, so repeated GETs during a lock don't each
spawn git and so the GET keeps returning the error rather than stale `Ok` data).
Log the error to `dux.log`; after N consecutive errors raise a keyed `Warning`
status (cleared on next success).

**get(session_id)** -> `Result<ChangesResponse, GitError>`: returns the cached
entry (Ok or Err) if fresh; else single-flight compute. On a fresh `Err` entry,
the handler returns `409 + Retry-After`.

**invalidate(session_id):** called by the git/file mutation handlers in
`git_routes.rs`/`file_routes.rs` after a successful stage/unstage/discard/
commit/write, so the pane refreshes immediately instead of waiting up to 10s for
the poller. It drops the cached `prev` (forcing the next compute to detect a
change) and triggers a compute+emit.

**Poller:** a supervised **async tokio task** (NOT `Engine::spawn_loop_worker`,
which is a dux-core method that runs a synchronous body on an OS thread and posts
to the engine's worker channel — it cannot `await` `session_worktree` or use
async fan-out). Drive it with `tokio::time::interval`; if the task panics, log
and restart it with backoff (own its `JoinHandle`). Each tick, for every
`bus.interested_sessions()`, run `compute_cached` with a bounded fan-out
(`buffer_unordered(8)`) and a per-session timeout, so one slow or locked repo
cannot stall the others. Cadence is 2s when an agent is active, else 10s, read
via a new **required** `EngineHandle::has_active_processes() -> bool`, backed by
cloning the existing `Arc<AtomicBool>` (`engine/mod.rs:139`) into the handle at
channel-build time (`build_actor_channels`) so the read is a local atomic load,
no actor round-trip. (Do not fall back to a fixed 2s — that polls idle sessions
5x too often.) Evict a session's cache entry when its interest reaches zero
(after a short grace) and when the session is deleted.

### `GET /api/v1/sessions/:id/changes` (`crates/dux-web/src/changes_routes.rs`)

- 404 if `engine.session_worktree(id)` is `None` (reuse `resolve_worktree`).
- 200 with a dedicated `ChangesResponse { rev: u64, staged: Vec<ChangedFileView>,
  unstaged: Vec<ChangedFileView> }`. Do **not** serialize `ChangedFilesView` (it
  carries `watched_session_id`, the global field we are removing, and lacks
  `rev`). The per-file `ChangedFileView` (`crates/dux-core/src/viewmodel.rs:199`)
  is reused unchanged.
- 409 + `Retry-After` on a git lock/rebase error (logged first).
- `:id` length-bounded before lookup. Route added to the gated sub-router.

### Status-toast scoping (the F2 leak fix)

Today operation statuses are emitted by the engine onto the global `status_tx`
broadcast (`WireStatus`) and every client's legacy-`/ws` forwarder delivers it,
so one client's toasts appear on all. Statuses are minted at several sites, which
is why scoping must be carried by the core status type rather than bolted on at
one place:
- synchronous command results (e.g. commit) returned via `WireCommandOutcome.status`;
- deferred keyed ops (push/pull) via `spawn_status_op` whose final lands seconds later;
- `spawn_command_worker` busy statuses (e.g. create-agent's "Creating agent…")
  posted as `WorkerEvent::CommandWorkerStarted(StatusUpdate)` — these have **no**
  command-dispatch context at emit time, so an origin passed only to `apply_wire`
  would miss them.

Fix, end to end (scope is a property of a status from creation to wire):

1. **dux-core status types carry scope.** Add `enum StatusScope { All, Connection(String) }`.
   Add `scope: StatusScope` (default `All`) to the core `StatusUpdate`
   (`engine/events.rs`), to `WireStatus` (`wire.rs`, `#[serde(default)]`), to the
   keyed snapshot entry `KeyedWireStatus` (`statusline.rs`), AND to
   `ResolvedFinal` (`status_op.rs`) — the type that carries a deferred op's
   final across the worker channel; omitting it would let push/pull/commit-message
   finals leak as `All` even when their busy was scoped. Additive
   and default `All`, so engine-internal/spontaneous statuses (agent crash, branch
   move, config reload) and the **TUI** (which ignores `scope`) are unaffected —
   audit the TUI status path to confirm it compiles and behaves identically.
2. **Origin is set once per command, read by every mint site.** Do NOT change the
   signature of the synchronous `Engine::apply_wire` (it has many non-web/test
   call sites). Instead: add `origin: StatusScope` to the web-only
   `EngineRequest::ApplyWire`; the engine-actor handler sets a transient
   `engine.current_origin` field to it for the duration of processing that
   command and resets it to `All` after. Every status minted while it is set
   (the sync outcome, `op.pending_status()`, `spawn_status_op`, and
   `spawn_command_worker`'s `busy_status`) stamps `scope = current_origin` at
   creation. For deferred finals, `spawn_status_op` and the direct
   `StatusOpCompleted` paths in `command.rs` capture `current_origin` **before**
   spawning their worker thread and set it on the `ResolvedFinal`;
   `ResolvedFinal::into_reaction` copies that scope onto the emitted
   `StatusUpdate` (by the time the worker completes, `current_origin` has been
   reset to `All`, so the scope must travel on `ResolvedFinal`, not be re-read).
   Statuses minted with no command in flight default to `All`.
3. **Correlate at the edge.** Assign each legacy `/ws` connection a random
   `connection_id` at upgrade (server-assigned, never client-supplied); a command
   from that connection sets `EngineRequest::ApplyWire.origin = Connection(id)`.
4. **Filter on BOTH delivery paths.** (a) The live per-`/ws`-connection status
   forwarder delivers a `WireStatus` only when `scope == All` or
   `scope == Connection(its own id)`. (b) The **on-connect status snapshot**
   (`status_frames(engine.status_snapshot())`, `server.rs`) must apply the SAME
   filter using the connecting connection's id — otherwise a client connecting
   mid-operation receives another connection's in-progress `Busy` and shows a
   ghost spinner that never clears. Both paths use one shared predicate.
5. **Frontend:** unchanged rendering; suppression is server-side, so a client
   stops receiving other clients' operation toasts while still getting `All` ones.

Footprint: this is several touches in the dux-core status path (scope on
`StatusUpdate`/`WireStatus`/`KeyedWireStatus`, the transient `current_origin`,
and the mint sites) plus the web-layer correlation/filtering — larger than a
one-line change, but it is the price of fixing the leak in Phase 1 and is
independent of the changed-files vertical, so it can land as its own commit.

## Frontend design

### `lib/eventsSocket.ts` (new)

Client for `/ws/events`: `subscribe(topics)`, `unsubscribe(topics)`,
`onEvent(ev)`. Maintains the full current subscription set, including app-wide
coarse topics (`sessions`, `projects`, `config`) subscribed on mount and the
per-screen fine topics. Reconnect (same backoff as the PTY socket): on (re)open,
re-send the complete set, then the store triggers a GET for each restored topic.

### `lib/changesApi.ts` (new)

`fetchChanges(sessionId): Promise<SessionChangesResponse>` -> `GET
/api/v1/sessions/:id/changes` via `fetch` with `credentials: "same-origin"` (the
pattern in `lib/git.ts`, which uses GET; note `lib/fileApi.ts` uses POST and is
NOT the model here). `SessionChangesResponse = { rev: number; staged:
ChangedFileView[]; unstaged: ChangedFileView[] }` is a new type; do not reuse the
existing `ChangedFiles` type (it has `watched_session_id`, not `rev`).

### Store (`lib/store.ts`)

New slice: `changes: { sessionId, phase: "idle"|"loading"|"loaded"|"error",
rev, staged, unstaged, error }`. This is the single source for changed-files data
across the app.

- `selectSession(id)` / `selectTerminal`: `unsubscribe` the previous
  `session:<prev>:changes`, `subscribe(["session:"+id+":changes"])`, set
  `phase:"loading"`, then `fetchChanges(id)`. (Replaces the four
  `watch_changed_files` sends; there is no global watch to clear, so the
  prune-null clobber is gone by construction.)
- On a `session.changes {id, rev}` event: if `id === selectedSessionId` and `rev
  >= changes.rev`, `fetchChanges(id)`. **When `phase === "error"`, always
  refetch** (the error path has no usable rev; this avoids the
  `rev > undefined` trap and lets the pane self-heal).
- On a `fetchChanges` response: apply to the slice **only if `resp.rev >=
  changes.rev`** (ignore an older response that lost the race) and only if
  `resp` is for the still-selected session.
- Lag catch-up arrives as an ordinary `session.changes {id}` event (the server
  writes it directly to the lagged connection's sink), so the same
  `session.changes` handler above refetches it; no separate event type.
- 404 -> clear the slice; `pruneSelectionIfGone` (already present) handles
  selection.

### Migrate all five consumers off `viewModel.changed_files`

`ChangedFiles.tsx`, `CommitDialog.tsx` (staged count), `ConfirmDiscardFileDialog.tsx`
(auto-close on file leaving unstaged), `MobileShell.tsx` (changes badge), and
`EditorOverlay.tsx` (file-tree status markers + the open-diff staleness signal)
all currently read `viewModel.changed_files`/`shouldShowChangedFiles`. Each now
reads the `changes` slice when `changes.sessionId === itsSessionId`. The
`ChangedFiles.tsx` spinner block (lines 274-288) is replaced by the slice's
phase: `loading`->spinner, `error`->error empty-state with a Refresh button
(re-`fetchChanges`), `loaded`->lists (or "No changes"). The stuck spinner is
impossible: phase tracks a real request, and `error` self-heals on the next
event.

## Error handling and edge cases

- **Session deleted mid-request:** GET 404 -> slice cleared; existing prune
  clears selection.
- **Git lock/rebase:** GET 409 + Retry-After -> error empty-state + Refresh; the
  poller keeps trying and emits `session.changes` on recovery, so the pane
  self-heals (no manual click needed).
- **Event missed (disconnect or broadcast lag):** reconnect re-subscribes +
  refetches; on `Lagged` the server writes a synthetic `session.changes` to that
  one connection's sink, which the standard handler refetches. No edge-triggered
  staleness.
- **Out-of-order GETs:** the `rev >=` apply guard drops the older response.
- **Many clients, one cold session:** single-flight collapses to one compute.
- **No subscribers:** `interested_sessions()` empty -> poller does no git work;
  exact via conditional add/drop interest.
- **Server restart:** the per-session `rev` is persisted in SQLite, so it resumes
  from its last value (never resets to a lower number); post-restart events are
  always >= the client's last applied rev, so the apply guard never wrongly drops
  them.

## Tests

Rust:
- GET 200 (correct lists) / 404 (unknown) / 409 (git error, via index.lock).
- **Two-connection isolation:** conn A subscribed to `session:s1:changes` gets
  `s1` events and not `s2`; conn B the reverse (the literal original bug).
- Interest exactness: duplicate subscribe does not inflate; a **real socket
  close** drains the refcount to zero.
- Single-flight: concurrent GETs on a cold session cause exactly one git compute.
- `rev` strictly increases across detected changes and **persists across restart**
  (reopen the SQLite `changes_rev` table and confirm the next rev continues, not
  resets); identical sorted lists do not emit; a change in only
  additions/deletions (same path+status) DOES emit (lists compared, stat
  included).
- Error caching: repeated GETs during a lock do not each spawn git; recovery
  emits an event.
- `Lagged` -> the connection receives a local catch-up refetch signal (and the
  bus is not used for it).
- Gate test extended to real `/api/v1/sessions/:id/changes`, `/ws/events`, and
  both PTY paths (401 without a session).
- **Status scope (live):** a status tagged `Connection(A)` is delivered to
  connection A and NOT to B; `All` reaches both. Serde test: an old `WireStatus`
  JSON without `scope` deserializes to `All` (TUI / older-peer compatibility).
- **Status scope (deferred + worker-busy):** drive a real `spawn_status_op`
  (push) AND a `spawn_command_worker` busy (create-agent) from connection A and
  assert both their statuses carry `Connection(A)` and reach only A — these are
  the async paths that would silently stay `All` if only `apply_wire` were
  scoped.
- **Status scope (snapshot):** a client connecting while connection A has an
  in-progress `Busy` receives a snapshot containing only `All`-scoped statuses,
  not A's `Connection(A)` busy.

Frontend (vitest):
- Slice machine: loading->loaded; loading->error; error heals on a later event;
  apply-only-if-rev-newer; wrong-session event ignored while a fetch is in
  flight; out-of-order response dropped.
- subscribe-before-fetch ordering (record call order); reconnect re-subscribe +
  refetch (incl. coarse topics).
- Consumer migration: CommitDialog/discard/mobile/editor read the slice.
- Replace `storeWatchChangedFiles.test.ts` and the `watch_changed_files`
  assertion in `storeCreateFocus.test.ts` with events-socket subscription
  assertions.

## Deferred review follow-ups (low severity, post-implementation)

These came out of the adversarial review of the implementation. All high/medium
findings were fixed; these lows are intentionally deferred:

- **Lagged catch-up has no real-socket integration test.** Forcing a deterministic
  `tokio::broadcast` `Lagged` through the live `/ws/events` path needs >1024
  distinct change events (the channel capacity), which would be a slow/flaky
  harness. The catch-up code (synthetic `session.changes` written to the lagged
  connection's own sink, never the bus) is in place; only the end-to-end test is
  deferred.
- **`ws_semaphore` is shared by `/ws` and `/ws/events`.** Resolved by documenting
  the shared cap (each browser uses one of each, so effective tab capacity is
  `max_websocket_connections / 2`) rather than splitting into two configurable
  caps. Split into a separate `max_event_connections` if per-endpoint reservation
  is ever needed.

## Acceptance

- Two browsers (or desktop+phone) on different sessions each show their own
  changes simultaneously, no spinner-strand, no flip-flop.
- Killing the WebSocket and reconnecting restores the pane without manual action.
- A locked repo shows an explicit error and **auto-recovers** when the lock
  clears.
- Staging/committing updates the pane immediately (not after the poll interval).
- One client's operation toasts (push/commit/launch) do not appear on another
  client; genuinely server-wide statuses still reach everyone.
