# REST-first architecture for dux server mode

Date: 2026-06-27
Status: Draft for review (revised after adversarial review)

## Problem

The web server grew around a single broadcast `ViewModel` pushed over one
WebSocket to every client, plus an ad-hoc mix of WebSocket commands and a few
HTTP routes. Two structural problems follow:

1. **Client-view state is stored globally.** The changed-files watch
   (`watched_session_id` + staged/unstaged lists, `crates/dux-core/src/engine/mod.rs:138`)
   lives on the shared engine and rides the broadcast ViewModel. Two clients
   viewing different sessions clobber each other; the loser strands on a
   "Loading changes..." spinner forever. The spinner is rendered in
   `crates/dux-web/web/src/components/ChangedFiles.tsx:274-288`, gated by the
   pure helper `shouldShowChangedFiles` (`crates/dux-web/web/src/lib/changedFiles.ts:75`).
   This is the originally reported bug.
2. **The WebSocket carries bulk data.** Any volatile change re-serializes and
   re-broadcasts the whole ViewModel, including build-static config. The socket
   does work REST should do.

## Mantra (the target contract)

1. **REST is the primary interface.** All reads are `GET`; all actions are
   `POST`/`PATCH`/`DELETE`. This is the scriptable, programmable surface and it
   works with zero WebSocket.
2. **The WebSocket only pushes events.** An event names *what changed*, never the
   value. The client decides whether to issue a REST request in response.
3. **The client subscribes to what it is showing**, and unsubscribes when it
   stops, so the event stream and the server's background work stay proportional
   to what is on screen.
4. **Separate sockets for separate jobs.** A multiplexed events socket, and one
   byte-stream socket per attached PTY.

## Alternatives considered (and why this is bigger than the bug)

The originally reported bug (stuck changes spinner) does **not** require this
architecture. It could be fixed far more cheaply, and we record those options
here so the scope is honest:

- **A. Per-connection watch in the engine actor.** Promote the single
  `watched_session_id` to a `HashMap<connection_id, session_id>` and deliver
  changed-files only to the requesting connection. ~30-50 lines, no new
  transport. Fixes the clobber, nothing else.
- **B. Client retry timeout.** Add a timeout that re-sends `watch_changed_files`
  if the pane is still loading. ~10 lines. Heals the races but leaves the global
  model.

We are deliberately choosing the larger REST-first migration instead, for two
reasons the cheap fixes cannot deliver: **programmability** (a real API third
parties can drive without a browser) and **scaling** (the WebSocket stops
carrying bulk data and per-client view state). If those goals were not in play,
option A would be the right call. Phase 1 still fixes the bug as its first
deliverable; the architecture is justified by the goals beyond it, not by the
bug.

## Goals / non-goals

**Goals**
- Express every client/server interaction as REST + events, with the PTY byte
  stream the single justified exception.
- Eliminate global client-view state (kill the changed-files clobber at root).
- Produce a programmable API third parties can drive without the browser.
- Migrate incrementally, each phase shippable.

**Non-goals (this effort)**
- Per-user isolation / multi-tenant security. The single-tenant trust model in
  `CLAUDE.md` stands: any authenticated client may drive any agent. Nesting is
  for clean middleware and addressability, not mutual distrust.
- Token/API-key auth. We keep cookie-session auth and do **not** build a token
  system now. We only avoid foreclosing it (see Auth) without adding an
  abstraction layer for it.
- UI redesign. Selection/scroll/focus stay client-only and become URL state in a
  later phase (deep-linking).

## Transports and their roles

| Transport | Carries | Endpoint(s) |
|---|---|---|
| REST | all data reads + all actions (the programmable API) | `/api/v1/...` |
| Events WS | server -> client change signals only (no bulk payloads) | `/ws/events` |
| PTY WS | terminal byte stream + input + resize | `/ws/sessions/:id/pty`, `/ws/sessions/:id/terminals/:tid/pty` |

