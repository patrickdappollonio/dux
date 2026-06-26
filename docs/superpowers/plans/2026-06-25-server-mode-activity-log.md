# Server-mode Activity Log Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the in-TUI server-mode screen a live, themed "Activity" panel that tails the server's lifecycle events (client connect/disconnect, login, logout, reload, ACME) and shows a live active-connection count.

**Architecture:** A bounded, thread-safe `ActivityRing` lives in `dux-core`. The `dux-web` `Console` — the single choke point that already emits these events — gains a capture sink that pushes structured events into the ring at its `emit()` method (which structurally excludes the banner and per-request access log). The flip path builds a capture-backed console and hands the same ring to `dux-tui`'s `ServerStatusScreen`, which renders a rounded, theme-styled panel below the existing logo/status header.

**Tech Stack:** Rust, `ratatui` 0.30 + `crossterm` (TUI), `tokio`/`axum` (web), `std::sync` primitives (`Mutex`, `Arc`, atomics), `chrono` (timestamps).

## Global Constraints

- **Target platforms are macOS and Linux only.** No `#[cfg(windows)]`, no `cfg!(windows)`. Assume Unix.
- **All new UI derives colors/styles from `Theme` (`crates/dux-tui/src/theme.rs`).** Never hardcode `Color::*` in rendering. Reuse an existing semantic field when it fits; only add a new field if none fits, and wire every theme/default mapping in the same change.
- **Rounded borders, theme engine, consistent panels** — match the rest of the TUI (`BorderType::Rounded`, `theme.overlay_border`).
- **No byte-based truncation of user-visible strings.** Use `.chars().count()` / `.chars().take(n)` for any width math on text that can contain multi-byte characters.
- **Wall-clock, not tick-count, for refresh cadence** — redraw decisions key off elapsed seconds and an event generation counter, never a raw tick count.
- **The `dux server` CLI path must not change behavior.** Only the in-TUI flip path gains the capture console + panel.
- **Tests prove the work.** Every task ends green on `cargo test`. The CI gate is `cargo clippy --all-targets --all-features -- -D warnings` — it must pass with zero warnings.
- **Verification commands** (run from repo root): `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

---

### Task 1: `ActivityRing` shared buffer in `dux-core`

**Files:**
- Create: `crates/dux-core/src/activity.rs`
- Modify: `crates/dux-core/src/lib.rs` (register the module)
- Test: inline `#[cfg(test)]` module in `crates/dux-core/src/activity.rs`

