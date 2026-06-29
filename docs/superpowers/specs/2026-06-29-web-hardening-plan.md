# Web-mode hardening plan (deferred-items cleanup)

Date: 2026-06-29
Branch: `server-mode` (unreleased)
Status: revised after a 14-charter adversarial review (high/medium findings folded in;
punted lows listed at the end).

This plan implements the six deferred items agreed in chat after the REST-first
migration. Each item is independent and can land as its own commit. All file:line
references were verified against the current tree during the adversarial review.

Scope recap (decisions already made with the user):

1. Close the reconnect "stale window" so a (re)subscribe always converges.
2. Split the single WebSocket connection cap into three independent caps
   (control/events, agent PTYs, terminal PTYs). **User chose three explicitly.**
3. Replace the per-tick full serialize-and-compare spine check with a
   change-gated check plus a slow self-healing backstop. In-memory only.
4. Make PTY viewer cleanup proactive (on disconnect) instead of on-next-output.
5a. Nest the git/file REST routes under `/api/v1/sessions/:id/...`, no aliases.
5b. Make project branch inspection tolerate a detached HEAD.
5c. Dropped (no change).

> The review's biggest lessons, applied throughout: (a) item 3's "two chokepoints"
> claim was **false** — there are four loop-level spine mutators, which both
> justifies keeping the backstop and requires bumping on the extra two; (b) item 5b
> is **much** larger than first scoped — the web add-project/create-agent paths
> differ from the TUI path and there are 7+ `current_branch()` callers; (c) item 2
> touches several files beyond config (the rebind check, the TUI renderer, the
> events inline acquire); (d) several missed frontend **test** files.

---

## Item 1 — Close the reconnect/first-connect stale window

### Problem
A client (re)subscribes to its topics over `/ws/events` and separately refetches
over REST. The two are unsynchronized: if a change is emitted on the bus *before*
the socket's delivery loop has registered the new subscription, the socket's
topic filter (server.rs:1290-1313) drops it, and the just-completed REST GET
already returned the older state. The panel stays stale until the next change.
This race is identical on **first connect and reconnect** — the same code path
handles both.

### What already exists (verified)
- The client's `onOpen` (store.ts:542-561) already refetches spine + bootstrap +
  the selected session's changes on every reconnect (`skipNextEventsOnOpenLoad`
  suppresses only the very first open). So **coarse** topics (sessions/projects/
  config) are already recovered on reconnect; only the precise post-GET/pre-
  subscribe race for the **selected session's changes** is left open.
- There is already a "Lagged recovery" catch-up in `handle_events_socket`
  (server.rs:1322-1376) that, for each subscribed session, sends
  `{event:"session.changes", id, rev: changes.peek_rev(sid)}`. Item 1 reuses this
  exact pattern, triggered on subscribe instead of on lag.
- The client treats a missing `rev` as a force-refetch (store.ts:519,
  `rev === undefined`), and dedups with `rev >= state.changes.rev` (store.ts:520).

### Chosen fix: catch-up on (re)subscribe, **fine topic only**
On processing a `subscribe` frame, for each newly-added `session:<id>:changes`
topic, send that socket a catch-up `session.changes` event carrying
`changes.peek_rev(sid)`.

- **Do not** emit catch-up for coarse topics. `onOpen` already refetches them on
  reconnect, and a coarse catch-up would fire *after* `onOpen` consumed
  `skipNextEventsOnOpenLoad`, causing 2–3 redundant spine/bootstrap loads on the
  initial page load (and a double load on every reconnect). Leaving coarse to the
  existing `onOpen` path is correct and avoids that.
- **Rev source:** `changes.peek_rev(sid)` (changes.rs:403) — in-memory, read-only.
  **Never** `next_changes_rev` (engine_actor.rs:526 / storage.rs:191): that UPSERTs
  and increments the persisted counter, which would burn a rev on every subscribe.
  On a cold cache `peek_rev` returns `None`; emit the catch-up with the rev field
  omitted — the client force-refetches on a missing rev (store.ts:519), so it still
  converges.