### Two socket shapes, on purpose

- **Events: one multiplexed socket per client.** Signals are tiny and benefit
  from a single ordered stream. A socket-per-resource would explode connection
  count for no gain.
- **PTY: one socket per attached stream, resource-nested.** A byte stream wants
  isolation (no head-of-line blocking between terminals), a trivial lifecycle
  (close = detach, no attach handshake), and path addressability so one
  `/ws/sessions/:id/...` guard middleware wraps both REST and the stream. Agents
  and companion terminals are the same primitive (a `PtyClient`); they differ
  only by address.

### Required protections on EVERY socket (and the gate)

The existing `/ws` handler enforces four protections that the new sockets MUST
replicate (verified in `crates/dux-web/src/server.rs`): they are not optional and
are not inherited automatically by a new upgrade handler.

1. **Origin / CSWSH check** (`same_origin_allowed`, runs *regardless* of whether
   auth is enabled) at the very top of every WS upgrade handler.
2. **Connection-cap semaphore** (`ws_semaphore`, from `max_websocket_connections`)
   acquired before upgrade, held for the connection lifetime; 503 when exhausted.
   Events and PTY sockets share this budget unless a per-family cap is added to
   config.
3. **Frame-size limit** (`ws.max_message_size(MAX_WS_MESSAGE_SIZE)`, 16 MiB) on
   every upgrade.
4. **User-revocation recheck** (`ws_recheck_period`) inside every socket task, so
   a revoked user's already-open socket is closed (the HTTP gate cannot revoke an
   upgraded socket).

All `/api/v1/*` routes and all WS upgrade routes go in the **gated** sub-router.
The gate regression test (`gated_data_route_is_401_without_session`) MUST be
extended to assert that a real `/api/v1/sessions/:id/changes`, `/ws/events`,
`/ws/sessions/:id/pty`, and `/ws/sessions/:id/terminals/:tid/pty` all return 401
without a session (using the actual router, not only the probe-route seam).

## REST conventions

- **Version prefix `/api/v1`.** Existing unversioned routes (`/api/git/*`,
  `/api/file/*`, `/api/me`) are re-homed under `/api/v1` during migration. The
  old paths are kept as **dual-route aliases** (the same handler registered at
  both paths), NOT HTTP redirects: the old routes carry `session_id` in the POST
  body while the new routes carry `:id` in the path, and a 3xx redirect cannot
  transform a body. Aliases are removed in the cutover phase. Reversible by
  removing the alias registration.
- **Resource nesting.** Routes mirror the resource tree
  (`/api/v1/sessions/:id/changes`, `/api/v1/sessions/:id/git/commit`). A single
  path-scoped middleware resolves and validates `:id` (session exists ->
  worktree) once, shared by REST and the nested PTY sockets. For terminal routes
  the middleware MUST also verify `companion_terminals[:tid].session_id == :id`
  (the existing `SubscribeTerminal` path looks up terminals by id alone and does
  not check session ownership).
- **Verbs and status codes.**

  | Case | Success | Errors |
  |---|---|---|
  | `GET` read | 200 | 404 |
  | `POST` create (session/project/terminal) | 201 + `Location` | 400 / 409 |
  | `PATCH` update | 200 (or 202, see deferred) | 404 |
  | `DELETE` | 204 | 404 |
  | `POST` action (git mutation, reconnect, pull, checkout) | 200 | 4xx client-actionable / 5xx unexpected |
  | `POST` async trigger (commit-message generation) | 202 (completion arrives as an event) | 4xx |
  | changed-files read during a git lock/rebase | 409 + `Retry-After` | (not 503: proxies may reroute 503) |

- **Idempotency.** `POST /api/v1/sessions` (create) is not naturally idempotent;
  a retry after a lost 5xx would create a duplicate worktree. It accepts an
  optional `Idempotency-Key` header: the server records the key -> created
  session id for a TTL and returns the same session on replay.