**Interfaces:**
- Consumes: nothing (leaf module; `std::sync` + `std::collections::VecDeque`).
- Produces:
  - `pub const ACTIVITY_CAP: usize = 50;`
  - `pub enum ActivityTone { Info, Ok, Warn, Error }` (derives `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub struct ActivityEvent { pub hms: String, pub tone: ActivityTone, pub message: String }` (derives `Clone, Debug`)
  - `pub struct ActivitySnapshot { pub generation: u64, pub connections: usize, pub events: Vec<ActivityEvent> }` (derives `Clone, Debug`)
  - `pub struct ActivityRing(Arc<ActivityInner>)` (derives `Clone`) with:
    - `ActivityRing::new() -> Self` (and `Default`)
    - `fn push(&self, event: ActivityEvent)`
    - `fn connection_opened(&self)`
    - `fn connection_closed(&self)`
    - `fn generation(&self) -> u64`
    - `fn connections(&self) -> usize`
    - `fn snapshot(&self, max_events: usize) -> ActivitySnapshot`

- [ ] **Step 1: Write the failing tests**

Create `crates/dux-core/src/activity.rs` with ONLY the test module first (the types come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ev(msg: &str, tone: ActivityTone) -> ActivityEvent {
        ActivityEvent { hms: "00:00:00".to_string(), tone, message: msg.to_string() }
    }

    #[test]
    fn ring_starts_empty_with_zero_connections() {
        let ring = ActivityRing::new();
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert!(snap.events.is_empty());
        assert_eq!(snap.connections, 0);
        assert_eq!(snap.generation, 0);
    }

    #[test]
    fn push_appends_and_bumps_generation() {
        let ring = ActivityRing::new();
        ring.push(ev("a", ActivityTone::Info));
        ring.push(ev("b", ActivityTone::Ok));
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert_eq!(snap.generation, 2);
        let msgs: Vec<&str> = snap.events.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["a", "b"], "events preserve insertion order");
    }

    #[test]
    fn ring_caps_at_capacity_dropping_oldest() {
        let ring = ActivityRing::new();
        for n in 0..(ACTIVITY_CAP + 10) {
            ring.push(ev(&format!("line{n}"), ActivityTone::Info));
        }
        let snap = ring.snapshot(ACTIVITY_CAP);
        assert_eq!(snap.events.len(), ACTIVITY_CAP, "never exceeds the cap");
        // The oldest 10 were dropped; the tail is line10..line(CAP+9).
        assert_eq!(snap.events.first().unwrap().message, "line10");
        assert_eq!(
            snap.events.last().unwrap().message,
            format!("line{}", ACTIVITY_CAP + 9)
        );
        // Generation counts every push, including the dropped ones.
        assert_eq!(snap.generation, (ACTIVITY_CAP + 10) as u64);
    }

    #[test]
    fn snapshot_returns_only_the_last_max_events() {
        let ring = ActivityRing::new();
        for n in 0..20 {
            ring.push(ev(&format!("l{n}"), ActivityTone::Info));
        }
        let snap = ring.snapshot(5);
        assert_eq!(snap.events.len(), 5);
        assert_eq!(snap.events.first().unwrap().message, "l15");
        assert_eq!(snap.events.last().unwrap().message, "l19");
    }

    #[test]
    fn connection_counter_increments_and_decrements() {
        let ring = ActivityRing::new();
        ring.connection_opened();
        ring.connection_opened();
        assert_eq!(ring.connections(), 2);
        ring.connection_closed();
        assert_eq!(ring.connections(), 1);
    }

    #[test]
    fn connection_close_saturates_at_zero() {
        let ring = ActivityRing::new();
        // A disconnect with no matching connect (or a double-fire) must never wrap.
        ring.connection_closed();
        ring.connection_closed();
        assert_eq!(ring.connections(), 0);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p dux-core activity::`
Expected: FAIL to compile — `ActivityRing`, `ActivityEvent`, `ActivityTone`, `ACTIVITY_CAP` are not defined.

- [ ] **Step 3: Write the minimal implementation**

Prepend the implementation ABOVE the test module in `crates/dux-core/src/activity.rs`:

```rust
//! A bounded, thread-safe tail of the web server's lifecycle events plus a live
//! active-connection count. The web `Console` (the producer, on many tokio
//! worker threads) pushes here; the in-TUI server status screen (the consumer,
//! on the engine-loop thread) reads a [`ActivityRing::snapshot`] each redraw.
//!
//! The buffer is intentionally lossy: it keeps only the most recent
//! [`ACTIVITY_CAP`] events and drops the oldest. There is deliberately no
//! scrollback — the status screen shows a fixed tail.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The maximum number of events the ring retains. Older events are dropped.
pub const ACTIVITY_CAP: usize = 50;

/// The tone of a captured activity event. The public mirror of the web
/// console's private `Tone`, so the TUI can re-color events with its theme
/// instead of parsing ANSI back out of a formatted line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityTone {
    Info,
    Ok,
    Warn,
    Error,
}

/// One captured lifecycle event. `hms` is the wall-clock `HH:MM:SS` formatted by
/// the producer, so the ring carries no clock dependency of its own.
#[derive(Clone, Debug)]
pub struct ActivityEvent {
    pub hms: String,
    pub tone: ActivityTone,
    pub message: String,
}

/// A point-in-time read of the ring: the event generation (for cheap
/// "did anything change?" checks), the live connection count, and the tail of
/// recent events (bounded by the caller's `max_events`).
#[derive(Clone, Debug)]
pub struct ActivitySnapshot {
    pub generation: u64,
    pub connections: usize,
    pub events: Vec<ActivityEvent>,
}

struct ActivityInner {
    events: Mutex<VecDeque<ActivityEvent>>,
    connections: AtomicUsize,
    /// Bumped on every push (including pushes that drop an older event), so a
    /// reader can detect new activity without copying the buffer.
    generation: AtomicU64,
}

/// A cheap-to-clone (`Arc`) shared handle to the activity buffer.
#[derive(Clone)]
pub struct ActivityRing(Arc<ActivityInner>);

impl Default for ActivityRing {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityRing {
    pub fn new() -> Self {
        Self(Arc::new(ActivityInner {
            events: Mutex::new(VecDeque::with_capacity(ACTIVITY_CAP)),
            connections: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
        }))
    }

    /// Append an event, dropping the oldest if the buffer is at capacity, then
    /// bump the generation. The lock is held only for the push/trim.
    pub fn push(&self, event: ActivityEvent) {
        {
            let mut events = self.0.events.lock().unwrap();
            events.push_back(event);
            while events.len() > ACTIVITY_CAP {
                events.pop_front();
            }
        }
        self.0.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_opened(&self) {
        self.0.connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active-connection count, saturating at zero so a disconnect
    /// without a matching connect (or a double fire) can never wrap.
    pub fn connection_closed(&self) {
        let _ = self
            .0
            .connections
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                Some(c.saturating_sub(1))
            });
    }

    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Relaxed)
    }

    pub fn connections(&self) -> usize {
        self.0.connections.load(Ordering::Relaxed)
    }

    /// Snapshot the last `max_events` events plus the current count/generation.
    pub fn snapshot(&self, max_events: usize) -> ActivitySnapshot {
        let events = self.0.events.lock().unwrap();
        let start = events.len().saturating_sub(max_events);
        let tail = events.iter().skip(start).cloned().collect();
        ActivitySnapshot {
            generation: self.generation(),
            connections: self.connections(),
            events: tail,
        }
    }
}
```

Register the module in `crates/dux-core/src/lib.rs` — add this line in the alphabetical `pub mod` block (between `pub mod action;` and `pub mod agent_job;`):

```rust
pub mod activity;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p dux-core activity::`
Expected: PASS (6 tests).

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt
cargo clippy -p dux-core --all-targets --all-features -- -D warnings
git add crates/dux-core/src/activity.rs crates/dux-core/src/lib.rs
git commit -m "Add a bounded ActivityRing for server-mode lifecycle events"
```

---

### Task 2: `Console` capture sink in `dux-web`

**Files:**
- Modify: `crates/dux-web/src/console.rs`
- Test: extend the existing `#[cfg(test)] mod tests` in `crates/dux-web/src/console.rs`