- **Implementation site:** not inside `apply_events_frame` (server.rs:1489) — that
  function has neither the socket `sink` nor the `ChangesService` in scope.
  Refactor it to **return the set of newly-inserted fine topics**, and have the
  caller in `handle_events_socket` (server.rs:1448) emit the catch-up using the
  already-in-scope `sink` and `changes`, mirroring the lag-recovery block.

### Honest cost (corrected)
Because the client dedups with `>=` (not `>`), a same-rev catch-up triggers **one**
extra changes GET per (re)subscribe of the selected session. That GET is served
from the changes cache and applied idempotently via the response rev guard. Worst
case is therefore "one extra changes GET per reconnect," not zero. (If we ever want
strict no-op-on-same-rev, change store.ts:520 to `>` — out of scope here.)

### Files
- `crates/dux-web/src/server.rs` — refactor `apply_events_frame` to return
  newly-inserted fine topics; emit catch-up at the caller (≈1448) via `sink` +
  `changes.peek_rev`.
- No frontend change required (existing `session.changes` handler converges,
  including the revless cold-cache case).

### Tests
- **Gap scenario (the actual bug):** seed a session at rev N, subscribe, advance
  the changes cache to N+1, then drive the subscribe path and assert the catch-up
  carries N+1 (not N).
- **Cold cache:** no compute yet → subscribe → assert a revless catch-up is emitted;
  a frontend test asserts an event with `rev === undefined` calls `loadChanges`.
- Same code path covers first-connect and reconnect; name the test `(re)subscribe`.

### Risk
Low. One extra idempotent GET per reconnect; mechanism reuses a proven path.

---

## Item 2 — Split the WebSocket cap into three

### Problem
One semaphore (`ws_semaphore`, cap `max_websocket_connections`, default 128 —
config.rs:206/:152, server.rs:82/:314) covers all three socket types, so a flood of
terminal sockets can starve the control/events channel.

### Design
Three independent semaphores + three config fields. (A reviewer argued two caps —
events vs all-PTY — would suffice since agent and terminal PTYs share a resource
profile; the user explicitly chose three. **Keeping three; see "Decision" below.**)

| Class            | Config field                            | Default | Permit site (current) |
|------------------|-----------------------------------------|---------|-----------------------|
| control/events   | `max_websocket_events_connections`      | 32      | server.rs:1155 (inline) |
| agent PTYs       | `max_websocket_agent_connections`       | 32      | server.rs:767 (helper)  |
| terminal PTYs    | `max_websocket_terminal_connections`    | 64      | server.rs:834 (helper)  |

Defaults chosen by the user: agents and terminals are deliberately separate caps
(an agent cap and a many-terminals cap are different needs). Totals to 128, the same
as the old single default. Each field's inline comment must document the `=0`
semantic the old field carried: **"A value of 0 permanently blocks this connection
class until the server is restarted with a nonzero value."**

### Files (expanded by the review — the original list was incomplete)
- `crates/dux-core/src/config.rs` — remove `max_websocket_connections`; add the
  three fields with `#[serde(default)]` + per-field default consts + the `=0`
  comment. **Correct rationale:** old configs carrying the removed key still load
  because `ServerConfig` has no `#[serde(deny_unknown_fields)]`, so TOML (self-
  describing) silently ignores the now-unknown key — this is *not* a `serde(default)`
  effect.
- `crates/dux-core/src/config_write.rs` — strip the old key in the **incremental
  patch path that runs on every save** (mirror the oneshot strip at :491, which is
  in `patch_*`, not "on regenerate"); emit the three new commented keys in the
  canonical renderer.
