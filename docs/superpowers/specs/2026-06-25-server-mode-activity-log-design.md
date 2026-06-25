# Server-mode activity log panel — design

## Problem

When `dux` flips into server mode from inside the TUI (`Ctrl-Shift-S` / palette
action), it shows a static splash screen (`crates/dux-tui/src/server_screen.rs`):
the ASCII logo, the bound URLs, an uptime counter, an auth line, and exit hints.
It is correct but inert — once it is up, nothing on it changes except the uptime
second.

Meanwhile, the `dux server` CLI path prints a live, colored console of the
server's life: clients connecting and disconnecting, login successes/failures,
logouts, config reloads, ACME lifecycle, and a per-request access log. That
surface is **thrown away** in the flip path: the in-process server is built with
a `Console::noop()` because the status screen owns the terminal and the console
must stay silent.

The result is that the most interesting operational signal — *who is connected
right now, and what just happened* — is invisible exactly when a user is sitting
in front of the server-mode screen watching it.

## Goal

Give the in-TUI server-mode screen a live **Activity** panel: a rolling log of
lifecycle events plus a live **active-connection count**, rendered with the TUI
theme engine in a rounded-border panel consistent with the rest of the app.

## Non-goals

- **No scrollback.** The log is a fixed tail of the most recent events. Older
  lines are dropped. There is deliberately no scroll, no search, no pager.
- **No per-request access log in the panel.** The access log is high-volume and
  would flood a 50-line tail during normal use. It stays a CLI-only surface.
- **No change to the `dux server` CLI path.** That path keeps its real stdout
  console exactly as today. Only the in-TUI flip path gains the panel.
- **No new logging framework, broadcast channel, or protocol layer.** This is a
  redirect of events that already exist into a buffer the screen can read.

## Key insight: the capture seam

`crates/dux-web/src/console.rs` already funnels every lifecycle event through a
single private choke point, `Console::emit(tone, message)`. The toned lifecycle
events — `client_connected`, `client_disconnected`, `login_ok`, `login_failed`,
`login_rate_limited`, `logout`, `acme`, `reload`, `bind_degraded` — all flow
through `emit()`. The two surfaces we explicitly do **not** want in the panel,
`banner()` and `access()`, bypass `emit()` and write pre-formatted strings
directly to the sink.

Therefore: capturing structured events at `emit()` yields *exactly* the lifecycle
set the panel wants and structurally excludes the banner and access log — with no
filtering logic and no risk of drift if new event types are added later. Capturing
the structured `(tone, message)` pair (not the already-ANSI-formatted line) lets
the TUI re-color each event through its own theme rather than parsing ANSI back
out.

## Architecture

### Shared types in `dux-core`

Both `dux-web` (the producer, via `Console`) and `dux-tui` (the consumer, via
`ServerStatusScreen`) already depend on `dux-core`, so the shared buffer lives
there (e.g. `dux_core::activity`).

```rust
/// The tone of a captured activity event — the public mirror of the console's
/// private `Tone`, so the TUI can re-color events with its theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityTone { Info, Ok, Warn, Error }

/// One captured lifecycle event.
pub struct ActivityEvent {
    /// Wall-clock `HH:MM:SS` at capture time (formatted by the producer so the
    /// ring stores no clock dependency).
    pub hms: String,
    pub tone: ActivityTone,
    pub message: String,
}

/// A bounded, thread-safe tail of the most recent activity events plus the live
/// active-connection count. Cheap to clone (`Arc`); shared between the web
/// console (many tokio worker threads) and the status screen (engine-loop thread).
#[derive(Clone)]
pub struct ActivityRing(Arc<ActivityInner>);

struct ActivityInner {
    events: Mutex<VecDeque<ActivityEvent>>, // capped at CAP, drop oldest
    connections: AtomicUsize,               // active WS connections
    generation: AtomicU64,                  // bumped on every push, for redraw detection
}
```

- `CAP = 50`. On push, `push_back`; if `len > CAP`, `pop_front`. No scroll.
- `push(event)` bumps `generation` so the screen can cheaply detect "something new
  happened since I last drew."
- `connection_opened()` / `connection_closed()` adjust `connections`
  (saturating — `connection_closed` never underflows below 0).
- Read side: a `snapshot()` that returns the current generation, the active count,
  and a cheap copy of the tail (the screen only ever shows the last N that fit, so
  the copy is bounded by `CAP`).

### Producer: `Console` capture sink