**Interfaces:**
- Consumes: `dux_core::activity::{ActivityRing, ActivityEvent, ActivityTone}` (Task 1).
- Produces:
  - `Console::capture(ring: ActivityRing) -> Console` — a console whose stdout sink is `Noop` but which pushes structured events into `ring`. Used by the flip path (Task 4).
  - Behavior change: `emit()` pushes to the capture ring (when present); `client_connected`/`client_disconnected` additionally move the ring's connection counter. `banner()` and `access()` remain capture-free (they never call `emit()`).

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `mod tests` in `crates/dux-web/src/console.rs` (the `ip(..)` and `sample_banner()` helpers already exist there):

```rust
    use dux_core::activity::{ActivityRing, ActivityTone};

    #[test]
    fn capture_console_pushes_lifecycle_events_with_tones() {
        let ring = ActivityRing::new();
        let console = Console::capture(ring.clone());
        console.client_connected(ip("10.0.0.1"));
        console.login_ok("alice", ip("10.0.0.2"));
        console.login_failed(ip("10.0.0.3"));
        console.acme(true, "order failed");

        let snap = ring.snapshot(dux_core::activity::ACTIVITY_CAP);
        let tones: Vec<ActivityTone> = snap.events.iter().map(|e| e.tone).collect();
        assert_eq!(
            tones,
            vec![
                ActivityTone::Info,  // client connected
                ActivityTone::Ok,    // login ok
                ActivityTone::Warn,  // login failed
                ActivityTone::Error, // acme failure
            ]
        );
        assert!(snap.events[0].message.contains("client connected from 10.0.0.1"));
        assert!(snap.events[1].message.contains("login ok for \"alice\""));
        // The capture stores the structured message — no ANSI escapes.
        assert!(!snap.events[0].message.contains('\u{1b}'));
    }

    #[test]
    fn capture_console_excludes_banner_and_access_log() {
        let ring = ActivityRing::new();
        let console = Console::capture(ring.clone());
        console.banner(&sample_banner());
        console.access("GET", "/api/me", 200, 3);
        // Neither the banner nor the access log flows through emit(), so the ring
        // stays empty — the panel never shows the high-volume access log.
        assert!(ring.snapshot(dux_core::activity::ACTIVITY_CAP).events.is_empty());
        assert_eq!(ring.generation(), 0);
    }

    #[test]
    fn capture_console_tracks_active_connection_count() {
        let ring = ActivityRing::new();
        let console = Console::capture(ring.clone());
        console.client_connected(ip("10.0.0.1"));
        console.client_connected(ip("10.0.0.2"));
        assert_eq!(ring.connections(), 2);
        console.client_disconnected(ip("10.0.0.1"));
        assert_eq!(ring.connections(), 1);
    }

    #[test]
    fn capture_console_is_inactive_for_stdout_purposes() {
        // The capture console has a Noop stdout sink, so the access-log
        // middleware and banner gating (which key off is_active) stay off.
        let ring = ActivityRing::new();
        assert!(!Console::capture(ring).is_active());
    }

    #[test]
    fn noop_console_does_not_capture() {
        // The plain noop (used by the reload arm in non-flip paths and tests)
        // has no ring, so nothing is captured and nothing panics.
        let console = Console::noop();
        console.client_connected(ip("10.0.0.1"));
        assert!(!console.is_active());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p dux-web console::tests::capture_console`
Expected: FAIL to compile — `Console::capture` does not exist.

- [ ] **Step 3: Add the capture target to `ConsoleInner` and constructors**

In `crates/dux-web/src/console.rs`, add the import near the top (with the other `use` lines):

```rust
use dux_core::activity::{ActivityEvent, ActivityRing, ActivityTone};
```

Map the private `Tone` to the public `ActivityTone` (place after the `impl Tone` block):

```rust
impl From<Tone> for ActivityTone {
    fn from(tone: Tone) -> Self {
        match tone {
            Tone::Info => ActivityTone::Info,
            Tone::Ok => ActivityTone::Ok,
            Tone::Warn => ActivityTone::Warn,
            Tone::Error => ActivityTone::Error,
        }
    }
}
```

Add a `capture` field to `ConsoleInner`:

```rust
struct ConsoleInner {
    color: bool,
    sink: Sink,
    /// When set, every `emit()` also pushes a structured event here (and
    /// connect/disconnect move the connection counter). The in-TUI flip path
    /// uses this to feed the status screen's Activity panel. `None` for every
    /// stdout/noop/test console.
    capture: Option<ActivityRing>,
}
```

Now set `capture: None` in EVERY existing `ConsoleInner { .. }` constructor so the crate compiles. There are these sites in this file — update each:
- `with_writer` (the `Self(Arc::new(ConsoleInner { color, sink: Sink::Writer { .. } }))`)
- `test_capture_bounded` (cfg(test))
- `noop` (`sink: Sink::Noop`)

For each, add `capture: None,` to the struct literal. Example for `noop`:

```rust
    pub fn noop() -> Self {
        Self(Arc::new(ConsoleInner {
            color: false,
            sink: Sink::Noop,
            capture: None,
        }))
    }
```

Add the new constructor (place it next to `noop`):

