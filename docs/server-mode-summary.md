# dux `server-mode` Branch — Technical Summary

> Canonical record of the `server-mode` branch. Audience: a developer who will
> maintain this branch. Every claim here is grounded in the branch commit log,
> the design specs under `docs/superpowers/specs/`, the implementation plans
> under `docs/superpowers/plans/`, the project tenets in `CLAUDE.md`, and the
> actual code under `crates/`. Where a feature was specified but its shipping
> could not be confirmed, it is called out explicitly (see the final section).

**Branch size:** 376 commits, ~254 files changed, ~+82.8k lines.

---

## 1. Overview

`server-mode` gives dux a second way to be used: in addition to the terminal UI
(TUI), the same orchestrator can run headless and serve its entire interface as
a polished web application. One dux process is the single owner of config,
SQLite, git, providers, and every PTY; a browser (desktop or phone) reaches it
over a single WebSocket. Each agent's pseudo-terminal streams to the browser at
1:1 fidelity via xterm.js, and the changed-files pane, diffs, command palette,
and status reporting are re-rendered as native web components rather than a copy
of the terminal cell grid. Server mode is entered two ways: `dux server` (boots
headless, the TUI is never created) or an in-process "flip" from a running TUI
(a palette action turns the live process into a server and shows a status screen
until the operator returns).

What made this possible is a large architectural transformation that landed
first: the previously monolithic TUI binary was carved into a Cargo workspace
with a headless engine crate (`dux-core`) at its center. All domain logic —
state, the command set, the worker/event model, PTY handling, config, git, and a
serializable view projection — moved into `dux-core`, which has zero
`ratatui`/`crossterm`/`axum` dependencies and compiles and tests headless. The
TUI (`dux-tui`) and the web server (`dux-web`) became thin surfaces that each
drive their own loop around the same engine. Because all capability lives in the
engine behind one dispatch boundary (`Engine::apply(Command)`), neither surface
can hold a capability the other lacks — parity is enforced *structurally*, not
by discipline.

The branch then built the web surface end-to-end in a security-gated build
order — web-facing core APIs, server skeleton + transport, the React web UI,
mobile/PWA, the lifecycle flip + CLI, auth, and TLS/ACME — followed by a long
arc of parity, polish, and reliability hardening (the config write queue, the
config-writer/PTY shutdown fixes, the full-worktree Monaco editor, and a unified
keyed status model that renders as a TUI status line and web toasts). Both
surfaces remain first-class; the longer-term bet is that server mode may
eventually become the default, so the architecture is built so the TUI can be
dropped later by deleting a crate, without untangling.

---

## 2. The architectural transformation (sub-project #2)

The first and largest body of work was a **behavior-preserving** refactor of a
TUI the author uses daily: extract the monolith into a workspace with a headless
engine. The discipline throughout was characterization-tests-first, incremental
slices, and the TUI green at every step.

### 2.1 The workspace topology

```
Cargo.toml                 # workspace manifest; resolver = "3"; one workspace version
crates/
  dux-core/                # headless engine. NO ratatui/crossterm/axum.
  dux-tui/                 # ratatui + crossterm terminal surface. depends on dux-core.
  dux-web/                 # axum + tokio web server surface. depends on dux-core.
  dux/                     # thin binary entrypoint. depends on dux-tui (+ dux-web).
crates/dux-web/web/        # React + Vite + Tailwind v4 + shadcn SPA, embedded into dux-web.
```

Four dependency invariants, validated by `cargo tree` at the close of the
extraction: `dux-core` has zero TUI deps; `dux-web` depends on `dux-core` only;
`dux-tui` depends on `dux-core`; the `dux` binary depends on `dux-tui` (and, for
the flip, on `dux-web` — it is the only crate that depends on both surfaces).
`[workspace.dependencies]` is the single source of truth for shared crate
versions, and the workspace `version` is shared so the TUI and the web can never
report different versions.

### 2.2 The phased extraction (Phases A–E)

A dependency scan found the extraction was gated on a `config ↔ keybindings ↔
theme` knot, so it was phased:

- **Phase A** — stood up the workspace and `dux-core`; moved five clean leaf
  modules (`model`, `io_retry`, `browser`, `editor`, `statusline`).
- **Phase B** — adopted `[workspace.dependencies]`; moved theme *identity*
  (`DEFAULT_THEME_NAME`) to core. The ratatui `Theme`/`Style` helpers and value
  loaders stay in the TUI permanently (rendering-specific).
- **Phase C** — C1 moved the `Action` enum (the shared command vocabulary) to
  `dux_core::action`; C2 moved the config **data layer** (`DuxPaths`, root
  resolution, env/path helpers) to `dux_core::config`. The documented-template
  renderer and `toml_edit` patch machinery stayed in the TUI at first.