- **Deferred semantics.** A provider change via `PATCH /api/v1/sessions/:id`
  takes effect on next reconnect (it never kills a running agent). The response
  is `200` with `{ "provider_change": "pending_reconnect" }` so callers do not
  assume the live agent switched.
- **Route-collision safety.** Literal sub-paths that could collide with `:id`
  (bulk reorder) use a distinct, non-colliding path: `POST /api/v1/sessions/reorder`
  and `POST /api/v1/projects/reorder` (not `PATCH .../order`). Register literal
  segments before parameterized ones regardless.
- **API evolution policy.** Within `/api/v1`: additive-only changes (new optional
  response fields, new optional request fields) are allowed without a version
  bump. Removing/renaming a field, changing a type, or adding a required request
  field requires `/api/v2`.
- **Off-thread git.** Every handler that shells git runs it via `spawn_blocking`
  (as `git_routes.rs`/`file_routes.rs` already do).

### Auth seam (cookie now)

All `/api/v1/*` routes and both socket families sit behind the existing
`require_auth` gate (the `gate` middleware defined at `crates/dux-web/src/server.rs:412`
and applied at `server.rs:298`), which validates the session cookie. We do **not**
add a token abstraction now; we only keep the gate as a single middleware layer
so a future bearer/API-key check is a localized change. No `AuthIdentity` type is
introduced in this effort.

## Event model

### Envelope

Two shapes on `/ws/events`, distinguished by `event`:

Resource change signal:
```json
{ "event": "session.changes", "id": "s_abc", "rev": 42 }
```
Status (the one event that carries an inline payload, because there is nothing to
GET for an ephemeral toast):
```json
{ "event": "status", "key": "op-7", "tone": "info", "message": "Committed.", "scope": "all" }
```

Rules: a resource event names what changed and carries `rev` where the client
needs ordering/dedup. It never carries the changed value. The client/server
share a single `topic_for_event(&Event) -> Option<String>` mapping (a Rust enum +
a matching TypeScript constant), so the two never drift.

### `rev` (monotonic, SQLite-persisted)

`rev` is assigned from a **single chokepoint** in the `ChangesService`: a
per-session counter persisted in a dedicated `changes_rev` housekeeping table in
the runtime `sessions.sqlite3`, incremented via one atomic upsert
(`INSERT ... ON CONFLICT DO UPDATE SET rev = rev + 1 RETURNING rev`) on each
detected change. Persistence makes it monotonic per session both within a run and
across restarts, with no wall-clock dependency. The row is removed when the
session is deleted.

Client rule, applied uniformly: **apply a GET response or act on an event only if
its `rev` >= the highest `rev` already applied for that session.** This single
rule provides both dedup and out-of-order-response protection (a slow older GET
cannot overwrite newer data). Because the persisted counter never resets to a
lower value, the client never needs to special-case restart.

### Catalog and topic mapping

| Event | Topic it is delivered on | Client reaction |
|---|---|---|
| `projects.changed` | `projects` | re-GET `/projects` |
| `sessions.changed` | `sessions` | re-GET `/sessions` |
| `session.status` `{id}` (lifecycle: status/title/PR; low-frequency) | `sessions` | re-GET `/sessions/:id` |
| `session.working` `{id}` (high-frequency hysteresis flag) | `session:<id>:working` | update the working indicator for that session only |
| `session.changes` `{id, rev}` | `session:<id>:changes` | re-GET `/sessions/:id/changes` if subscribed |
| `session.commit_message` `{id}` | `session:<id>:commit-message` | re-GET `/sessions/:id/commit-message` |
| `terminals.changed` `{id}` (session's terminal list) | `session:<id>:terminals` | re-GET `/sessions/:id/terminals` |
| `config.changed` | `config` | re-GET `/bootstrap` |
| `status` `{key,tone,message,scope}` | delivered to all connections, **filtered by scope** | render/replace/clear a toast |

The high-frequency `working` flag is split onto its own fine topic so coarse
`sessions` subscribers are not woken by it; only a client showing that session's
terminal subscribes to `working`.

### status scope (toast leak fix, Phase 1)

Goal: stop one client's operation toasts (push/commit/launch, including the
persistent error/warning ones) from appearing on every other client. Fixed in
Phase 1, on the legacy `/ws` status path (where toasts ride in Phase 1 — they do
not move onto `/ws/events` yet):