- `crates/dux-web/src/server.rs` — replace `ws_semaphore` with three semaphores in
  `AppState`; init from the three config values (:314); **refactor the events
  handler at :1155** (currently an inline `try_acquire_owned`) to use
  `acquire_ws_permit` with the events semaphore; PTY handlers (:767/:834) draw from
  their own via the extended helper. Update the stale access-log comment at :429-432
  (it cites `/api/file/raw?session_id=…`) — folded here or in 5a, see note.
- `crates/dux-web/src/engine_actor.rs` — **`server_rebind_settings_changed` (:268)
  references the removed field (compile error).** Replace the single comparison with
  one per new field (OR'd); update the function's doc comment (:252-258) and the
  test `rebind_drift_detects_max_websocket_connections_change` (:1942) into three
  per-field assertions. (Without this, changing a cap via reload silently gives no
  "restart required" warning and the change is inert.)
- `crates/dux-tui/src/config.rs` — the TUI canonical renderer has its own
  `ConfigEntry::Field` for the old key (≈650-663) + a test assertion (≈1292);
  replace with three commented entries + three assertions.
- `crates/dux-tui/src/cli.rs` — `dux config diff` calls `diff_usize("server.max_
  websocket_connections", …)` (≈315-317); replace with three.
- `lib.rs` + `tests/auth_gate.rs` — field references (compile-error sites).
- One-time **migration warning:** on load, if the raw TOML still contains
  `max_websocket_connections`, log a warning naming the three replacements and
  noting `=0` meant "disable." (Unreleased branch → a warning is sufficient; do not
  auto-distribute the old value.)
- Docs: website/README if either documents the old single cap.

### Tests
- **Independence (not both-zero):** set the terminal semaphore to 0 and the events
  semaphore to 64; assert an events upgrade still succeeds (and the reverse).
- **Permit lifecycle:** acquire a permit from the terminal semaphore, assert the
  available count drops; drop it, assert it recovers (proves release-on-close).
- Three rebind-drift tests, one per new field.

### Settled
Three caps, deliberately (a reviewer suggested two; the user wants agents and
terminals capped independently). Defaults 32 / 32 / 64.

### Risk
Medium (config schema + several call sites). The expanded file list + tests above
are exactly the review's mitigations.

---

## Item 3 — Change-gated spine check + slow backstop

### Problem
Every 5th tick (≈250ms, engine_actor.rs:160/:169) the actor serializes the whole
projects+sessions spine to JSON and compares it to the previous string
(`spine_fingerprints`, **engine_actor.rs:1330** — not engine/mod.rs), even when
nothing changed.

### Corrected constraint (the review disproved the original claim)
Spine state is mutated at **four** loop-level sites, not two:
1. `apply_wire(cmd)` — engine_actor.rs:1345 (defined `Engine::apply_wire`, wire.rs:615).
2. `process_worker_event(event)` — engine_actor.rs:916.
3. `refresh_terminal_foregrounds()` — engine_actor.rs:1107 (mutates
   `terminal.foreground_cmd`, projected at viewmodel.rs:330).
4. `prune_exited_ptys()` — engine_actor.rs:1112 (removes providers, flips session
   status; projected into the sessions spine).
The streaming flag (`is_agent_streaming`, engine/mod.rs:605) is time-derived from
`pty_activity` and is **not** observable via a mutation counter, and it does **not**
cover prune (a quiet, non-streaming agent that exits).

### Design (in-memory only)
1. **Mutation version** (`u64` in the actor): bump after each of the **four** sites
   above — for #3/#4 only when they report a change (`prune_exited_ptys` already
   returns a `Vec`; bump when non-empty; `refresh_terminal_foregrounds` returns/sets
   a "changed" signal). This keeps idle-agent-exit detection prompt (≈250ms), not
   2s.
2. **Streaming-change counter** (O(1), replaces the per-tick sorted hash a reviewer
   flagged as O(N log K)+alloc): inside `poll_pty_activity`, keep each agent's
   previous `is_agent_streaming()` result; bump the counter on any transition. No
   per-tick allocation or sort.