`Console` gains an optional capture target (an `Option<ActivityRing>` on
`ConsoleInner`, independent of the existing stdout `Sink`). A new constructor
`Console::capture(ring)` builds a console whose stdout sink is `Noop` but whose
capture target is set — used only by the flip path.

- `emit(tone, message)` pushes an `ActivityEvent { hms, tone: tone.into(), message }`
  to the ring (when present) in addition to its existing stdout behavior. Because
  the flip path's stdout sink is `Noop`, the only observable effect there is the
  ring push.
- `client_connected()` additionally calls `ring.connection_opened()`;
  `client_disconnected()` additionally calls `ring.connection_closed()`. These
  are the only two methods that touch the counter, so the bump is explicit and
  parse-free (we never infer connection state from message text).
- `banner()` and `access()` are untouched — they never reach `emit()` and never
  touch the ring, so they stay out of the panel by construction.

`is_active()` semantics are preserved for the existing stdout-gated paths; the
capture push is gated on the capture `Option`, not on `is_active()`.

### Consumer: `ServerStatusScreen`

`main.rs` (the only crate depending on both `dux-tui` and `dux-web`) creates one
`ActivityRing`, builds the flip-path console with `Console::capture(ring.clone())`,
and passes the other clone into `ServerStatusScreen::new(...)`.

Rendering changes in `server_screen.rs`:

- **Header unchanged in spirit.** The ASCII logo, heading, URLs, uptime, and auth
  line stay as today's centered splash content (unboxed header). The pure,
  terminal-free `screen_lines` builder is kept for this region so it stays
  unit-testable without a TTY.
- **New Activity panel.** A full-width, rounded-border (`BorderType::Rounded`,
  `theme.overlay_border`) panel titled `Activity` fills the remaining vertical
  space below the header. The live connection count renders right-aligned in the
  panel title (e.g. `╭─ Activity ──── 3 connected ─╮`).
- **Log rows.** A new pure helper turns the tail of the ring (the last rows that
  fit the panel's inner height) into themed `Line`s. `ActivityTone → Theme`
  mapping: `Info → muted`, `Ok → success`, `Warn → theme.warning_fg`,
  `Error → error`. Exact theme field names are confirmed during planning; a new
  semantic field is added (and wired through every theme/default mapping in the
  same change) only if no existing token fits.
- **Footer unchanged.** Exit hints (`<q>`/`<Esc>` return, `<Ctrl-C>` quit) stay.

### Redraw cadence

`tick()` currently redraws only when the uptime second changes (or on resize).
It gains one more trigger: redraw when the ring's `generation` advanced since the
last draw. This keeps the wall-clock / event-driven discipline (no per-engine-tick
churn) while making new events appear promptly. Resize still forces a redraw.

## Thread-safety notes

- The ring is `Mutex`-guarded. Pushes happen from tokio worker threads (the web
  handlers); the read/snapshot happens on the engine-loop thread. Lock hold times
  are tiny (push one bounded event / copy ≤ 50 events), so there is no risk of
  parking a worker the way a blocking stdout write could — which is why the ring
  does not need the existing console's drop-on-full writer-thread machinery.
- The active-connection counter is an `AtomicUsize`; `connection_closed` uses a
  saturating decrement so a disconnect without a matching connect (or any double
  fire) can never wrap to a huge number.

## Testing

- **`dux-core` (`ActivityRing`)**: caps at 50 and drops the oldest; preserves
  insertion order; `generation` advances on push; `connection_opened` /
  `connection_closed` increment/decrement; `connection_closed` saturates at 0
  (never underflows).
- **`dux-web` (`Console` capture)**: each lifecycle method (`client_connected`,
  `client_disconnected`, `login_ok`, `login_failed`, `login_rate_limited`,
  `logout`, `acme`, `reload`, `bind_degraded`) pushes exactly one structured
  event with the right tone; `access()` and `banner()` push **nothing**;
  `client_connected` / `client_disconnected` move the counter the right way; a
  `Console::capture` console reports the existing stdout-`is_active()` contract
  unchanged.
- **`dux-tui` (`server_screen`)**: the header still renders the logo, URLs,
  uptime, and auth line; the new log helper maps each tone to the expected style;
  only the last N events that fit the panel are rendered (tail behavior); the
  connection count renders in the panel title. Pure helpers stay terminal-free so
  they run without a TTY, matching the existing test discipline in this file.

## Out-of-scope follow-ups (noted, not built)

- Surfacing the access log behind an explicit toggle.
- A richer "currently connected" list (per-client IP + duration) instead of a
  bare count.
- Persisting activity across a flip back to the TUI and a later re-flip.