- **dux-core (scope on the core status types):** add
  `enum StatusScope { All, Connection(String) }` and a `scope` field (default
  `All`) on `StatusUpdate`, `WireStatus` (`#[serde(default)]`), the keyed snapshot
  entry, and `ResolvedFinal` (so a deferred op's final carries its origin across
  the worker channel). Additive; engine-internal statuses and the TUI (which ignores
  `scope`) are unaffected (audit to confirm). Status is minted at several sites
  (sync command result, `spawn_status_op` deferred final, `spawn_command_worker`
  busy like create-agent), so scope must live on the status itself, not on one
  call.
- **Correlate (one origin per command, no `apply_wire` signature change):** each
  legacy `/ws` connection gets a server-assigned random `connection_id`; its
  command sets `EngineRequest::ApplyWire.origin`, and the engine-actor handler
  holds it in a transient `current_origin` while processing so every mint site
  stamps it. (The sync `Engine::apply_wire` keeps its signature — many test call
  sites.)
- **Filter (both delivery paths):** the live per-connection status forwarder AND
  the on-connect status snapshot deliver a `WireStatus` only when `scope == All`
  or `scope == Connection(its own id)`. Filtering only the live path would leak
  an in-progress `Busy` to a mid-operation joiner.

This is independent of the changed-files vertical and can ship as its own commit
within Phase 1. Detail and tests are in the phase-1 spec.

### Subscription model

The client opens `/ws/events`, then sends subscribe/unsubscribe frames:
```json
{ "subscribe":   ["sessions", "projects", "config", "session:s_abc:changes"] }
{ "unsubscribe": ["session:s_abc:changes"] }
```

Server rules:
- Each connection owns its subscription set as a `HashSet<String>`. **Call
  `add_interest(topic)` only when `set.insert(topic)` returns true; call
  `drop_interest(topic)` only when `set.remove(topic)` returns true.** This keeps
  the global interest refcount exact under duplicate subscribe frames (the
  reconnect path re-sends the full set).
- **One task per connection, one cleanup path.** The frame-reader and the
  event-forwarder run in a single `tokio::select!` loop (not two tasks), so there
  is exactly one owner of the subscription set and exactly one place that drains
  interests on exit. This avoids the double-decrement underflow and the
  forwarder-dies-but-handler-lives leak. On any loop exit, drain all held fine
  topics.
- **Validate before registering interest.** On `subscribe` to
  `session:<id>:changes`, verify the session exists (`engine.session_worktree`)
  before `add_interest`; drop unknown-session subscriptions silently. This stops
  a client from inflating the poll set with phantom sessions.
- **Bounds.** Reject a subscribe frame with more than N topics (e.g. 64); cap
  total fine topics per connection (e.g. 64); enforce the 16 MiB frame limit.
- **Interest drives polling.** Changed-files polling runs only for sessions with
  a live `:changes` subscriber.

### Snapshot / ordering rule

On (re)connect or when newly subscribing to a topic, the client **subscribes
first, then issues the REST GET.** Because subscribe (WS) and GET (HTTP) travel
on different connections with no cross-transport ordering guarantee, the GET
endpoint additionally treats the request as an implicit subscription confirmation
and the client always issues one unconditional GET per newly-subscribed topic.
Any event in the gap causes at worst a redundant (idempotent) refetch. The
`rev` rule above makes the redundant fetch harmless.

### Broadcast capacity and lag recovery