3. **Gated check:** run `spine_fingerprints` (the serialize+compare, unchanged) only
   when the mutation version or the streaming counter moved since the last check.
   The fingerprint compare remains the precise emit gate (no spurious emits). On an
   idle system neither signal moves → no serialize.
4. **Slow backstop (kept — now clearly justified):** every ~40 ticks (~2s) run the
   fingerprint compare unconditionally. The review *proved* the loop-mutator set is
   easy to under-enumerate, so this is genuine **defense-in-depth** for any future
   loop mutator added without a bump — not a contradiction of step 1. Reframe the
   prose accordingly (the earlier "cannot miss" wording was wrong).

Persisted nowhere; resets on restart with no consequence (clients refetch on any
signal).

### Files
- `crates/dux-web/src/engine_actor.rs` — bumps at the four sites; streaming counter
  in `poll_pty_activity`; rewrite the spine-check block (≈1159-1178) to be
  signal-gated with the backstop. `spine_fingerprints` itself unchanged.

### Tests (with the seams the review showed are required)
- **Skip proof:** a `#[cfg(test)]` `AtomicU64` call-counter incremented inside
  `spine_fingerprints`; assert it stays 0 across many idle ticks (proves the
  serialize is actually skipped — "no event emitted" alone does not).
- **Backstop proof:** a `#[cfg(test)]` `EngineRequest::InjectSpineMutation` that
  mutates a session name **without** bumping the version; advance past the backstop
  interval; assert the change is emitted.
- **Streaming flip (deterministic, no sleep):** run the engine directly (not via the
  actor) and back-date `pty_activity` past `AGENT_STREAMING_WINDOW` (mirroring the
  existing hysteresis tests at engine/mod.rs:1639), assert the streaming counter
  moved.
- **Prune/refresh bumps:** a quiet agent exit and a foreground-command change each
  trigger a check within one spine-check interval.

### Risk
Medium (changes when events fire). The seams above make every property testable;
the backstop bounds any future completeness mistake.

---

## Item 4 — Proactive PTY viewer cleanup

### Problem
A closed viewer's `Sender` is pruned only on the next PTY output
(`subs.retain(|tx| tx.send(...).is_ok())`, pty.rs:577), because the web forwarder
detects disconnect via a 250ms `is_closed()` poll (server.rs:581/:596) and relies
on the next byte to prune. On a quiet PTY the registration lingers and inflates
item 2's terminal count.

### Design — RAII unsubscribe guard (async-safe)
- Give each subscriber a stable id: change the store from `Vec<Sender<Vec<u8>>>` to
  `Vec<(u64, Sender<Vec<u8>>)>` (pty.rs:427) with a monotonic counter.
- `subscribe()`/`subscribe_with_repaint()` return a guard holding the id; the guard
  lives in `handle_pty_socket` (async). **Async-safety (review finding):** the
  subscriber store is a `std::sync::Mutex`; taking it in the guard's `Drop` from a
  tokio worker can block the executor under a mass-disconnect at the higher terminal
  cap. Preferred: the guard pushes its id onto a lock-free "pending removals" list
  (or sets a per-entry `AtomicBool` "dead"); the reader loop drains it before each
  fan-out, keeping the `Vec` mutation on the reader thread. Acceptable fallback:
  direct `retain` under the std mutex with a comment that the lock is held only
  briefly (bounded by the fan-out) — fine at realistic counts. The reactive
  send-error `retain` stays as a backstop.

### Tests
- Subscribe, drop the guard, assert `rx.recv().is_err()` (Disconnected) with **no**
  intervening PTY output (proves proactive, not the reactive path).
- Two subscribers: drop one → its `rx` is Disconnected while the other's
  `try_recv()` is `Empty` (still held).

### Risk
Low. Backed by the retained reactive prune.

---