- **Phase D** — D1 cascaded the domain cluster (`logger`, `git`, `storage`,
  `provider`, `startup`, `lockfile`) into core; D2 decoupled `pty` from ratatui
  via core's own `CellColor`/`CellModifier` (converted 1:1 to ratatui at the
  single TUI paint site in `tui_color.rs`) and moved `pty` into core.
- **Phase E** — the Engine extraction itself, decomposed into E1–E5:
  - **E1** moved `Config`/`KeysConfig` (with an empty core `KeysConfig::default`,
    runtime-equivalent since `RuntimeBindings`/`detect_conflicts` fall back to
    `BINDING_DEFS`) and `WorkerEvent` + its domain payload types into core.
  - **E2** introduced `dux_core::Engine` as the domain-state container; ~31
    domain fields moved out of `App` into `Engine`; `App` now embeds `engine:
    Engine` and keeps view/input fields. The ~410-site access-path rewrite was
    mechanical and preserved borrow structure.
  - **E3** moved domain operations and background workers into `Engine` methods
    (decomposed into many sub-batches T1–T3f), introducing the
    `EventReaction` pattern (below) and relocating helper clusters into new core
    modules (`project_browser`, `gh`, `resource_stats`, `agent_job`).
  - **E4** introduced the `Command` enum and `Engine::apply(Command) ->
    Result<EventReaction>` dispatch (deletion family, project persistence,
    agent-creation dispatch, git ops, misc). E4f (rewiring `input.rs` to call
    `engine.apply` inline) was **cancelled** after an adversarial scope review
    found it was code-shuffling with no architectural gain — `engine.apply` was
    already the single `pub` dispatch point and the web layer already calls it
    directly; the App wrappers stayed because they factor input validation and
    bindings-aware message formatting into testable units.
  - **E5** carved the final topology: `dux-tui` became a library crate holding
    the whole TUI surface, `dux-web` became a stub depending only on `dux-core`,
    and `crates/dux` became a thin binary entrypoint.

### 2.3 The Engine / Command / WorkerEvent / EventReaction model

The engine is the single owner; surfaces are thin. The model has four moving
parts:

- **`Command`** — the union of everything either surface can ask for (create
  agent, fork, commit, push, pull, stage/unstage, delete, persist project,
  etc.). `Engine::apply(Command) -> Result<EventReaction>` is the **one**
  dispatch boundary. A web button or TUI key with no backing `Command` is the
  smell to reject.
- **`WorkerEvent`** — the result type that background workers send back over an
  mpsc channel. All potentially-blocking work (git, file I/O, PTY spawn, network)
  runs on workers, never on a surface's loop.
- **`EventReaction`** — a typed enum returned by the engine's pure event/command
  processing. The engine mutates **only engine state** and returns an
  `EventReaction`; the surface consumes it for view updates (the TUI via
  `apply_reaction`; the web via a `wire_statuses_from_reaction` translation).
  This keeps the engine free of any view-state writes — status updates flow back
  as `EventReaction::Status(...)`, not `set_*` calls.
- **Spawn-worker primitive** — a later hardening pass (the
  engine-spawn-worker spec) collapsed four heterogeneous in-flight tracking
  fields into one `HashSet<InFlightKey>` keyed by a typed enum, and routed every
  `thread::spawn` site (~17) through `spawn_command_worker` /
  `spawn_background_worker` / `spawn_loop_worker`. These mark the right
  in-flight key synchronously, clear it whether the worker finishes, errors, or
  **panics** (`catch_unwind`), and deliver the "starting" status through the same
  FIFO channel as the completion event so a busy status cannot be overwritten
  out of order.

"**No protocol layer**" is unchanged and load-bearing: providers still run as
plain CLIs in a real PTY (`portable-pty`), with no JSON-RPC, ACP, or adapter
binaries. The web path bypasses the alacritty emulator (xterm.js is its own
emulator on the client) but the engine still keeps an authoritative alacritty
grid per session, which is what makes the on-connect repaint possible.

The **`StatusLine`** controller (`dux_core::statusline`) is the single shared
engine-status surface, behaving 1:1 across the TUI and the web. The TUI holds one
in-process; the web actor holds one and broadcasts its state. Both follow the
tone-aware auto-clear policy (Busy persists until a final state; Info auto-clears;
Warning/Error persist). This model was later generalized into a keyed-multi
controller (see §4 and §3.10).

---

## 3. Server mode

The web surface was built in a security-gated build order. Each workstream below
records what it does, why, and the load-bearing design decisions.