The events bus is a `tokio::sync::broadcast` channel with an explicit named
capacity (e.g. `EVENT_BUS_CAPACITY = 1024`). The per-connection forwarder MUST
arm `RecvError::Lagged(n)` with **log-and-continue** (never `break`, matching the
existing status forwarders in `server.rs`). Because changed-files data is not
ephemeral and coarse events have no poller heartbeat, on `Lagged` the forwarder
ALSO issues a **catch-up**: it writes a synthetic refetch signal for every topic
the connection is subscribed to **directly to that one connection's sink** (never
back onto the broadcast bus, which would fan one slow connection's recovery out
to all), so the client refetches and recovers without a disconnect.

## The full resource map (the sweep)

Reads are GET; the event that invalidates each is in brackets.

**Bootstrap / config** — `GET /api/v1/bootstrap` (version, providers, welcome
tips, macros, palette commands, `ui.*` flags, gh_available, global_env)
[`config.changed`]; `GET/PUT /api/v1/macros`.

**Projects** — `GET /api/v1/projects`, `GET /api/v1/projects/:id`
[`projects.changed`]; `POST /api/v1/projects`; `PATCH /api/v1/projects/:id`;
`DELETE /api/v1/projects/:id`; `POST /api/v1/projects/reorder`;
`POST /api/v1/projects/:id/pull`, `.../checkout-default`;
`GET /api/v1/projects/:id/worktrees`, `GET /api/v1/projects/inspect?path=`.

**Sessions / agents** — `GET /api/v1/sessions`, `GET /api/v1/sessions/:id`
[`sessions.changed`, `session.status`]; `POST /api/v1/sessions` (create;
`source` for fork; `Idempotency-Key`); `PATCH /api/v1/sessions/:id`;
`DELETE /api/v1/sessions/:id`; `POST /api/v1/sessions/:id/reconnect`;
`POST /api/v1/sessions/reorder`.

**Changes / git** — `GET /api/v1/sessions/:id/changes` [`session.changes`];
`POST /api/v1/sessions/:id/git/{stage,unstage,discard,commit,push,pull}`
(aliases of `/api/git/*`); `POST /api/v1/sessions/:id/commit-message` (202;
ready -> [`session.commit_message`]).

**Files / diff** — keep `/api/file/*`, re-homed under
`/api/v1/sessions/:id/files{,/raw,/diff}`. Empty/missing `path` query is rejected
(400) for `inspect`/`browse` rather than resolving to the server CWD; `browse`
with no path falls back to `$HOME` as today. Path params (`:id`, `:tid`) are
length-bounded (e.g. 128 bytes) before lookup.

**PR** — `GET /api/v1/sessions/:id/pr` [`session.status`].

**Terminals** — `GET/POST /api/v1/sessions/:id/terminals`,
`DELETE /api/v1/sessions/:id/terminals/:tid` [`terminals.changed`]; live I/O on
`/ws/sessions/:id/terminals/:tid/pty`.

**Utility** — `GET /api/v1/browse?path=`, `GET /api/v1/agent-name`.

**PTY** — `/ws/sessions/:id/pty`, `/ws/sessions/:id/terminals/:tid/pty`. Input +
resize are frames on the same socket. `Resize` MUST be gated on the sender being
the current subscriber of that PTY (today any client can resize any PTY).

## Migration (strangler, each phase shippable)

The legacy combined `/ws` and the broadcast ViewModel keep working until the
final phase. New surfaces are added beside them; the frontend switches per
surface; old code is deleted last.