## Item 5a — Nest git/file routes under the session

### Design — nested paths, drop `session_id` from payloads, no aliases
Git (`git_routes.rs`, POST): `/api/v1/sessions/:id/git/{stage,unstage,discard,
commit,push,pull}` (bodies lose `session_id`: stage/unstage/discard `{path}`,
commit `{message}`, push/pull bodiless).
Files (`file_routes.rs`): `POST /api/v1/sessions/:id/files/{list,read,diff,write,
open-in-editor}` and `GET /api/v1/sessions/:id/files/raw?path=` (drop `session_id`
from all bodies/query).

Mirror `session_actions.rs:43-56` with `Path(id): Path<String>`, and — per the
review — **add the `id_within_bound(&id)` guard** at the top of every new handler
(today's git/file handlers lack it; the sibling nested routes in
`session_actions.rs`/`terminal_actions.rs` all have it).

### Files (expanded — the original list missed the test files)
- `crates/dux-web/src/git_routes.rs`, `file_routes.rs` — re-path, drop `session_id`,
  add `id_within_bound`.
- `crates/dux-web/src/server.rs` — registration unchanged in shape; update the
  access-log comment at :429-432 (cites the old `/api/file/raw?session_id=…`).
- Frontend: `web/src/lib/git.ts` (6), `fileApi.ts` (5), `markdown.ts:53` (raw URL).
- **Frontend tests (missed before):** `web/src/lib/gitFileApi.test.ts` (asserts the
  literal `/api/v1/git/*` and `/api/v1/file/*` URLs + bodies incl. `session_id`) and
  `web/src/lib/markdown.test.ts:83` (asserts the exact raw URL) — both must move to
  the nested forms.

### Tests
- **Positive extraction:** POST to a nested route with a known seeded session id;
  assert a non-routing outcome (proves `:id` is extracted), not just the 404 case.
- **Negative:** unknown id → 404; oversized id → 404 via `id_within_bound`.

### Risk
Low/medium (broad but mechanical). Tests + `tsc` catch breakage.

---

## Item 5b — Tolerate a detached HEAD (much larger than first scoped)

### Problem
`current_branch()` runs `symbolic-ref --quiet --short HEAD` (git.rs:58), which exits
**1** on a detached HEAD and **128** on a real failure (not a repo / git missing).
The review found **seven** call sites that choke on detached HEAD, across both the
TUI and the web — the original plan named only three, and the web create-agent path
does not even use the path it cited.

### Design
- **`current_branch_opt(repo) -> Result<Option<String>>`** in git.rs:
  `Ok(None)` **iff `output.status.code() == Some(1)`** (detached; `--quiet` silences
  stderr); `Err` on any other non-zero (128 etc.) or spawn failure. Apply the same
  `.trim()` current_branch uses (else `"main\n" != "main"` breaks warnings). Tests:
  detached → `Ok(None)`; non-repo → `Err`.
- **Change `leading_branch_for_project` signature** to
  `(path, current_branch: Option<&str>)` (project_browser.rs:71); fallback order:
  `origin/HEAD` (remote default) → `current_branch` if `Some` → else cannot
  determine (error).
- **Apply `_opt` + tolerant handling at all inspection callers:**
  - Web inspect `GET /api/v1/projects/inspect` (project_reads.rs:172): 200 with
    `current_branch: null` on detached; compute the warning only when a branch is
    present. (Keep the non-repo `Err` → 400 so `inspect_non_repo_reports_error`
    still holds.)
  - `load_projects` (project_browser.rs:108): use `_opt`; detached → store
    `current_branch = None`/empty, **not** a faked `"main"`.
  - Branch-status job (project_browser.rs:197) and checkout-default inspection
    (:224): tolerate `None`.
  - **Web add-project** paths `wire.rs:1238` (`add_project_checkout_default`) and
    `wire.rs:2080` (`AddProject`): use `_opt`, fall through to
    `leading_branch_for_project(None)`.
  - TUI `add_project` (sessions.rs:57) and TUI create-agent pre-check
    (workers.rs:1389): use `_opt`.
  - `git::switch_branch_if_needed` (git.rs:231) — called by `agent_job.rs:64`
    (pull-before-create) and `engine/command.rs:745` (project pull): use `_opt`; on
    `None` (detached) skip the equality check and switch unconditionally.