```rust
    /// A capture console: a `Noop` stdout sink (it writes nothing to the
    /// terminal, so the flip's status screen keeps sole ownership of it) whose
    /// every `emit()` pushes a structured [`ActivityEvent`] into `ring` and
    /// whose connect/disconnect calls move the ring's connection counter. The
    /// in-TUI flip path uses this to drive the Activity panel.
    pub fn capture(ring: ActivityRing) -> Self {
        Self(Arc::new(ConsoleInner {
            color: false,
            sink: Sink::Noop,
            capture: Some(ring),
        }))
    }
```

- [ ] **Step 4: Wire the capture into `emit` and the connect/disconnect methods**

Replace the existing `emit` method body so it captures BEFORE the stdout early-return (computing the timestamp once):

```rust
    fn emit(&self, tone: Tone, message: &str) {
        let active = self.is_active();
        // Nothing to do if there is neither a capture target nor a live stdout sink.
        if self.0.capture.is_none() && !active {
            return;
        }
        let hms = now_hms();
        if let Some(ring) = &self.0.capture {
            ring.push(ActivityEvent {
                hms: hms.clone(),
                tone: tone.into(),
                message: message.to_string(),
            });
        }
        if active {
            self.write_line(format_line(self.0.color, tone, &hms, message));
        }
    }
```

Update `client_connected` and `client_disconnected` to move the counter (the `emit` call still records the log line):

```rust
    pub fn client_connected(&self, ip: IpAddr) {
        if let Some(ring) = &self.0.capture {
            ring.connection_opened();
        }
        self.emit(Tone::Info, &format!("client connected from {ip}"));
    }

    pub fn client_disconnected(&self, ip: IpAddr) {
        if let Some(ring) = &self.0.capture {
            ring.connection_closed();
        }
        self.emit(Tone::Info, &format!("client disconnected from {ip}"));
    }
```

Leave `banner()` and `access()` untouched — they call `write_line` directly, never `emit`, so they never capture.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p dux-web console::`
Expected: PASS — the new capture tests pass and every pre-existing console test (noop, writer-thread, banner, access) still passes.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt
cargo clippy -p dux-web --all-targets --all-features -- -D warnings
git add crates/dux-web/src/console.rs
git commit -m "Add a capture sink to the web console feeding an ActivityRing"
```

---

### Task 3: Render the Activity panel in `ServerStatusScreen`

**Files:**
- Modify: `crates/dux-tui/src/server_screen.rs`
- Modify: `crates/dux/src/main.rs` (create the ring, pass it to `ServerStatusScreen::new`)
- Test: extend the existing `#[cfg(test)] mod tests` in `crates/dux-tui/src/server_screen.rs`

**Interfaces:**
- Consumes: `dux_core::activity::{ActivityRing, ActivityTone, ActivityEvent, ActivitySnapshot, ACTIVITY_CAP}` (Task 1).
- Produces:
  - `ServerStatusScreen::new(urls, loopback, auth_enabled, user_count, theme_name, paths, activity: ActivityRing) -> Result<Self>` — the new trailing `activity` parameter.
  - Internal pure helpers: `header_lines(...)`, `footer_hint_lines()`, `activity_lines(events: &[ActivityEvent], max_rows: usize) -> Vec<ScreenLine>`.
  - `Role::Log(ActivityTone)` variant.