### 3.1 Web-facing core APIs

The first slice added the headless contract the server would later expose, with
no server or UI yet (exercised by tests against the headless engine):

- **`ViewModel`** (`dux_core::viewmodel`) — a serializable projection of
  everything a client needs to draw: projects and their state; agent sessions
  with status; changed files (staged/unstaged) and structured diffs; status-line
  text/tone; available palette actions; active prompts. Navigation, selection,
  and focus are intentionally **client-side** (independent navigation, Linear-
  style), so the ViewModel excludes the TUI cursor/focus state.
- **Wire-command intake** (`dux_core::wire`) — reconstructs and dispatches engine
  `Command`s from a generic, transport-agnostic message.
- **PTY byte fan-out + on-connect repaint** — bytes still feed alacritty (the
  authoritative grid) **and** a per-session broadcast for web clients. A joining
  client gets the current screen via a synthesized ANSI repaint from the existing
  `TerminalSnapshot` (set alt-screen mode if active, clear, position cursor, emit
  styled cells, restore cursor), then live bytes.

### 3.2 Server skeleton & transport

`dux-web` runs an **axum + tokio** server. Because `Engine` is `!Send`, it runs
as an **actor on its own thread** (`engine_actor.rs`): an `EngineRequest` mpsc
channel with oneshot replies, a `watch` channel carrying the ViewModel, and
broadcast streams for status and commit-message output, all behind an
`EngineHandle`. ViewModel pushes are deduped by projection equality so clients
only see real changes. A single WebSocket carries ViewModel deltas, PTY bytes
(binary), and Command intake. The first `dux server` subcommand served
`127.0.0.1:8080`; a smoke-test page validated the transport before the React app
existed. Nagle is disabled on accepted connections so remote typing is not
batched.

### 3.3 Web UI

The frontend (`crates/dux-web/web/`) is a **React 19 + Vite + Tailwind v4 +
shadcn/base-ui** SPA, built and embedded into the binary (gzip-compressed assets,
SPA fallback, auto-rebuild via `build.rs` when sources change). It mirrors the
TUI's three-pane mental model: a left sidebar of projects/sessions, a center
terminal (xterm.js over the PTY WS) or diff/editor surface, and a right
changed-files pane. It was shipped, then **redesigned the same day** onto the
shadcn sidebar-dashboard block with Empty states and sonner toasts.

The web UI follows its own conventions (codified in `CLAUDE.md`), distinct from
the TUI's `theme.rs` engine:

- **Dark-only.** `main.tsx` force-adds the `.dark` class and sets
  `colorScheme = 'dark'`; the light tokens exist but are inert. Style through the
  shadcn/base-ui token CSS variables, never hardcoded colors.
- **shadcn/base-ui first.** Reuse an existing `components/ui/*` primitive before
  hand-rolling. Hover hints go through the shared `SimpleTooltip`, never native
  `title=`.