- **Web create-agent base selection** (agent_job.rs:60-63, 95-98): the
  `leading_branch` fallback currently uses `project.current_branch` (which becomes
  `None`-derived after the load_projects fix). Route the fallback through
  `leading_branch_for_project(repo, project.current_branch.as_deref())` so a
  detached web project branches off `origin/HEAD`, not a faked `"main"`; if no base
  is determinable, fail with a clear `CreateAgentFailed` message.

### Tests
- Detached inspect → 200 `current_branch: null`; **non-repo inspect → still 400**.
- TUI add_project on a detached repo → success using the derived leading branch.
- TUI create-agent, detached + stored leading → success off the leading branch.
- Web create-agent, detached + leading `None` + `origin/HEAD` present → builds off
  the default; detached + no `origin/HEAD` → clear error.
- `switch_branch_if_needed` on detached → switches (no spurious failure).

### Risk
Medium (it grew). Base selection is unchanged; the spread is across inspection
callers, each covered above.

---

## Suggested landing order
1, 5b, 4, 2, 3, 5a (each its own commit; 5a/5b are the largest).

## Punted low-severity items (documented, not blocking)
- **X-Connection-Id is client-supplied** (rest_common.rs): a forged value scopes a
  mutation's toasts to a nonexistent connection, silencing them — pre-existing, not
  introduced here, and harmless under the single-tenant trusted model. Document only.
- File-ref corrections from the review are already folded into this revision
  (spine_fingerprints → engine_actor.rs:1330; apply_wire → wire.rs:615;
  add_interest → event_bus.rs:96; rev read → peek_rev changes.rs:403).
- `peek_rev` cold-cache `None`: **not a gap** — the client force-refetches on a
  missing rev (store.ts:519). Noted in item 1.

## Settled decisions
- **Item 2: three caps** (separate agent vs terminal), deliberate. Defaults
  32 / 32 / 64 (totals 128, same as the old single default).
- **Item 1: fine-topic-only catch-up**, coarse left to the existing `onOpen` refetch.

## Item 6 — Connection liveness + notification-tag validation (new)

Two requests from the user, solved by one shared piece: a **per-class registry of
live connections** (connection id → {class, last-active, close handle}), maintained
on connect/disconnect. This single structure backs both features below.

### 6a. Don't let a forged/stale notification tag silence feedback
Today a REST mutation carries a client-supplied connection id that scopes its status
toasts; it is never validated, so a forged or stale id scopes the toasts to a
nonexistent connection and they appear nowhere (the action still runs). The
connection id the server mints per events socket is an unguessable UUID, so another
user cannot easily target *someone else's* toasts — the realistic abuse is silencing
*your own* feedback (or a troll forging a random id over the network).
**Fix:** on each mutation, validate the supplied id against the live-connection
registry; if it does not match a live connection, fall back to broadcasting to **all**
clients (the existing safe default). Worst case becomes "toast shows everywhere"
(mildly noisy) instead of "toast shows nowhere." Cheap, and it reuses the registry.

### 6b. Eviction when a cap is reached (approach pending — see chat)
Goal: a new connection should not be rejected just because the class is full. The
honest tradeoff (auto-reconnecting clients can thrash if a live connection is
evicted) and the recommended design are under discussion; once chosen it will be
specified here. Candidate pieces: liveness ping to reclaim dead/zombie slots
automatically; optional least-recently-active eviction paired with a "displaced, do
not auto-reconnect" close signal to prevent thrash.