This task makes the screen render the panel from whatever the ring holds. The ring is not yet fed by the server (that is Task 4), so until then the panel renders empty with `0 connected`. Each task still compiles and tests pass.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/dux-tui/src/server_screen.rs` (the `one(..)` and `plain_text(..)` helpers already exist there):

```rust
    use dux_core::activity::{ActivityEvent, ActivityTone};

    fn ev(hms: &str, msg: &str, tone: ActivityTone) -> ActivityEvent {
        ActivityEvent { hms: hms.to_string(), tone, message: msg.to_string() }
    }

    #[test]
    fn header_lines_keep_logo_urls_uptime_but_no_exit_hints() {
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, false, 0, 42);
        let text = plain_text(&lines);
        assert!(text.contains("dux server running"));
        assert!(text.contains("http://127.0.0.1:8080"));
        assert!(text.contains("up 0:42"));
        // The exit hints moved to the footer — they are NOT in the header now.
        assert!(!text.contains("return to dux"));
        assert!(!text.contains("quit dux entirely"));
        // The wordmark is still the first line.
        assert_eq!(lines[0][0].1, Role::Logo);
    }

    #[test]
    fn footer_hint_lines_carry_both_exit_keys() {
        let lines = footer_hint_lines();
        let keys: Vec<&str> = lines
            .iter()
            .flatten()
            .filter(|(_, role)| *role == Role::Key)
            .map(|(t, _)| t.as_str())
            .collect();
        assert!(keys.contains(&"q"));
        assert!(keys.contains(&"Esc"));
        assert!(keys.contains(&"Ctrl-C"));
        let text = plain_text(&lines);
        assert!(text.contains("return to dux"));
        assert!(text.contains("quit dux entirely"));
    }

    #[test]
    fn activity_lines_map_each_tone_to_a_log_role() {
        let events = vec![
            ev("10:00:00", "client connected from 10.0.0.5", ActivityTone::Info),
            ev("10:00:01", "login ok for \"pat\"", ActivityTone::Ok),
            ev("10:00:02", "login failed from 10.0.0.9", ActivityTone::Warn),
            ev("10:00:03", "order failed", ActivityTone::Error),
        ];
        let lines = activity_lines(&events, 10);
        assert_eq!(lines.len(), 4);
        // Each row carries the timestamp (muted) and the toned message.
        let roles: Vec<Role> = lines
            .iter()
            .map(|segs| segs.iter().find(|(_, r)| matches!(r, Role::Log(_))).unwrap().1)
            .collect();
        assert_eq!(
            roles,
            vec![
                Role::Log(ActivityTone::Info),
                Role::Log(ActivityTone::Ok),
                Role::Log(ActivityTone::Warn),
                Role::Log(ActivityTone::Error),
            ]
        );
        assert!(plain_text(&lines).contains("client connected from 10.0.0.5"));
        assert!(plain_text(&lines).contains("10:00:00"));
    }

    #[test]
    fn activity_lines_show_only_the_last_max_rows() {
        let events: Vec<ActivityEvent> = (0..20)
            .map(|n| ev("10:00:00", &format!("event{n}"), ActivityTone::Info))
            .collect();
        let lines = activity_lines(&events, 5);
        assert_eq!(lines.len(), 5, "only the last 5 events are rendered");
        let text = plain_text(&lines);
        assert!(text.contains("event15"));
        assert!(text.contains("event19"));
        assert!(!text.contains("event14"));
    }

    #[test]
    fn activity_lines_empty_is_empty() {
        assert!(activity_lines(&[], 10).is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p dux-tui server_screen::`
Expected: FAIL to compile — `header_lines`, `footer_hint_lines`, `activity_lines`, and `Role::Log` do not exist.

- [ ] **Step 3: Add the `Log` role and split the content builders**

In `crates/dux-tui/src/server_screen.rs`, add the import near the top (with the other `use` lines):

```rust
use dux_core::activity::{ActivityEvent, ActivityRing, ActivityTone};
```

Add a `Log` variant to the `Role` enum (it carries the tone so `line_for` can style it):

```rust
    /// One activity-log row's message, styled by its captured tone.
    Log(ActivityTone),
```

Replace the existing `screen_lines` function with a header-only `header_lines` (same content MINUS the trailing spacer + the two exit-hint lines) plus a `footer_hint_lines` and an `activity_lines` builder:

```rust
/// Build the header content (logo, heading, URLs, uptime, auth/security line) as
/// a pure, terminal-free, theme-free description. The exit hints live in
/// [`footer_hint_lines`]; the activity log in [`activity_lines`].
fn header_lines(
    urls: &[String],
    loopback: bool,
    auth_enabled: bool,
    user_count: usize,
    uptime_secs: u64,
) -> Vec<ScreenLine> {
    let mut lines: Vec<ScreenLine> = Vec::new();

    for logo_line in ASCII_LOGO {
        lines.push(vec![(logo_line.to_string(), Role::Logo)]);
    }

    lines.push(vec![(String::new(), Role::Spacer)]);
    lines.push(vec![("dux server running".to_string(), Role::Heading)]);
    for url in urls {
        lines.push(vec![(url.to_string(), Role::Url)]);
    }
    lines.push(vec![(format!("up {}", format_uptime(uptime_secs)), Role::Muted)]);

    if auth_enabled {
        let noun = if user_count == 1 { "user" } else { "users" };
        lines.push(vec![(String::new(), Role::Spacer)]);
        lines.push(vec![(
            format!("Login required. {user_count} {noun} configured."),
            Role::AuthInfo,
        )]);
    } else if !loopback {
        lines.push(vec![(String::new(), Role::Spacer)]);
        lines.push(vec![(
            "Listening beyond this machine with NO authentication. \
             Anyone on the network can control your agents."
                .to_string(),
            Role::Warning,
        )]);
    }

    lines
}

/// The two exit-hint rows shown in the footer (`<q>`/`<Esc>` return, `<Ctrl-C>`
/// quit). These keys are NOT user-configurable bindings: the TUI keybinding
/// system isn't running in server mode, so naming them literally is correct.
fn footer_hint_lines() -> Vec<ScreenLine> {
    vec![
        vec![
            ("q".to_string(), Role::Key),
            ("Esc".to_string(), Role::Key),
            (" stop the server and return to dux".to_string(), Role::HintDesc),
        ],
        vec![
            ("Ctrl-C".to_string(), Role::Key),
            (" quit dux entirely".to_string(), Role::HintDesc),
        ],
    ]
}

/// Build the activity log rows: the last `max_rows` events, oldest first, each a
/// muted `HH:MM:SS` timestamp segment followed by the toned message segment.
fn activity_lines(events: &[ActivityEvent], max_rows: usize) -> Vec<ScreenLine> {
    let start = events.len().saturating_sub(max_rows);
    events[start..]
        .iter()
        .map(|e| {
            vec![
                (format!("{}  ", e.hms), Role::Muted),
                (e.message.clone(), Role::Log(e.tone)),
            ]
        })
        .collect()
}
```

In `line_for`, handle the new `Role::Log(tone)` arm in the `match role` block (add it before the `Role::Key | Role::Spacer` arm). It reuses existing semantic theme fields — Info→muted, Ok→info, Warn→warning, Error→error — so no new theme field is introduced:

```rust
            // Activity-log message: colored by its captured tone, reusing the
            // existing status palette (Info→muted, Ok→info, Warn→warning,
            // Error→error). No new theme field needed.
            Role::Log(tone) => {
                let fg = match tone {
                    ActivityTone::Info => theme.provider_label_fg,
                    ActivityTone::Ok => theme.status_info_fg,
                    ActivityTone::Warn => theme.warning_fg,
                    ActivityTone::Error => theme.status_error_fg,
                };
                Style::default().fg(fg)
            }
```

Also update the width helper `line_render_width`: `Role::Log` is normal text (not a keycap), and the existing `else` branch already covers any non-`Key` role, so no change is needed there — confirm it still compiles.

- [ ] **Step 4: Restructure `draw` and the struct to render the three regions**

Add the ring and the last-drawn generation to the struct (alongside `last_drawn_secs`):

```rust
    /// The shared activity buffer fed by the web console. Snapshotted each draw.
    activity: ActivityRing,
    /// Activity generation most recently drawn, so [`tick`] redraws when a new
    /// event arrives (not only when the uptime second advances).
    last_drawn_generation: u64,
```

Update `ServerStatusScreen::new` to take the trailing `activity: ActivityRing` parameter and initialize the new fields. Change the signature line and the struct literal:

```rust
    pub fn new(
        urls: &[String],
        loopback: bool,
        auth_enabled: bool,
        user_count: usize,
        theme_name: &str,
        paths: &DuxPaths,
        activity: ActivityRing,
    ) -> Result<Self> {
```

…and in the `Self { .. }` literal add (after `last_drawn_secs: u64::MAX,`):

```rust
            activity,
            last_drawn_generation: 0,
```

Note: the initial `screen.draw(0)?` call below the literal must be updated to pass a snapshot — see the new `draw` signature. Replace the bottom of `new` (`screen.draw(0)?; screen.last_drawn_secs = 0; Ok(screen)`) with:

```rust
        let snapshot = screen.activity.snapshot(dux_core::activity::ACTIVITY_CAP);
        screen.last_drawn_generation = snapshot.generation;
        screen.draw(0, &snapshot)?;
        screen.last_drawn_secs = 0;
        Ok(screen)
```

Update `tick` to also redraw when the activity generation advanced. Replace the final block of `tick` (from `let secs = ...` to the end) with:

```rust
        let secs = self.started.elapsed().as_secs();
        let snapshot = self.activity.snapshot(dux_core::activity::ACTIVITY_CAP);
        if secs != self.last_drawn_secs || snapshot.generation != self.last_drawn_generation {
            let _ = self.draw(secs, &snapshot);
            self.last_drawn_secs = secs;
            self.last_drawn_generation = snapshot.generation;
        }
        ServerScreenTick::Continue
```

(The resize arm above already sets `self.last_drawn_secs = u64::MAX;` to force a redraw — that still works.)

Replace the whole `draw` method with the three-region layout: a top header (centered, no border), the rounded Activity panel filling the middle, and the footer hints at the bottom:

```rust
    /// Draw one frame: the header (logo + status), the Activity panel, and the
    /// footer hints. `snapshot` is taken by the caller so `tick` can compare the
    /// generation without snapshotting twice.
    fn draw(&mut self, uptime_secs: u64, snapshot: &ActivitySnapshot) -> Result<()> {
        let theme = &self.theme;
        let header = header_lines(
            &self.urls,
            self.loopback,
            self.auth_enabled,
            self.user_count,
            uptime_secs,
        );
        let footer = footer_hint_lines();
        self.terminal.draw(|frame| {
            let area = frame.area();
            // Fill the whole frame with the theme background.
            frame.render_widget(Clear, area);
            let bg = Block::default().style(Style::default().bg(theme.app_bg));
            frame.render_widget(bg, area);

            // Vertical split: header (its content height), Activity (fills the
            // rest, min 3 rows for a border + one line), footer (its rows + a
            // one-row gap above for breathing room).
            let full_width = area.width.max(1);
            let header_rows: u16 = header
                .iter()
                .map(|segs| wrapped_row_count(segs, full_width))
                .sum();
            let footer_rows = footer.len() as u16 + 1;
            let chunks = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    ratatui::layout::Constraint::Length(header_rows),
                    ratatui::layout::Constraint::Min(3),
                    ratatui::layout::Constraint::Length(footer_rows),
                ])
                .split(area);

            // ── Header (centered, no border) ────────────────────────────────
            let header_text: Vec<Line> = header.iter().map(|s| line_for(s, theme)).collect();
            let header_para = Paragraph::new(header_text)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(header_para, chunks[0]);

            // ── Activity panel (rounded, themed) ────────────────────────────
            let count = snapshot.connections;
            let count_label = format!(" {count} connected ");
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.overlay_border))
                .style(Style::default().bg(theme.app_bg))
                .padding(Padding::horizontal(1))
                .title(Line::from(Span::styled(
                    " Activity ",
                    Style::default()
                        .fg(theme.title_focused)
                        .add_modifier(Modifier::BOLD),
                )))
                .title(
                    Line::from(Span::styled(
                        count_label,
                        Style::default().fg(theme.provider_label_fg),
                    ))
                    .right_aligned(),
                );
            // Visible rows = inner height (panel height minus the two borders).
            let inner_rows = chunks[1].height.saturating_sub(2) as usize;
            let log = activity_lines(&snapshot.events, inner_rows);
            let log_text: Vec<Line> = log.iter().map(|s| line_for(s, theme)).collect();
            let log_para = Paragraph::new(log_text)
                .alignment(Alignment::Left)
                .block(block);
            frame.render_widget(log_para, chunks[1]);

            // ── Footer hints (centered) ─────────────────────────────────────
            let footer_text: Vec<Line> = footer.iter().map(|s| line_for(s, theme)).collect();
            let footer_para = Paragraph::new(footer_text)
                .alignment(Alignment::Center)
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(footer_para, chunks[2]);
        })?;
        Ok(())
    }