- **Row actions collapse into one `⋯` menu.** A list row needing more than one
  action puts them behind a single `DropdownMenu` that reveals on hover, consumes
  no idle layout space, stays revealed while the menu is open (keyed off Base
  UI's `data-popup-open`, not `aria-expanded`), and always shows on touch.
- **One git-status marker** via the shared `FileStatusIcon`, interpreted once by
  the pure, unit-tested `fileStatusMeta(status)` in `lib/changedFiles.ts`.
- **Destructive actions always confirm** via a dedicated `Dialog` (Cancel
  `autoFocus`, confirm `variant="destructive"`, misclick-safe spacing). The
  menu entry itself is neutral with a trailing `…`; red is reserved for the
  confirm button.
- **Touch targets ≥44px** on phones (`max-md:min-h-11`/`size-11`), desktop
  density restored with `md:`.

Key frontend pieces present in the tree: `App.tsx`, a Zustand-style `store.ts`,
`ws.ts` transport, `CommandPalette.tsx` (cmdk), `Sidebar.tsx`, `TerminalPane.tsx`
(xterm + fit addon), `ChangedFiles.tsx`, `DiffViewer.tsx`, `EditorOverlay.tsx` +
`CodeEditor.tsx` (Monaco), `MobileShell.tsx`, `AccessoryBar.tsx`, `LoginScreen.tsx`,
plus a large set of dialogs and pure `lib/*` modules with colocated vitest tests.

### 3.4 Web↔TUI parity infrastructure & the lifecycle flip

A dedicated "parity root-cause infra" effort, plus two later parity-gap audits
(a DO-NOW batch and a LATER batch), brought the web to **effective capability
parity** with the TUI across all three panes: create/fork/rename/reconnect/
delete agents; provider swap; worktree adoption; new-agent-from-PR;
project add/remove with a directory browser and non-default-branch pre-flight;
project pull and checkout-default-branch (run through the two-worker chain so a
synchronous `git switch` never blocks the engine loop); stage/unstage/discard;
commit + AI commit message; push/pull; changed-files search; diff line-number
toggle; sort-by commands; persisted drag-and-drop ordering. Shared logic was
pushed into `dux-core` where it had been duplicated (sidebar grouping in
`dux_core::sidebar`, welcome tips in `dux_core::welcome`, the PR parser/lookup,
branch-warning helpers).

The **in-process flip** (`serve_with_engine`) turns the *main thread* into the
engine-actor thread — the binary orchestrates the cycle, axum runs on an internal
runtime, and PTYs keep running across the flip because the engine owns them, not
the surface. The single-instance lock rides inside the engine. Hardening here was
real: a reviewer empirically proved (gdb) that a runtime drop waited forever on
`spawn_blocking` PTY forwarders parked in `recv()` while the senders stayed alive,
fixed via `recv_timeout` + a shutdown flag + a bounded runtime shutdown; tokio
signal residue that left the resumed TUI unkillable was fixed by restoring
`SIG_DFL` via libc; the SIGWINCH handler is unregistered per flip. A themed
`ServerStatusScreen` (existing theme tokens only, wall-clock redraws) shows while
serving (q/Esc returns to the TUI, Ctrl-C quits).

### 3.5 Auth / login gate

Auth is **single-tenant / trusted-access by design** (see §5). v1 is local
bcrypt credentials, multi-user, stored htpasswd-style as `users = ["name:hash"]`
under `[auth]` in `config.toml`, managed from the trusted side only (TUI palette
`server-add-user` / `server-remove-user`, or by editing config). Web-side user
management was **deliberately excluded** — a remote session minting users is
privilege-escalation surface.

- **Session layer:** `tower-sessions` (MemoryStore) was used directly;
  `axum-login` was evaluated and declined (one backend, three routes). 128-bit
  CSPRNG session ids; `cycle_id` before privilege insert (anti-fixation);
  HttpOnly + SameSite=Strict cookies (Secure deferred to the TLS step); anonymous
  traffic creates zero session state.
- **The gate** covers all routes **and** the WebSocket upgrade, plus an Origin
  check (same-host) as cross-site-WS-hijack defense. AuthState is parsed once at
  startup and refreshed live on config reload; user removal revokes live sessions
  (the gate and `/api/me` re-verify the session's username per request, and an
  already-upgraded socket is rechecked periodically and closed if the user
  vanishes). bcrypt verify runs on `spawn_blocking` so N IPs cannot pin all tokio
  workers; a per-IP rate limiter (swept, capped at 4096 entries) bounds brute
  force.
- **The exposure gate re-keys:** non-loopback binds are allowed when auth is on;
  `--disable-auth` alone can never open a public bind (it still needs the
  insecure opt-in). `--disable-auth` is a deliberate CLI-only delegation to an
  upstream auth proxy. The web frontend has a boot `GET /api/me` probe driving an
  auth state machine (checking | disabled | anonymous | authed | unreachable),
  a shadcn `LoginScreen`, a logout palette entry (web-surfaced only), and an
  "unreachable — retrying" screen with capped backoff. A final three-seat council
  (security/quality/architecture) drove a fix round (config.toml now written
  0600 since it carries hashes; a live non-loopback server refuses a reload that
  would drop its last user).

### 3.6 TLS / ACME

`dux server`'s secure preset uses **`rustls-acme` (HTTP-01)** via `axum-server`:
`:443` serves TLS; `:80` answers `/.well-known/acme-challenge/…` and otherwise
redirects to HTTPS; both ports configurable. `rustls-acme` is runtime-agnostic
with no background tasks, so the `AcmeState` stream is polled on a dedicated task
and every lifecycle event is logged loudly (explicit-failure tenet) and streamed
to clients as keyed status. A `production` toggle selects Let's Encrypt
staging vs production. "Behind my own proxy" disables built-in ACME and serves
plain HTTP with TLS terminated upstream. The config was reshaped (user-driven)
into **LOCAL MODE** (`port`, always loopback plus optionally the machine's
Tailscale IP — used by the flip and by `dux server` with no `listen_addrs`) and
**FULL WEB MODE** (`listen_addrs`, a list of `IP:port` sockets each classified
local-or-public independently); the old `bind` key is deprecated and migrated on
load. Tailscale is opt-out (`tailscale_enabled = true` by default; detected via
the `tailscale ip` CLI in `dux_core::tailscale`; not found → warn and serve
loopback only, never block). HSTS is sent; a no-gate (no-auth) TLS server warns;
an end-to-end TLS test suite serves over a self-signed cert. Deferred auth items
(Secure cookie flag, host allowlist = the ACME domains, expired-session sweep)
were folded in here.

### 3.7 Mobile PWA

Mobile is a first-class target, designed in from the start. `App.tsx` branches on
`useIsMobile()` (768px) to a `MobileShell` hub-and-spoke navigation state machine
(home → full-screen terminal → changes) over the existing store, with History-API
hardware-back integration (entering a spoke pushes a history state; `popstate`
navigates back in-app rather than exiting). The **accessory bar** synthesizes raw
byte sequences straight to the PTY (Esc/Tab, sticky Ctrl/Alt toggles, arrows that
honor the terminal's cursor-keys mode) so it does not depend on inconsistent
soft-keyboard key events; tapping it does not steal focus (pointerdown +
preventDefault keeps the keyboard open). `window.visualViewport` keeps the
terminal and accessory bar above the soft keyboard and re-fits + resizes the PTY.
The **PWA** is a manifest + an offline-fallback-only service worker (no app-shell
caching, so no stale-bundle risk), registered only when
`'serviceWorker' in navigator && window.isSecureContext` — dormant on plain-HTTP
LAN, automatic under HTTPS/localhost. `termkeys` byte synthesis is unit-tested,
and `vitest` was introduced here as a web test gate.

### 3.8 Scrollback replay; changed-files + commit-message generation; palette + macros; diffs; config + terminals

- **Scrollback replay.** A web client opening or reconnecting to a session sees
  the **full scrollback history** the TUI shows, not just the visible screen. A
  new `TerminalState::reconnect_repaint` rebuilds the client's primary buffer by
  printing the entire alacritty grid as a newline-separated, SGR-tracked line
  stream (the only way to push history into a terminal's scrollback over a byte
  stream), idempotent via `\x1b[3J`; the alt-screen branch is unchanged. The
  ViewModel carries `agent_scrollback_lines` so the web xterm is sized to match
  the configured depth (default 10000) instead of xterm's silent 1000-line
  default.
- **Changed files + commit-message generation.** A per-engine changed-files
  watch drives the web pane; stale poll events for an unwatched worktree are
  dropped; generated commit messages are scoped to their originating session; the
  changed-files git work and the staged-diff read for commit-message generation
  were moved **off** the single engine actor thread to avoid freezing all tabs.
- **Command palette + macros.** The palette is a surface-aware registry shared by
  the TUI and web (`dux_core::palette`), with a web-surface pin enforced across
  the language boundary. Macros are exposed over the wire (run them and edit them
  from the web), with the byte transform and surface predicate relocated into
  `dux_core::macros`.
- **Diffs.** A headless serializable diff engine in `dux_core::diff` (the same
  `similar` + `syntect` computation, hardened to reject symlink escapes and
  detect binary like the TUI) serves per-file diffs over the WS; the web renders
  them with syntax-highlighted lines, line wrapping, and a line-number toggle.
- **Config + terminals.** The web bootstrap loads the user's real `config.toml`;
  the `toml_edit` config writer moved into `dux_core::config_write` so both
  surfaces persist identically; companion terminals are fully supported over the
  web (create, view, stream, close); subscribing launches/resumes the real
  provider; GitHub PR status and details are surfaced.

### 3.9 The full-worktree editor

The web Monaco editor began scoped to changed files (read/write a working copy,
validated as a git-changed file, binary rejected, symlinks refused, `.git`
rejected, IntelliSense workers dropped to shrink the binary). It was then
expanded (the `editor-full-worktree` work) to browse and edit the **whole**
worktree:

- **Lists everything** via a filesystem walk, including gitignored files and
  files in wholly-ignored dirs, and `.git/` itself (with `.git/objects` and
  `.git/logs` excluded for performance). `.git/*` is **readable but
  write-refused** — both `.git` guards stay on the write path, loosened on read —
  so "see everything" holds without letting an edit corrupt or detach the
  worktree.
- **Read-permissive symlinks:** a symlink whose target resolves outside the
  worktree shows its content (read) but is marked `read_only` so the UI greys out
  Save; writes to out-of-worktree targets are refused. The markdown-preview image
  proxy follows symlinks the same way as the file read path, with worktree
  containment enforced against intermediate symlinks.
- **Performance is cap + virtualize, not lazy-tree:** a server-side ~50k-entry
  cap with a truncation indicator, a node cap inside `buildFileTree` with a
  banner, and row-virtualization in `FileTree.tsx`. The flat-list data model and
  the protocol are unchanged.

### 3.10 Keyed status + web toasts

The status model was generalized into a **`KeyedStatusController`** in
`dux_core::statusline`: a map of `key → entry` with a monotonic generation token
per key (so a stale success from a prior op cannot dismiss a newer error on the
same key), plus an anonymous slot for unkeyed transient statuses, and a busy
timeout that expires a leaked Busy to a Warning. Each surface renders the same
keyed source differently:

- **TUI = a most-recent-wins single status line** that honors keyed clears
  (documented limitation: lossy for concurrent errors — the web toast stack is
  where all concurrent statuses are visible).
- **Web = one `sonner` toast per active keyed entry**, using the key as the
  sonner id (re-emit updates in place; clear dismisses; success auto-clears ~6s,
  warning/error persist). An explicit `StatusCleared { key }` server message
  (not empty-message overloading) and a full-snapshot replay on connect were
  required additions. Every Busy→final producer pair was keyed (pull, reconnect,
  launch, async delete, PR lookup, checkout-default, commit-message, config
  writes, **ACME** lifecycle), and the local web success toasts that the engine
  now covers were removed to avoid double-firing.

This is the one place the branch **reversed a tenet**: the prior "do NOT
duplicate engine status onto a second surface (no web toast)" rule was updated.
The new tenet: the core keyed status controller is the single source; the TUI
renders it as a status line, the web renders it as toasts; correlation by
per-operation key. The toast `Toaster` is positioned bottom-center with a
mobile-safe offset clearing the footer and safe-area, and the StatusBar's
connection indicator is kept.

A standalone dev console (a vite-style `dux server` console, `console.rs`) and
server `color`/`access_log` config settings were also added.

---

## 4. Reliability & data-safety subsystems

### 4.1 The config save queue (`ConfigWriteQueue`)

Config saves used to happen in many scattered places, each writing directly,
which (on the web) ran on the one shared engine thread and briefly froze all
tabs, had no write ordering, and were not atomic (truncate-then-write could leave
a half/empty file; first-run creation had no owner-only permission). The fix
funnels **every** write through one shared ordered saver per process:

- **Atomic primitive:** every path (queued and synchronous-direct) writes via a
  temp file in the config directory (`tempfile::NamedTempFile`, mode 0600,
  self-deleting on drop), fsync for eager writes, then atomic `rename` over the
  real path. This replaced the old truncate-then-write and the raw `fs::write`
  sites in first-creation, deprecation-migration, regenerate, and recover (none
  of which had 0600 or atomicity).
- **Eager vs lazy:** an eager write is awaited inline (dedicated oneshot reply,
  ~2s timeout) and rolled back on failure with a persistent error — *except*
  per-site preserved exceptions (macros and `toggle-github-integration`
  keep-and-report rather than roll back, because their side effects can't be
  unwound). Eager sites: theme, agent provider, auth users, `[env]`, macros,
  project add/remove/update. Lazy writes mutate in-memory, enqueue, and coalesce
  a burst to one write of the latest state on a fixed (non-resettable) ~250ms
  deadline; lazy sites: pane sizes, changes-pane visibility, PR-banner position,
  diff line-numbers.
- **Quiesce barrier:** an exclusive config op (reload, recover) sets a
  "defer config-mutating commands" flag on the engine thread *before* spawning
  its worker, then the worker calls `queue.quiesce()` (drain pending + finish
  in-flight + pause the writer), performs its own write synchronous-direct, and
  the writer resumes on completion; deferred commands then run against the
  authoritative post-op state. This is what prevents a concurrent whole-settings
  write (e.g. a second web tab mid-reload) from clobbering an externally-edited
  config.
- **Lock-ordering fix:** the web bootstrap now takes the single-instance lock
  *before* reading config or opening the session DB (matching the TUI).
- **Locked down:** the raw write functions are `#[deprecated]` so any unrouted
  caller fails the `-D warnings` build; the few blessed synchronous-direct paths
  carry an explicit `#[allow(deprecated)]`. A `ConfigWritePermit` token was
  rejected because it cannot work across crates.

### 4.2 Config-writer / PTY shutdown hardening

A focused hardening pass (the most recent commits on the branch) closed a class
of teardown and ordering faults:

- The config writer is stopped with an **explicit shutdown message** so dropping
  the queue while a reload barrier is open no longer deadlocks; the lazy-inflight
  counter is wrap-safe and cap-exact; the writer logs faults, drains on shutdown,
  and resets its lazy counter if it panics so its cap gate cannot latch shut; a
  direct write is refused if the writer never acknowledged the pause; the writer-
  shutdown join is bounded; pending lazy writes are flushed when the queue is
  dropped so a clean exit does not lose a lazy write.
- The PTY writer thread is stopped with an **explicit shutdown signal** so a
  surviving sender clone cannot wedge teardown; a **blocking PTY write** no longer
  freezes the entire web server; the PTY child's whole process group is killed on
  teardown so `Drop` cannot hang.
- The in-flight **login-user update is carried through the surface config swap on
  reload**, and a login-user write is **deferred during a config reload** and
  replayed when the barrier closes (with a fault-injection harness for the login
  refund/rollback ordering, and a hardened login throttle, signals, and rollback
  gate).
- Background worktree-remove, commit-message, and session spawns are **panic-safe**
  so a panicked job cannot wedge in-flight state.
- **Logging is initialized in server mode** so config-writer and other faults are
  recorded in `dux.log`.

---

## 5. Design tenets & constraints

From `CLAUDE.md`, the principles that govern this branch:

- **All settings are configurable; the config file is the documentation.** Every
  server setting (`port`/`listen_addrs`, `tailscale_enabled`, ACME ports/on-off/
  domains/email, auth users) lives in `config.toml` with inline comments and
  concrete first-boot defaults. The canonical renderer produces a fully commented
  config on first creation; subsequent saves preserve user edits via `toml_edit`.
- **Config vs SQLite split.** Project config (`[[projects]]`) stores portable
  desired state (id, env-expanded path, name, default provider, auto-reopen,
  startup command). Derived state (current/leading branch, branch status, agent
  worktree paths/branches, provider process state, session/project sort order)
  lives in SQLite/runtime only. Config wins for explicit user preferences on
  startup; SQLite may fill missing fields.
- **New UI uses the theme engine** (TUI: `theme.rs`; web: shadcn/base-ui token
  CSS variables, dark-only). No hardcoded visual values.
- **Navigation:** Tab/Shift-Tab between panes; panes have local key combos and
  interactive vs non-interactive modes; Space activates the focused dialog button.
- **Single-tenant / trusted-access security model.** The login gate authenticates
  *that* a connection is allowed, not *which* subset of the workspace it may
  touch. Every authenticated client shares one workspace — it can drive any PTY,
  browse the server filesystem, and see every session. This is intentional for a
  per-developer or trusted-team tool; there is deliberately **no per-user
  ownership or path sandboxing**. The read-permissive editor symlink behavior is
  an accepted widening under this model.
- **macOS + Linux only.** Windows runs via WSL2 (Linux). No `#[cfg(windows)]`
  branches; assume Unix throughout.
- **Git command safety.** Prefer plumbing over porcelain for machine-readable
  output; `--porcelain=v1 -z` for status, `--numstat -z` for diff stats; override
  config with `-c` where needed; rely on exit status for imperative commands;
  compute diffs in-process with `similar` + `syntect` rather than shelling out to
  `git diff`. Source-checkout refresh is `--ff-only`.
- **Worktrees are user data;** never removed or mutated casually; deletion requires
  explicit confirmation. Project removal cascades to its agents while keeping
  worktrees.
- **Long-running work never blocks the UI thread;** every async op keeps the
  status line updated for its full lifecycle (Busy → final). Prefer explicit
  failure over silent waiting. Commit messages are plain sentences (no
  conventional-commit prefixes, no structured trailers).

---

## 6. Testing & quality

- **Workspace gates (CI):** `cargo fmt`, `cargo clippy --all-targets
  --all-features -- -D warnings` (a hard CI gate — any warning fails the PR), and
  `cargo test --workspace`. `dux-core` stays headless-testable. The Rust test
  count grew across the initiative from ~872 at Phase A to ~1322+ by the auth
  council close (well over 200 of those in `dux-core` alone).
- **Web test suite:** `tsc -b` (typecheck) + `eslint` + `vitest` (`npm run
  test`). The web suite grew from its introduction in the mobile work to ~168+
  tests by the auth step. Pure logic lives in small `lib/*` functions with
  colocated `.test.ts` files (byte synthesis, file-status meta, file tree, auth
  state, sort/reorder, markdown, palette groups, store slices, etc.).
- **Process:** the foundational extraction used characterization-tests-first and
  spec/quality/final reviews per phase; the later web work used **subagent-driven
  development** with **adversarial review per slice** and a **council review**
  (multiple independent reviewers across distinct disciplines) at major
  milestones (auth, TLS, Monaco editor). Several blockers were found
  empirically (gdb-confirmed runtime-drop hang; timing/enumeration probes on
  auth), not just by inspection, and each fix landed with a regression test or a
  fault-injection harness.

---

## 7. Crate / module layout

### `crates/dux` — thin binary
- `main.rs` — arg parsing, surface selection (`dux` TUI vs `dux server`),
  lifecycle orchestration (the in-process flip), status screen wiring. The only
  crate that depends on both `dux-tui` and `dux-web`.

### `crates/dux-core` — headless engine (no ratatui/crossterm/axum)
- `engine/` — `mod.rs` (the `Engine` state container + operations), `command.rs`
  (the `Command` enum + `Engine::apply` dispatch), `events.rs` (`EventReaction`
  + event/command processors), `lifecycle.rs`, `companion.rs`, `config_saver.rs`,
  `in_flight.rs` (`InFlightKey`), `spawn_worker.rs` (the panic-safe spawn
  primitives), `resume_fallback.rs`, `test_support.rs`.
- Domain modules: `action`, `agent_job`, `auth`, `config`, `config_queue`
  (`ConfigWriteQueue`), `config_write`, `diff`, `editor`, `gh`, `git`, `lockfile`,
  `logger`, `macros`, `model`, `palette`, `project_browser`, `provider`, `pty`,
  `resource_stats`, `sidebar`, `startup`, `statusline` (`KeyedStatusController`),
  `storage`, `tailscale`, `theme`, `viewmodel` (`ViewModel`), `welcome`, `wire`,
  `worker` (`WorkerEvent`), `worktree_file`, `io_retry`, `browser`.

### `crates/dux-tui` — terminal surface
- `app/` — `mod.rs` (the `App` struct + run loop), `input.rs`, `render.rs`,
  `sessions.rs`, `workers.rs`, `auth_users.rs`, `text_input.rs`, `test_support.rs`,
  `components/`.
- Surface modules: `cli`, `config` (renderer/validation), `config_saver`, `diff`
  (rendering), `keybindings`, `raw_input`, `clipboard`, `server_screen`
  (`ServerStatusScreen`), `theme`, `tui_color`.

### `crates/dux-web` — web server surface
- `lib.rs` (serving, `serve_with_engine`, gating), `server.rs`,
  `engine_actor.rs` (`EngineHandle`/`EngineRequest`/actor loop), `bootstrap.rs`,
  `protocol.rs` (WS message protocol), `auth.rs`, `tls.rs` (ACME), `console.rs`,
  `file_routes.rs` (editor file list/read/write/image-proxy), `git_routes.rs`
  (validated mutating git endpoints), `web_assets.rs` (embedded SPA).
- `web/` — the React/Vite/Tailwind v4/shadcn SPA: `App.tsx`, `store.ts`, `ws.ts`,
  `components/` (panes, dialogs, `MobileShell`, `EditorOverlay`, `CommandPalette`,
  `TerminalPane`, …), `components/ui/` (shadcn primitives), `lib/` (pure helpers +
  tests), `hooks/` (`use-mobile`, `use-visual-viewport`), PWA `public/`.

---

## 8. Specified items not confirmed shipped (for the reviewer to verify)

- **OIDC/OAuth login** — designed for as a later additional `axum-login` backend;
  explicitly a non-goal for this delivery. Not shipped (bcrypt only). Correctly
  scoped out, not a gap.
- **Shared semantic theme tokens in `dux-core` (spec §4.9 / §5.11)** — the spec
  intended the theme token *values* to live in core so the web derives its
  palette from them. The web UI ships **dark-only via its own shadcn/base-ui
  token palette** (`main.tsx` force-dark; CSS variables), and a `dux_core::theme`
  module exists but is small (~497 bytes — effectively theme identity, not the
  full token-values surface the spec described). The full token-values migration
  into core does **not** appear to have shipped; the web styles through its own
  tokens instead. Worth confirming whether this was intentionally deferred.
- **Inline diff comments** — explicitly a non-goal here; the diff surface was
  only kept forward-compatible (stable per-line anchors). Not shipped, by design.
- **WebGL xterm renderer** — listed as a follow-up "only if the spike shows lag";
  the package.json shows `@xterm/addon-fit` but not the WebGL addon, so the DOM
  renderer is in use. Deferred as planned.
- **Docker image / packaging (build-order step 8)** — the web-surface build order
  ends at packaging (Dockerfile, release artifacts, PWA polish, server-mode site
  docs). The commit log shows no Dockerfile or website/docs commits for server
  mode on this branch; assets are gzip-embedded into the single binary, but the
  Docker/website packaging step appears **not yet done**. Worth verifying against
  the repo root (`website/`, `Dockerfile`).
- **On-device mobile verification** — the mobile-input spike and full-parity
  walkthrough were explicitly left as phone-in-hand tasks for the user; recorded
  as findings, not as shipped/verified code behavior.