1. **Phase 1 (the bug fix + the pattern's proof).** `/ws/events` with the
   subscription/interest model (carrying resource-change events only);
   `GET /api/v1/sessions/:id/changes`; the `ChangesService` (SQLite-persisted
   `rev`); migrate ALL changed-files consumers off the ViewModel; git-mutation
   invalidation; **and the status-toast scope fix** (`WireStatus.scope` +
   per-`/ws`-connection filtering — its own commit). Detailed in the phase-1 spec.
2. **Bootstrap/config.** `GET /api/v1/bootstrap` + `config.changed`; remove
   static fields from the ViewModel.
3. **Reads for projects/sessions.** REST + list events; drop those arrays from
   the ViewModel.
4. **Actions to REST verbs.** create/delete/fork/rename/reconnect/reorder
   (programmability lands here); alias the old `/api/git/*` and `/api/file/*`.
   (Status scoping already shipped in Phase 1; statuses may optionally move from
   the legacy `/ws` onto `/ws/events` here, carrying their existing `scope`.)
5. **Split the PTY** onto `/ws/sessions/:id/pty` (+ companion path); gate Resize;
   retire PTY on the legacy socket.
6. **Cutover.** Retire the broadcast ViewModel and the legacy `/ws`; remove
   aliases; add deep-linking (`#/agent/:id`).

Sibling global-state cleanups (flagged by review, scheduled, not silently left):
PTY resize gating (phase 5), commit-message snapshot moved from a single global
slot to a per-session map and served via `GET /sessions/:id/commit-message`
(phase 4). The status-toast leak is fixed in **phase 1** (see "status scope"
above).

## Rollback

Each phase is additive. The frontend bundle is embedded in the server binary
(`rust_embed`), so a binary rollback atomically restores both server and
frontend; the legacy ViewModel path is preserved for old-bundle clients during a
rolling deploy, not as a runtime fallback for the new frontend. Phase 6 is the
only irreversible step and ships last.

## Observability (CLAUDE.md status conventions apply to new async paths)

- The changed-files poller runs as a supervised async tokio task (restart on
  panic with backoff via its `JoinHandle`), NOT the dux-core `spawn_loop_worker`
  (which runs a synchronous body on an OS thread and cannot await the engine).
  On repeated git errors for a session it logs to `dux.log` and raises a keyed
  `Warning` status (cleared on next success). The GET handler logs git errors
  before returning 409.
- Every `Busy` status still pairs with a final state; no new path emits an
  unkeyed `Busy`.

## Testing strategy

- **Rust:** per-route handler tests (200/404/4xx); a **two-connection** events
  test (conn A subscribed to `session:s1:changes` gets `s1` events, conn B to
  `s2` does not, and vice versa — the literal original bug); interest exactness
  under duplicate subscribe and under a **real socket close** (refcount returns
  to zero); single-flight (concurrent GETs on a cold session cause one compute);
  per-session `rev` persists across restart (reopen the `changes_rev` table and
  confirm it continues, never resets); `Lagged` -> catch-up;
  status scope filtering (conn A's operation status not delivered to conn B);
  gate test extended to the real new routes.
- **Frontend (vitest):** the changes slice machine (idle/loading/loaded/error,
  error heals on a later event, apply-only-if-rev-newer, ignore wrong-session
  events while a fetch is in flight); subscribe-before-fetch ordering; reconnect
  re-subscribe + refetch; consumer migration (CommitDialog/discard/mobile/editor
  read the slice). Replace the now-obsolete `storeWatchChangedFiles.test.ts` and
  the `watch_changed_files` assertion in `storeCreateFocus.test.ts`.

## Open decisions (resolved)

- PTY endpoints: resource-nested, one socket per PTY. (Confirmed.)
- Auth: cookie only this effort; gate kept as one middleware layer. (Confirmed.)
- Versioning: `/api/v1` with an additive-only evolution policy. (Confirmed.)
- `/ws/events`: built in Phase 1. (Confirmed.)
- `rev`: kept, monotonic via a SQLite-persisted per-session counter (dedicated
  `changes_rev` housekeeping table) at a single chokepoint. (Confirmed.)
- Toast-leak scope filtering: **Phase 1**, via a `WireStatus.scope` field
  (default `All`) and per-`/ws`-connection origin correlation + delivery
  filtering. (Confirmed.)