```

Remove the now-unused `content_width_for` helper and its test (`content_width_excludes_the_wrapping_warning`) — the centered single-box width math is gone. Keep `wrapped_row_count` and `line_render_width` (still used for header sizing). If `clippy` flags any other now-unused import (e.g. `Alignment` is still used; `Padding` is still used), resolve per the compiler.

- [ ] **Step 5: Update the `main.rs` caller**

In `crates/dux/src/main.rs`, inside the `FlipToServer` arm, create the ring before constructing the screen and pass it in. Add right after the `let user_count = ...;` line (around line 79):

```rust
                // The activity buffer is shared between the web console (the
                // producer, wired in serve_with_engine) and the status screen
                // (the consumer). Created here so both get the same handle.
                let activity = dux_core::activity::ActivityRing::new();
```

Update the `ServerStatusScreen::new(..)` call to pass `activity.clone()` as the final argument:

```rust
                let mut screen = match dux_tui::ServerStatusScreen::new(
                    &urls,
                    loopback,
                    auth_enabled,
                    user_count,
                    &theme_name,
                    &paths,
                    activity.clone(),
                ) {
```

(Task 4 will also pass `activity` into `serve_with_engine`; for now the ring is created and the screen reads it, but nothing feeds it yet — the panel renders empty.)

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p dux-tui server_screen::`
Expected: PASS — the new header/footer/activity tests pass; the remaining pre-existing `screen_lines`-based tests were renamed to `header_lines`/`footer_hint_lines` (update any that still reference `screen_lines` — `content_includes_url_and_heading_and_uptime`, `content_lists_all_bound_urls`, `loopback_auth_off_omits_the_warning`, `non_loopback_auth_off_includes_the_loud_warning`, `auth_on_shows_quiet_login_line_not_the_warning`, `auth_on_singular_user_uses_singular_noun`, `content_includes_the_wordmark` → point them at `header_lines`; `content_includes_both_exit_hints` → split into the `footer_hint_lines` test above).

- [ ] **Step 7: Build the binary to confirm the caller compiles**

Run: `cargo build -p dux`
Expected: Success (the new `ServerStatusScreen::new` argument is supplied).

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt
cargo clippy -p dux-tui -p dux --all-targets --all-features -- -D warnings
git add crates/dux-tui/src/server_screen.rs crates/dux/src/main.rs
git commit -m "Render a themed Activity panel on the server status screen"
```

---

### Task 4: Feed the panel — capture console through `serve_with_engine`

**Files:**
- Modify: `crates/dux-web/src/lib.rs` (`serve_with_engine` signature + console wiring)
- Modify: `crates/dux/src/main.rs` (pass the ring into `serve_with_engine`)
- Test: extend `#[cfg(test)] mod tests` in `crates/dux-web/src/lib.rs` if a seam is reachable (see Step 4); otherwise rely on the Task 2 capture tests + the integration assertion below.

**Interfaces:**
- Consumes: `dux_core::activity::ActivityRing` (Task 1), `Console::capture` (Task 2).
- Produces: `serve_with_engine(engine, listeners, activity: ActivityRing, on_tick) -> Result<(Engine, ServerExit)>` — a new `activity` parameter threaded before `on_tick`. The flip path's router and reload arm now use `Console::capture(activity)` instead of `Console::noop()`.

- [ ] **Step 1: Add the `activity` parameter and build the capture console**

In `crates/dux-web/src/lib.rs`, change the `serve_with_engine` signature to accept the ring:

```rust
pub fn serve_with_engine(
    mut engine: Engine,
    listeners: Vec<std::net::TcpListener>,
    activity: dux_core::activity::ActivityRing,
    mut on_tick: impl FnMut() -> ServerTick,
) -> Result<(Engine, ServerExit)> {
```

Immediately after the signature (before `let auth = ...`), build the capture console once and clone it where needed:

```rust
    // The flip owns the terminal with its themed status screen, so this console
    // writes NOTHING to stdout — but it captures every lifecycle event into the
    // shared ring that drives the status screen's Activity panel.
    let console = Console::capture(activity);
```

Replace the `AuthReloadContext { .. console: Console::noop() }` (around line 985) so the reload arm also captures:

```rust
            console: console.clone(),
```

Add `.with_console(console.clone(), false)` to the flip router build (around line 1057). The flip keeps the access log OFF (the access log is never wanted in the panel and `access()` does not capture anyway):

```rust
    let (app, sweep_store) = server::build_app(
        handle.clone(),
        Arc::clone(&auth),
        axum::Router::new(),
        RouterParams::plain_http()
            .with_console(console.clone(), false)
            .with_max_websocket_connections(engine.config.server.max_websocket_connections),
    );
```

- [ ] **Step 2: Update the `main.rs` caller to pass the ring**

In `crates/dux/src/main.rs`, pass `activity` into `serve_with_engine` (the ring was created in Task 3, Step 5). Change the call (around line 104):

```rust
                let (engine, exit) = dux_web::serve_with_engine(*engine, listeners, activity, || {
```

(The screen got `activity.clone()`; `serve_with_engine` takes ownership of the original `activity` — both share the same underlying `Arc`.)

- [ ] **Step 3: Build the whole workspace to confirm the wiring**

Run: `cargo build`
Expected: Success. If any in-crate test in `dux-web` calls `serve_with_engine` directly, update it to pass `dux_core::activity::ActivityRing::new()` as the new argument.

- [ ] **Step 4: Add a wiring regression test (if a seam exists)**

`serve_with_engine` spins a full tokio server, so do not start it in a unit test. Instead, prove the wiring contract that matters — that the flip path uses a *capturing* console, not a noop — by asserting the helper that builds it. Add to `crates/dux-web/src/lib.rs` `mod tests`:

```rust
    #[test]
    fn flip_console_captures_into_the_shared_ring() {
        // The flip path builds its console from the shared ring; a client-connect
        // event on that console must land in the ring the status screen reads.
        let ring = dux_core::activity::ActivityRing::new();
        let console = crate::console::Console::capture(ring.clone());
        console.client_connected("10.0.0.7".parse().unwrap());
        assert_eq!(ring.connections(), 1);
        assert_eq!(ring.snapshot(dux_core::activity::ACTIVITY_CAP).events.len(), 1);
    }
```

(This guards the contract Task 4 relies on: `Console::capture` + the ring is the seam `serve_with_engine` uses. The end-to-end "real server pushes on real WS connect" is covered by manual smoke testing in Task 5.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p dux-web`
Expected: PASS.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
git add crates/dux-web/src/lib.rs crates/dux/src/main.rs
git commit -m "Feed the server-mode Activity panel through a capture console"
```

---

### Task 5: Documentation sync, full verification, and smoke test

**Files:**
- Modify (if they describe the server-mode screen): `README.md`, `website/` content, `website/docs/*.md`
- No test file — this is the integration/verification task.

**Interfaces:**
- Consumes: the complete feature (Tasks 1-4).
- Produces: accurate docs and a green, lint-clean workspace.

- [ ] **Step 1: Check whether docs describe the server-mode screen**

Run: `grep -rin "server status\|server mode\|status screen\|dux server\|flip" README.md website/ 2>/dev/null | grep -vi node_modules`
Expected: a list of references. For each that describes what the server-mode screen *shows*, update the prose to mention the live Activity panel and connection count. Match the site's existing playful tone (per CLAUDE.md). Do NOT enumerate keybindings. If nothing describes the screen's contents, note that and make no change.

- [ ] **Step 2: If the website documents server mode, add the Activity panel**

If `website/docs/` has a server-mode page, add a short, accurate sentence: the in-TUI server screen now shows a rolling activity log (clients connecting/disconnecting, logins) and a live connection count, capped to the most recent events with no scrollback. Keep values accurate (cap = 50). Skip if no such page exists.

- [ ] **Step 3: Run the full verification suite**

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
Expected: fmt clean, zero clippy warnings, all tests pass.

- [ ] **Step 4: Manual smoke test (ask the user to run)**

Ask the user to run `cargo run`, flip into server mode (the configured flip key / palette action), open the web URL in a browser, and confirm: (a) the Activity panel appears with rounded borders in the theme colors; (b) opening/closing the browser tab makes `client connected` / `client disconnected` lines appear and the `N connected` count change; (c) `q`/`Esc` still returns to the TUI and `Ctrl-C` still quits. Per CLAUDE.md, do not run the interactive TUI automatically — request this as a final sanity check.

- [ ] **Step 5: Final commit (docs only, if any changed)**

```bash
git add README.md website/
git commit -m "Document the server-mode Activity panel"
```

(Skip if Step 1/2 changed nothing.)

---

## Self-Review

**Spec coverage:**
- Live connection count → Task 1 (counter), Task 2 (console moves it), Task 3 (title render). ✓
- Rolling lifecycle log capped at 50, drop oldest, no scroll → Task 1 (`ACTIVITY_CAP`, drop-oldest), Task 3 (tail render). ✓
- Capture at the `emit()` choke point; exclude banner + access log → Task 2 (+ explicit exclusion test). ✓
- Keep ASCII logo; themed rounded panel → Task 3 (`header_lines` keeps `ASCII_LOGO`; `BorderType::Rounded` + `theme.overlay_border`). ✓
- Shared type in `dux-core` (both crates depend on it) → Task 1. ✓
- Redraw on new event, wall-clock cadence → Task 3 (`generation` compare in `tick`). ✓
- `dux server` CLI path unchanged → only `serve_with_engine` (flip) touched; `build_console` (CLI) untouched. ✓
- Tone→theme via existing fields, no new theme field → Task 3 (`provider_label_fg`/`status_info_fg`/`warning_fg`/`status_error_fg`). ✓
- Tests at every layer → Tasks 1-4 each ship tests. ✓
- Docs/site sync → Task 5. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; test bodies are concrete.

**Type consistency:** `ActivityRing`, `ActivityEvent`, `ActivityTone`, `ActivitySnapshot`, `ACTIVITY_CAP` are defined in Task 1 and referenced with the same names/signatures in Tasks 2-4. `Console::capture(ActivityRing)`, `header_lines`/`footer_hint_lines`/`activity_lines`, `Role::Log(ActivityTone)`, and the `serve_with_engine(.., activity, on_tick)` / `ServerStatusScreen::new(.., activity)` signatures are consistent across the tasks that produce and consume them.
