//! The `dux server` terminal console: vite-style, colored, well-formatted
//! output that shows the server's life as it runs (bind banner, client
//! connect/disconnect, auth events, ACME lifecycle, config reload, and a
//! per-request access log).
//!
//! ## Scope
//!
//! This is the `dux server` CLI surface ONLY. The in-process TUI↔server flip
//! ([`crate::serve_with_engine`]) keeps its themed status screen and owns the
//! terminal, so it MUST NOT print here — it constructs a [`Console::noop`] so
//! every emit call is a cheap no-op and stdout stays untouched.
//!
//! ## What goes where
//!
//! The console is ADDITIVE to `dux.log`: lifecycle events the rest of the crate
//! already logs keep logging exactly as before; the console is a second, richer,
//! human-facing surface. The ONE exception is the access log, which is
//! console-only (piping `dux server`'s stdout IS the access log; keeping it out
//! of `dux.log` keeps the log lean).
//!
//! ## Color
//!
//! Color is hand-rolled minimal ANSI (no color dependency). [`detect`] decides
//! whether to color from the `[server] color` setting plus the environment
//! (`IsTerminal`, `NO_COLOR`, `TERM`). When color is OFF the formatters emit
//! plain ASCII with word glyphs (`info`/`ok`/`warn`/`error`) instead of the
//! Unicode glyph vocabulary, so piped/redirected output is clean.

use std::io::{IsTerminal, Write};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

// ── ANSI palette (hand-rolled — no color dependency) ───────────────────────

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

/// The tone of a console line — drives both the glyph and the color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Ok,
    Warn,
    Error,
}

impl Tone {
    /// The Unicode glyph used in color/terminal mode (vite-ish vocabulary).
    fn glyph(self) -> &'static str {
        match self {
            Tone::Info => "\u{279c}",  // ➜
            Tone::Ok => "\u{2713}",    // ✓
            Tone::Warn => "\u{26a0}",  // ⚠
            Tone::Error => "\u{2717}", // ✗
        }
    }

    /// The plain-ASCII label used when color is off (piped/NO_COLOR/dumb), so a
    /// redirected log never carries Unicode glyphs or escape codes.
    fn label(self) -> &'static str {
        match self {
            Tone::Info => "info",
            Tone::Ok => "ok",
            Tone::Warn => "warn",
            Tone::Error => "error",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Tone::Info => CYAN,
            Tone::Ok => GREEN,
            Tone::Warn => YELLOW,
            Tone::Error => RED,
        }
    }
}

// ── Detection ──────────────────────────────────────────────────────────────

/// The runtime inputs [`decide_color`] consults, injected so the decision is a
/// pure, exhaustively testable function (no env mutation in tests).
#[derive(Debug, Clone)]
pub struct ColorInputs<'a> {
    /// The `[server] color` setting (`auto` / `always` / `never`; any other value
    /// is treated as `auto`).
    pub setting: &'a str,
    /// Whether stdout is a real terminal (`std::io::stdout().is_terminal()`).
    pub stdout_is_terminal: bool,
    /// The `NO_COLOR` environment variable, if set. Per the `NO_COLOR` spec, any
    /// non-empty value disables color in `auto` mode.
    pub no_color: Option<&'a str>,
    /// The `TERM` environment variable, if set. `dumb` disables color in `auto`.
    pub term: Option<&'a str>,
}

/// Pure color decision over injected inputs.
///
/// - `always` → on (ignores everything else).
/// - `never`  → off.
/// - `auto` (or any unrecognized value) → on only when stdout is a terminal AND
///   `NO_COLOR` is unset/empty AND `TERM` is not `dumb`.
pub fn decide_color(inputs: &ColorInputs<'_>) -> bool {
    match inputs.setting {
        "always" => true,
        "never" => false,
        _ => {
            let no_color_active = inputs.no_color.is_some_and(|v| !v.is_empty());
            let term_dumb = inputs.term == Some("dumb");
            inputs.stdout_is_terminal && !no_color_active && !term_dumb
        }
    }
}

/// Whether `setting` is a recognized `[server] color` value. An unrecognized
/// value is honored as `auto` but the caller warns so a typo is visible.
pub fn is_known_color_setting(setting: &str) -> bool {
    matches!(setting, "auto" | "always" | "never")
}

/// Read the real environment and decide whether to color, given the configured
/// `[server] color` setting. The thin caller over [`decide_color`].
pub fn detect(setting: &str) -> bool {
    let no_color = std::env::var("NO_COLOR").ok();
    let term = std::env::var("TERM").ok();
    decide_color(&ColorInputs {
        setting,
        stdout_is_terminal: std::io::stdout().is_terminal(),
        no_color: no_color.as_deref(),
        term: term.as_deref(),
    })
}

// ── Writer seam ────────────────────────────────────────────────────────────

/// The bound on the writer channel. Emitters `try_send` into it; when it is full
/// (a stalled stdout consumer — a full pipe or a `Ctrl-S`-stopped terminal) the
/// line is DROPPED rather than blocking the emitting tokio worker. 1024 lines is
/// generous headroom for a momentary stall while staying a fixed, bounded cost.
const WRITER_CHANNEL_BOUND: usize = 1024;

/// A message handed to the writer thread.
enum WriterMsg {
    /// One already-formatted line to write + flush.
    Line(String),
    /// A test-only sync barrier: the writer thread sends `()` back once it has
    /// processed every message queued before this one, giving tests a
    /// deterministic drain with no sleeps. Never constructed in production.
    #[cfg(test)]
    Sync(SyncSender<()>),
}

/// Where the console writes. Production hands lines to a dedicated writer THREAD
/// over a bounded channel so a stalled stdout consumer can never park an emitting
/// tokio worker on a blocking `write()`; tests inject an in-memory buffer the
/// thread writes into and read it back through a deterministic sync barrier.
enum Sink {
    /// No output at all — the flip path and any disabled console. Every emit is
    /// a cheap no-op.
    Noop,
    /// The bounded sender to the writer thread. `emit`/`access`/`banner`
    /// `try_send` into it; a full channel means a slow consumer, so the line is
    /// dropped (accounted on `dropped`) instead of blocking the runtime. The
    /// writer thread owns the actual `Box<dyn Write + Send>`.
    Writer {
        tx: SyncSender<WriterMsg>,
        /// Lines dropped while the channel was full, awaiting a one-line warning
        /// the next successful send emits. Relaxed: it only gates a human-facing
        /// warning, so exact cross-thread ordering does not matter.
        dropped: AtomicU64,
    },
}

/// The shared console handle. Cheap to clone (`Arc`) and cheap to no-op (the
/// flip path constructs [`Console::noop`], whose emit calls return immediately).
///
/// ## Shutdown
///
/// The writer thread lives as long as any `Console` clone does: it loops on the
/// channel receiver, so when the LAST `Console` drops, the `SyncSender` is
/// dropped, the channel closes, the loop ends, and the thread exits on its own —
/// no detached-thread hang. The thread is a plain (daemon-style) `std::thread`,
/// so even a hard process exit while a `Console` is still alive simply tears it
/// down with the process; it never blocks shutdown.
#[derive(Clone)]
pub struct Console(Arc<ConsoleInner>);

struct ConsoleInner {
    color: bool,
    sink: Sink,
}

impl Console {
    /// A real console writing to stdout. `color` comes from [`detect`].
    pub fn stdout(color: bool) -> Self {
        Self::with_writer(color, Box::new(std::io::stdout()))
    }

    /// Build a writer-backed console over an owned writer: spawn the dedicated
    /// writer thread (owns the writer; writes + flushes per line; exits when the
    /// channel closes) and keep only the bounded sender + the drop counter.
    fn with_writer(color: bool, writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WriterMsg>(WRITER_CHANNEL_BOUND);
        std::thread::Builder::new()
            .name("dux-console-writer".to_string())
            .spawn(move || writer_loop(writer, rx))
            .expect("spawn console writer thread");
        Self(Arc::new(ConsoleInner {
            color,
            sink: Sink::Writer {
                tx,
                dropped: AtomicU64::new(0),
            },
        }))
    }

    /// A writer-backed console with a CUSTOM channel bound and an injected writer,
    /// returning the console plus a read handle. Test-only: it lets the drop-on-full
    /// test pick a tiny bound (e.g. 2) and a deliberately-slow writer so the bound
    /// fills deterministically.
    #[cfg(test)]
    fn test_capture_bounded(color: bool, bound: usize, writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WriterMsg>(bound);
        std::thread::Builder::new()
            .name("dux-console-writer-test".to_string())
            .spawn(move || writer_loop(writer, rx))
            .expect("spawn console writer thread");
        Self(Arc::new(ConsoleInner {
            color,
            sink: Sink::Writer {
                tx,
                dropped: AtomicU64::new(0),
            },
        }))
    }

    /// The bounded sender to the writer thread, for tests that need to drive the
    /// `Sync` barrier directly against a console built via [`Self::test_capture_bounded`].
    #[cfg(test)]
    fn writer_tx(&self) -> SyncSender<WriterMsg> {
        match &self.0.sink {
            Sink::Writer { tx, .. } => tx.clone(),
            Sink::Noop => unreachable!("a writer console always has a sender"),
        }
    }

    /// The current dropped-line count (test-only visibility into the drop
    /// accounting). Zero on a no-op console.
    #[cfg(test)]
    fn dropped_count(&self) -> u64 {
        match &self.0.sink {
            Sink::Writer { dropped, .. } => dropped.load(Ordering::Relaxed),
            Sink::Noop => 0,
        }
    }

    /// A no-op console: every emit returns immediately and NOTHING is written.
    /// The TUI flip uses this so the status screen keeps sole ownership of the
    /// terminal.
    pub fn noop() -> Self {
        Self(Arc::new(ConsoleInner {
            color: false,
            sink: Sink::Noop,
        }))
    }

    /// A buffer-backed console plus a handle to read what it wrote, for unit
    /// tests in OTHER modules of this crate (e.g. the access-log middleware e2e in
    /// `server.rs`). `pub(crate)` + `#[cfg(test)]` so it never ships. The writer
    /// thread writes into the shared buffer; [`TestSink::contents`] drains it
    /// deterministically through the sync barrier.
    #[cfg(test)]
    pub(crate) fn test_capture(color: bool) -> (Self, TestSink) {
        let buf = SharedBuffer::new();
        let console = Self::with_writer(color, Box::new(buf.clone()));
        let tx = match &console.0.sink {
            Sink::Writer { tx, .. } => tx.clone(),
            Sink::Noop => unreachable!("with_writer always builds a Writer sink"),
        };
        (console, TestSink { buf, tx })
    }

    /// Whether this console actually writes anything. The flip's regression guard
    /// asserts a no-op console reports `false`.
    pub fn is_active(&self) -> bool {
        !matches!(self.0.sink, Sink::Noop)
    }

    /// Hand one already-formatted line to the writer thread. A `Noop` console
    /// drops it. A writer `try_send`s: on a full channel (a stalled stdout
    /// consumer) the line is DROPPED and counted; on a successful send after a
    /// drop streak, a single warning line is emitted first so the operator knows
    /// output fell behind.
    fn write_line(&self, line: String) {
        let Sink::Writer { tx, dropped } = &self.0.sink else {
            return;
        };
        // If we previously dropped lines and the channel now has room, surface a
        // single warning BEFORE this line so the gap is visible and accounted.
        let drop_streak = dropped.load(Ordering::Relaxed);
        if drop_streak > 0 {
            let warn = format_line(
                self.0.color,
                Tone::Warn,
                &now_hms(),
                &format!(
                    "console output fell behind — {drop_streak} line(s) dropped \
                     (slow stdout consumer)"
                ),
            );
            // Only clear the streak if the warning itself made it through, so a
            // still-full channel keeps accumulating rather than losing the notice.
            if tx.try_send(WriterMsg::Line(warn)).is_ok() {
                dropped.fetch_sub(drop_streak, Ordering::Relaxed);
            }
        }
        match tx.try_send(WriterMsg::Line(line)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            // The writer thread is gone (only on process teardown): nothing to do.
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Format a timestamped, toned line and emit it. Early-returns on a no-op
    /// console BEFORE doing any timestamp/format work, so the flip path pays
    /// nothing per emit.
    fn emit(&self, tone: Tone, message: &str) {
        if !self.is_active() {
            return;
        }
        self.write_line(format_line(self.0.color, tone, &now_hms(), message));
    }

    // ── Event renderers (the public emit surface) ──────────────────────────

    /// The post-bind startup banner. Multi-line: a header, one row per bound
    /// listener, the login row, and any ⚠ degradation rows.
    pub fn banner(&self, banner: &Banner) {
        if !self.is_active() {
            return;
        }
        for line in render_banner(self.0.color, banner) {
            self.write_line(line);
        }
    }

    pub fn client_connected(&self, ip: IpAddr) {
        self.emit(Tone::Info, &format!("client connected from {ip}"));
    }

    pub fn client_disconnected(&self, ip: IpAddr) {
        self.emit(Tone::Info, &format!("client disconnected from {ip}"));
    }

    /// A successful login. The username IS logged here (success is not an
    /// enumeration leak — the operator wants to know who got in).
    pub fn login_ok(&self, username: &str, ip: IpAddr) {
        self.emit(Tone::Ok, &format!("login ok for \"{username}\" from {ip}"));
    }

    /// A failed login. NEVER logs the attempted username (enumeration/log
    /// hygiene per the auth slices) — IP only.
    pub fn login_failed(&self, ip: IpAddr) {
        self.emit(Tone::Warn, &format!("login failed from {ip}"));
    }

    /// A rate-limited login attempt. IP only (same hygiene as a failure).
    pub fn login_rate_limited(&self, ip: IpAddr) {
        self.emit(
            Tone::Warn,
            &format!("login rate-limited from {ip} (too many failed attempts)"),
        );
    }

    /// A logout. The username IS logged (the session was already authenticated).
    pub fn logout(&self, username: &str) {
        self.emit(Tone::Info, &format!("logout for \"{username}\""));
    }

    /// An ACME certificate-lifecycle event, already classified into a tone +
    /// message by the caller (the same classification `dux.log` uses).
    pub fn acme(&self, tone_is_error: bool, message: &str) {
        let tone = if tone_is_error { Tone::Error } else { Tone::Ok };
        self.emit(tone, message);
    }

    /// A config reload landed. `refused`/`rebind_changed` drive the tone so a
    /// refusal or a restart-needed reload reads as a warning, a clean reload as
    /// info.
    pub fn reload(&self, message: &str, warn: bool) {
        let tone = if warn { Tone::Warn } else { Tone::Info };
        self.emit(tone, message);
    }

    /// A best-effort listener bind that degraded (e.g. a busy Tailscale leg).
    pub fn bind_degraded(&self, message: &str) {
        self.emit(Tone::Warn, message);
    }

    /// One access-log line. Console-only; gated by the caller on the `access_log`
    /// config AND console activity. Early-returns on a no-op console BEFORE the
    /// timestamp/format work so a disabled console pays nothing per request.
    pub fn access(&self, method: &str, path: &str, status: u16, latency_ms: u128) {
        if !self.is_active() {
            return;
        }
        self.write_line(format_access_line(
            self.0.color,
            &now_hms(),
            method,
            path,
            status,
            latency_ms,
        ));
    }
}

/// The dedicated writer thread body: own the writer, drain the channel, write +
/// flush each line. A `Sync` barrier (test-only) is acknowledged once everything
/// before it has been processed. The loop ends — and the thread exits — when the
/// channel closes (the last `Console` dropped its sender).
fn writer_loop(mut writer: Box<dyn Write + Send>, rx: std::sync::mpsc::Receiver<WriterMsg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            WriterMsg::Line(line) => {
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
            #[cfg(test)]
            WriterMsg::Sync(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

// ── Pure formatting ─────────────────────────────────────────────────────────

/// Current wall-clock time as `HH:MM:SS`. Wall-clock, not a tick counter, per the
/// project's animation/refresh tenet.
fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Format the dim timestamp prefix (or the bare time when color is off).
fn timestamp_prefix(color: bool, hms: &str) -> String {
    if color {
        format!("{DIM}{hms}{RESET}")
    } else {
        hms.to_string()
    }
}

/// Render the tone marker: the colored Unicode glyph in color mode, the plain
/// word label otherwise.
fn tone_marker(color: bool, tone: Tone) -> String {
    if color {
        format!("{}{}{}", tone.color(), tone.glyph(), RESET)
    } else {
        tone.label().to_string()
    }
}

/// Format a complete toned line: `<ts> <marker> <message>`. Pure so both modes
/// are unit-tested.
fn format_line(color: bool, tone: Tone, hms: &str, message: &str) -> String {
    format!(
        "{} {} {message}",
        timestamp_prefix(color, hms),
        tone_marker(color, tone)
    )
}

/// The status-class color for an access-log status code (2xx green, 3xx cyan,
/// 4xx yellow, 5xx red, anything else uncolored).
fn status_color(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => Some(GREEN),
        300..=399 => Some(CYAN),
        400..=499 => Some(YELLOW),
        500..=599 => Some(RED),
        _ => None,
    }
}

/// Format one access-log line: `<ts> <METHOD> <path> <status> <latency>ms`. The
/// status code is colored by class in color mode; plain otherwise. The path is
/// printed AS-IS, including any query string — the only query strings that reach
/// the server are public ACME challenge tokens; nothing sensitive rides paths
/// today.
fn format_access_line(
    color: bool,
    hms: &str,
    method: &str,
    path: &str,
    status: u16,
    latency_ms: u128,
) -> String {
    let status_text = match (color, status_color(status)) {
        (true, Some(c)) => format!("{c}{status}{RESET}"),
        _ => status.to_string(),
    };
    let method_text = if color {
        format!("{BOLD}{method}{RESET}")
    } else {
        method.to_string()
    };
    format!(
        "{} {method_text} {path} {status_text} {latency_ms}ms",
        timestamp_prefix(color, hms)
    )
}

// ── Banner ───────────────────────────────────────────────────────────────────

/// One labeled, bound listener row in the startup banner.
#[derive(Debug, Clone)]
pub struct ListenerRow {
    /// The label for this leg (e.g. `Local`, `Tailscale`, or a domain).
    pub label: String,
    /// The full URL a user opens.
    pub url: String,
    /// An optional trailing note (e.g. the ACME redirect note).
    pub note: Option<String>,
}

/// The login-state row.
#[derive(Debug, Clone)]
pub enum LoginRow {
    /// The gate is on with `count` configured user(s).
    Enabled { count: usize },
    /// The gate is OFF via `--disable-auth` — a loud red warning.
    Disabled,
    /// No login is required because there are zero valid users (the gate is OFF
    /// per `auth_enabled`, but this is NOT `--disable-auth`). The tone tracks how
    /// reachable the server is:
    /// - `reachable: false` → loopback-only, a calm muted "local only" note.
    /// - `reachable: true` → a non-loopback leg is bound, a ⚠ warning.
    ///
    /// `public` only refines the warning text: a required non-loopback leg (an
    /// explicit `listen_addrs` public/LAN entry) gets the stronger wording, while
    /// a best-effort (Tailscale local-mode) leg gets the Tailscale-specific one.
    /// Ignored when `reachable` is false.
    NoLoginRequired { reachable: bool, public: bool },
}

/// Everything the post-bind banner needs. Built by the serve path from the
/// addresses that ACTUALLY bound, so it shows truth (no pre-bind hedging).
#[derive(Debug, Clone)]
pub struct Banner {
    /// The dux crate version (e.g. `0.1.0`).
    pub version: String,
    /// The mode line (e.g. `plain HTTP`, `TLS via Let's Encrypt`,
    /// `TLS via Let's Encrypt [STAGING]`).
    pub mode: String,
    /// One row per bound listener.
    pub listeners: Vec<ListenerRow>,
    /// The login-state row.
    pub login: LoginRow,
    /// ⚠ rows for degraded/best-effort legs (e.g. a busy Tailscale address).
    pub warnings: Vec<String>,
}

/// Render the banner to its lines. Pure so both color modes are unit-tested.
fn render_banner(color: bool, banner: &Banner) -> Vec<String> {
    let mut out = Vec::new();

    // Header: bold "dux" + version + the mode in the info tone.
    let header_name = if color {
        format!("{BOLD}{CYAN}dux{RESET}")
    } else {
        "dux".to_string()
    };
    let version = if color {
        format!("{DIM}v{}{RESET}", banner.version)
    } else {
        format!("v{}", banner.version)
    };
    out.push(format!("{header_name} {version}  {}", banner.mode));

    // One row per bound listener.
    for row in &banner.listeners {
        let arrow = if color {
            format!("{CYAN}{}{RESET}", Tone::Info.glyph())
        } else {
            "->".to_string()
        };
        let label = if color {
            format!("{BOLD}{}{RESET}", row.label)
        } else {
            row.label.clone()
        };
        let url = if color {
            format!("{CYAN}{}{RESET}", row.url)
        } else {
            row.url.clone()
        };
        let mut line = format!("  {arrow} {label}: {url}");
        if let Some(note) = &row.note {
            let note_text = if color {
                format!("{DIM}{note}{RESET}")
            } else {
                note.clone()
            };
            line.push_str(&format!(" {note_text}"));
        }
        out.push(line);
    }

    // Login row.
    let login_line = match &banner.login {
        LoginRow::Enabled { count } => {
            let text = format!("login enabled — {count} user(s)");
            if color {
                format!("  {GREEN}{}{RESET} {text}", Tone::Ok.glyph())
            } else {
                format!("  ok {text}")
            }
        }
        LoginRow::Disabled => {
            let text = "login DISABLED (--disable-auth) — anyone who reaches this server \
                        controls your agents";
            if color {
                format!("  {RED}{}{RESET} {text}", Tone::Error.glyph())
            } else {
                format!("  error {text}")
            }
        }
        LoginRow::NoLoginRequired { reachable, public } => {
            if !reachable {
                // Loopback-only: calm, muted. The gate is off but nothing off-host
                // can reach it, so this is the expected local-dev shape.
                let text = "No login required (local only) — add a login user to require sign-in";
                if color {
                    format!("  {DIM}{} {text}{RESET}", Tone::Info.glyph())
                } else {
                    format!("  info {text}")
                }
            } else {
                // A non-loopback leg is bound: warn. Public/LAN (required) gets the
                // stronger wording; a best-effort Tailscale leg gets its own.
                let text = if *public {
                    "No login required — this server is reachable on a public/LAN address with NO \
                     login. Add a login user or front it with an auth proxy."
                } else {
                    "No login required — anyone on your Tailscale network can connect. Add a login \
                     user to require sign-in."
                };
                if color {
                    format!("  {YELLOW}{}{RESET} {text}", Tone::Warn.glyph())
                } else {
                    format!("  warn {text}")
                }
            }
        }
    };
    out.push(login_line);

    // ⚠ degradation rows.
    for warning in &banner.warnings {
        let line = if color {
            format!("  {YELLOW}{}{RESET} {warning}", Tone::Warn.glyph())
        } else {
            format!("  warn {warning}")
        };
        out.push(line);
    }

    out
}

// ── Test-only shared buffer sink ─────────────────────────────────────────────

/// A read handle on a buffer-backed test console (see [`Console::test_capture`]).
/// Exposes the bytes the console wrote so a cross-module unit test can assert the
/// exact line shape. Reads drain the writer thread deterministically (no sleeps)
/// by sending a `Sync` barrier down the same channel and waiting for the thread
/// to acknowledge it, which guarantees every previously-sent line is on disk.
#[cfg(test)]
pub(crate) struct TestSink {
    buf: SharedBuffer,
    tx: SyncSender<WriterMsg>,
}

#[cfg(test)]
impl TestSink {
    /// Block until the writer thread has processed every line sent so far, so the
    /// buffer reflects all prior emits. Deterministic: the barrier rides the same
    /// FIFO channel as the lines, so its ack proves they were all written first.
    pub(crate) fn sync(&self) {
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel::<()>(0);
        // `send` (blocking) so a full channel does not lose the barrier; the
        // writer thread always drains, so this returns promptly.
        if self.tx.send(WriterMsg::Sync(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// The accumulated console output as a UTF-8 string, after draining the writer
    /// thread so every prior emit is reflected.
    pub(crate) fn contents(&self) -> String {
        self.sync();
        self.buf.contents()
    }
}

#[cfg(test)]
#[derive(Clone)]
struct SharedBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(test)]
impl SharedBuffer {
    fn new() -> Self {
        Self(Arc::new(std::sync::Mutex::new(Vec::new())))
    }

    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[cfg(test)]
impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // ── decide_color matrix ────────────────────────────────────────────────

    #[test]
    fn decide_color_always_is_on_regardless_of_env() {
        // `always` ignores the terminal, NO_COLOR, and TERM entirely.
        assert!(decide_color(&ColorInputs {
            setting: "always",
            stdout_is_terminal: false,
            no_color: Some("1"),
            term: Some("dumb"),
        }));
    }

    #[test]
    fn decide_color_never_is_off_regardless_of_env() {
        assert!(!decide_color(&ColorInputs {
            setting: "never",
            stdout_is_terminal: true,
            no_color: None,
            term: Some("xterm-256color"),
        }));
    }

    #[test]
    fn decide_color_auto_on_when_tty_and_clean_env() {
        assert!(decide_color(&ColorInputs {
            setting: "auto",
            stdout_is_terminal: true,
            no_color: None,
            term: Some("xterm-256color"),
        }));
        // An EMPTY NO_COLOR does not disable (per the spec, only a non-empty
        // value counts).
        assert!(decide_color(&ColorInputs {
            setting: "auto",
            stdout_is_terminal: true,
            no_color: Some(""),
            term: None,
        }));
    }

    #[test]
    fn decide_color_auto_off_when_piped() {
        assert!(!decide_color(&ColorInputs {
            setting: "auto",
            stdout_is_terminal: false,
            no_color: None,
            term: Some("xterm-256color"),
        }));
    }

    #[test]
    fn decide_color_auto_off_when_no_color_set() {
        assert!(!decide_color(&ColorInputs {
            setting: "auto",
            stdout_is_terminal: true,
            no_color: Some("1"),
            term: Some("xterm-256color"),
        }));
    }

    #[test]
    fn decide_color_auto_off_when_term_dumb() {
        assert!(!decide_color(&ColorInputs {
            setting: "auto",
            stdout_is_terminal: true,
            no_color: None,
            term: Some("dumb"),
        }));
    }

    #[test]
    fn decide_color_unknown_setting_behaves_like_auto() {
        // An unrecognized value is honored as auto: on for a clean tty.
        assert!(decide_color(&ColorInputs {
            setting: "rainbow",
            stdout_is_terminal: true,
            no_color: None,
            term: None,
        }));
        // ...and off when piped.
        assert!(!decide_color(&ColorInputs {
            setting: "rainbow",
            stdout_is_terminal: false,
            no_color: None,
            term: None,
        }));
    }

    #[test]
    fn is_known_color_setting_recognizes_the_three_values() {
        assert!(is_known_color_setting("auto"));
        assert!(is_known_color_setting("always"));
        assert!(is_known_color_setting("never"));
        assert!(!is_known_color_setting("rainbow"));
        assert!(!is_known_color_setting(""));
    }

    // ── format_line: color vs plain ────────────────────────────────────────

    #[test]
    fn format_line_color_carries_ansi_and_glyph() {
        let line = format_line(true, Tone::Info, "12:00:00", "hello");
        assert!(line.contains("\x1b["), "color mode must carry ANSI: {line}");
        assert!(
            line.contains(Tone::Info.glyph()),
            "must use the glyph: {line}"
        );
        assert!(line.contains("hello"));
        assert!(line.contains("12:00:00"));
    }

    #[test]
    fn format_line_plain_has_no_ansi_and_uses_word_glyph() {
        let line = format_line(false, Tone::Warn, "12:00:00", "careful");
        assert!(
            !line.contains('\x1b'),
            "plain mode must have no ANSI: {line}"
        );
        assert!(
            !line.contains(Tone::Warn.glyph()),
            "plain mode uses no Unicode glyph: {line}"
        );
        assert!(
            line.contains("warn"),
            "plain mode uses the word label: {line}"
        );
        assert!(line.contains("careful"));
        assert!(line.contains("12:00:00"));
    }

    #[test]
    fn each_tone_has_a_distinct_glyph_and_label() {
        let tones = [Tone::Info, Tone::Ok, Tone::Warn, Tone::Error];
        for t in tones {
            // color: glyph present, ANSI present
            let c = format_line(true, t, "00:00:00", "m");
            assert!(c.contains(t.glyph()));
            assert!(c.contains(t.color()));
            // plain: label present, no ANSI
            let p = format_line(false, t, "00:00:00", "m");
            assert!(p.contains(t.label()));
            assert!(!p.contains('\x1b'));
        }
    }

    // ── format_access_line ─────────────────────────────────────────────────

    #[test]
    fn access_line_plain_shape() {
        let line = format_access_line(false, "12:00:00", "GET", "/api/me", 200, 3);
        assert!(
            !line.contains('\x1b'),
            "plain access line has no ANSI: {line}"
        );
        assert!(line.contains("GET"));
        assert!(line.contains("/api/me"));
        assert!(line.contains("200"));
        assert!(line.contains("3ms"));
    }

    #[test]
    fn access_line_colors_status_by_class() {
        // 2xx green, 3xx cyan, 4xx yellow, 5xx red.
        assert!(format_access_line(true, "t", "GET", "/", 204, 1).contains(GREEN));
        assert!(format_access_line(true, "t", "GET", "/", 308, 1).contains(CYAN));
        assert!(format_access_line(true, "t", "GET", "/", 404, 1).contains(YELLOW));
        assert!(format_access_line(true, "t", "GET", "/", 500, 1).contains(RED));
    }

    #[test]
    fn access_line_preserves_query_string() {
        // Challenge tokens (and query strings generally) are printed as-is.
        let line = format_access_line(false, "t", "GET", "/x?a=1&b=2", 200, 1);
        assert!(
            line.contains("/x?a=1&b=2"),
            "query must be preserved: {line}"
        );
    }

    // ── banner rendering ───────────────────────────────────────────────────

    fn sample_banner() -> Banner {
        Banner {
            version: "0.1.0".to_string(),
            mode: "plain HTTP".to_string(),
            listeners: vec![ListenerRow {
                label: "Local".to_string(),
                url: "http://127.0.0.1:8080".to_string(),
                note: None,
            }],
            login: LoginRow::Enabled { count: 1 },
            warnings: vec![],
        }
    }

    #[test]
    fn banner_plain_mode_shape() {
        let lines = render_banner(false, &sample_banner());
        let joined = lines.join("\n");
        assert!(
            !joined.contains('\x1b'),
            "plain banner has no ANSI: {joined}"
        );
        assert!(joined.contains("dux v0.1.0"));
        assert!(joined.contains("plain HTTP"));
        assert!(joined.contains("Local: http://127.0.0.1:8080"));
        assert!(joined.contains("login enabled — 1 user(s)"));
    }

    #[test]
    fn banner_color_mode_has_ansi() {
        let lines = render_banner(true, &sample_banner());
        let joined = lines.join("\n");
        assert!(
            joined.contains("\x1b["),
            "color banner carries ANSI: {joined}"
        );
        assert!(joined.contains("0.1.0"));
    }

    #[test]
    fn banner_disabled_auth_is_a_red_warning() {
        let mut b = sample_banner();
        b.login = LoginRow::Disabled;
        let plain = render_banner(false, &b).join("\n");
        assert!(
            plain.contains("login DISABLED"),
            "must warn loudly: {plain}"
        );
        assert!(plain.contains("--disable-auth"));
        let color = render_banner(true, &b).join("\n");
        assert!(
            color.contains(RED),
            "disabled-auth row must be red in color mode"
        );
    }

    #[test]
    fn banner_no_login_loopback_is_muted_local_only() {
        let mut b = sample_banner();
        b.login = LoginRow::NoLoginRequired {
            reachable: false,
            public: false,
        };
        let plain = render_banner(false, &b).join("\n");
        assert!(
            plain.contains("No login required (local only)"),
            "loopback-only must say local only: {plain}"
        );
        // Never the old lie about an enabled gate or the loud disabled warning.
        assert!(!plain.contains("login enabled"));
        assert!(!plain.contains("DISABLED"));
        let color = render_banner(true, &b).join("\n");
        assert!(
            color.contains(DIM) && !color.contains(YELLOW) && !color.contains(RED),
            "local-only row is muted (DIM), not a warning: {color}"
        );
    }

    #[test]
    fn banner_no_login_tailscale_is_yellow_warning() {
        let mut b = sample_banner();
        b.login = LoginRow::NoLoginRequired {
            reachable: true,
            public: false,
        };
        let plain = render_banner(false, &b).join("\n");
        assert!(
            plain.contains("Tailscale network"),
            "tailscale wording: {plain}"
        );
        assert!(plain.starts_with("dux") || plain.contains("warn"));
        let color = render_banner(true, &b).join("\n");
        assert!(
            color.contains(YELLOW),
            "reachable no-login row is a yellow warning: {color}"
        );
    }

    #[test]
    fn banner_no_login_public_is_stronger_yellow_warning() {
        let mut b = sample_banner();
        b.login = LoginRow::NoLoginRequired {
            reachable: true,
            public: true,
        };
        let plain = render_banner(false, &b).join("\n");
        assert!(
            plain.contains("public/LAN address") && plain.contains("auth proxy"),
            "public wording must name the public/LAN reach and the proxy mitigation: {plain}"
        );
        let color = render_banner(true, &b).join("\n");
        assert!(
            color.contains(YELLOW),
            "public no-login row is a yellow warning: {color}"
        );
    }

    #[test]
    fn banner_acme_staging_mode_and_redirect_note() {
        let b = Banner {
            version: "0.1.0".to_string(),
            mode: "TLS via Let's Encrypt [STAGING]".to_string(),
            listeners: vec![ListenerRow {
                label: "dux.example.com".to_string(),
                url: "https://dux.example.com/".to_string(),
                note: Some("(plain HTTP on :80 redirects here)".to_string()),
            }],
            login: LoginRow::Enabled { count: 2 },
            warnings: vec![],
        };
        let plain = render_banner(false, &b).join("\n");
        assert!(plain.contains("[STAGING]"));
        assert!(plain.contains("https://dux.example.com/"));
        assert!(plain.contains("redirects here"));
        assert!(plain.contains("login enabled — 2 user(s)"));
    }

    #[test]
    fn banner_degraded_rows_render_as_warnings() {
        let mut b = sample_banner();
        b.warnings = vec!["Tailscale: 100.64.0.1:8080 busy — serving without it".to_string()];
        let plain = render_banner(false, &b).join("\n");
        assert!(plain.contains("warn Tailscale: 100.64.0.1:8080 busy"));
        let color = render_banner(true, &b).join("\n");
        assert!(
            color.contains(YELLOW),
            "a degraded row must be yellow in color mode"
        );
        assert!(color.contains(Tone::Warn.glyph()));
    }

    // ── Console emit through the writer seam ────────────────────────────────

    #[test]
    fn noop_console_writes_nothing_and_is_inactive() {
        let console = Console::noop();
        assert!(!console.is_active(), "a noop console must report inactive");
        // Every emit is a no-op — there is no observable output, and these calls
        // must not panic.
        console.client_connected(ip("10.0.0.1"));
        console.login_failed(ip("10.0.0.1"));
        console.access("GET", "/", 200, 1);
        console.banner(&sample_banner());
    }

    #[test]
    fn console_emits_event_lines_to_the_buffer() {
        let (console, sink) = Console::test_capture(false);
        console.client_connected(ip("10.0.0.1"));
        console.client_disconnected(ip("10.0.0.1"));
        console.login_ok("alice", ip("10.0.0.2"));
        console.login_failed(ip("10.0.0.3"));
        console.login_rate_limited(ip("10.0.0.4"));
        console.logout("alice");
        let out = sink.contents();
        assert!(out.contains("client connected from 10.0.0.1"));
        assert!(out.contains("client disconnected from 10.0.0.1"));
        assert!(out.contains("login ok for \"alice\" from 10.0.0.2"));
        assert!(out.contains("login failed from 10.0.0.3"));
        // The failure line names the IP but NEVER a username.
        let failed_line = out
            .lines()
            .find(|l| l.contains("login failed"))
            .expect("a failure line");
        assert!(
            !failed_line.contains("alice"),
            "failure must not log a username: {failed_line}"
        );
        assert!(out.contains("login rate-limited from 10.0.0.4"));
        assert!(out.contains("logout for \"alice\""));
    }

    #[test]
    fn console_access_line_goes_to_the_buffer() {
        let (console, sink) = Console::test_capture(false);
        console.access("POST", "/api/login", 401, 250);
        let out = sink.contents();
        assert!(out.contains("POST"));
        assert!(out.contains("/api/login"));
        assert!(out.contains("401"));
        assert!(out.contains("250ms"));
    }

    #[test]
    fn console_banner_writes_every_line() {
        let (console, sink) = Console::test_capture(false);
        console.banner(&sample_banner());
        let out = sink.contents();
        assert!(out.contains("dux v0.1.0"));
        assert!(out.contains("Local: http://127.0.0.1:8080"));
        assert!(out.contains("login enabled — 1 user(s)"));
    }

    #[test]
    fn stdout_console_is_active() {
        // The production constructor reports active so the access middleware and
        // banner actually emit.
        assert!(Console::stdout(false).is_active());
    }

    // ── Writer-thread: ordering + drop-on-full ──────────────────────────────

    #[test]
    fn writer_thread_preserves_order_across_concurrent_senders() {
        // Many threads share one cheap-clone Console and each emits a batch of
        // tagged lines. The writer thread serializes them, so within any single
        // sender the lines must appear in send order (no mid-line interleave, no
        // reordering of a sender's own stream).
        let (console, sink) = Console::test_capture(false);
        const SENDERS: usize = 8;
        const PER_SENDER: usize = 50;
        let mut handles = Vec::new();
        for s in 0..SENDERS {
            let c = console.clone();
            handles.push(std::thread::spawn(move || {
                for n in 0..PER_SENDER {
                    c.client_connected(ip(&format!("10.0.{s}.{n}")));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let out = sink.contents();
        // Every sender's lines are present and in order.
        for s in 0..SENDERS {
            let mut last = -1i64;
            for line in out.lines().filter(|l| l.contains(&format!("10.0.{s}."))) {
                // Extract the trailing octet (the per-sender sequence number).
                let n: i64 = line
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .trim()
                    .parse()
                    .expect("a numeric trailing octet");
                assert!(
                    n > last,
                    "sender {s}'s lines must stay in send order: {n} after {last}"
                );
                last = n;
            }
            assert_eq!(
                last,
                (PER_SENDER - 1) as i64,
                "sender {s} must have emitted all {PER_SENDER} lines"
            );
        }
    }

    /// A writer whose every `write` first SIGNALS that it was entered (proving
    /// the writer thread dequeued a line off the bounded channel), then BLOCKS
    /// until a token is released on the gate. The entered signal is what makes
    /// the drop test deterministic: the test waits for it before filling the
    /// channel, so the fill can never race the dequeue. Bytes land in the shared
    /// buffer once a write is allowed to proceed.
    #[cfg(test)]
    struct GatedWriter {
        buf: SharedBuffer,
        gate: std::sync::mpsc::Receiver<()>,
        entered: std::sync::mpsc::Sender<()>,
    }

    #[cfg(test)]
    impl Write for GatedWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            // Announce the dequeue (unbounded send, never blocks), then block
            // until the test releases one token. A closed gate (test done) lets
            // the write proceed so the thread can drain and exit.
            let _ = self.entered.send(());
            let _ = self.gate.recv();
            self.buf.write(data)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_thread_drops_on_full_and_warns_with_count() {
        // bound = 2 + a writer wedged on its first write. The writer thread pulls
        // line 1 and blocks inside `write`; the channel (cap 2) then absorbs lines
        // 2 and 3; every line after that is DROPPED and counted. When pressure is
        // relieved, the next successful send emits ONE warning naming the count.
        let buf = SharedBuffer::new();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let writer = GatedWriter {
            buf: buf.clone(),
            gate: release_rx,
            entered: entered_tx,
        };
        let console = Console::test_capture_bounded(false, 2, Box::new(writer));

        // Line 1: the writer thread dequeues it and blocks in `write` (gate empty).
        console.access("GET", "/1", 200, 1);
        // BARRIER: wait until the writer thread has provably taken line 1 off the
        // channel and is wedged inside `write` (it signals `entered` before
        // blocking on the gate). Only then is the cap-2 channel empty, so the
        // next two sends deterministically fill it rather than racing the
        // dequeue.
        entered_rx
            .recv()
            .expect("the writer thread must enter write(/1)");
        // Fill the channel: lines 2 and 3 occupy the 2 buffered slots.
        console.access("GET", "/2", 200, 1);
        console.access("GET", "/3", 200, 1);
        // The channel is now full (thread wedged on line 1, slots hold 2 and 3).
        // These MUST be dropped (try_send → Full), incrementing the counter.
        for n in 4..=6 {
            console.access("GET", &format!("/{n}"), 200, 1);
        }
        assert_eq!(
            console.dropped_count(),
            3,
            "exactly lines 4-6 are dropped once the fill cannot race the dequeue"
        );

        let dropped_before = console.dropped_count();

        // Relieve the pressure: release plenty of tokens for every queued + future
        // write, then DRAIN the channel deterministically with a blocking Sync
        // barrier. After its ack the channel is empty and the writer thread is
        // parked on `recv` (not wedged on a write), so the next send below has room.
        for _ in 0..32 {
            let _ = release_tx.send(());
        }
        let tx = console.writer_tx();
        let sync = |tx: &SyncSender<WriterMsg>| {
            let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel::<()>(0);
            tx.send(WriterMsg::Sync(ack_tx)).unwrap();
            ack_rx.recv().unwrap();
        };
        sync(&tx);

        // The channel is now empty; this fresh send sees the outstanding drop
        // streak and a non-full channel, so it rides the single warning ahead of
        // itself and resets the counter.
        console.access("GET", "/after", 200, 1);
        sync(&tx);

        let out = buf.contents();
        assert!(
            out.contains("console output fell behind") && out.contains("line(s) dropped"),
            "a drop streak must surface a single warning: {out}"
        );
        // The warning names the EXACT number of lines that were dropped.
        assert!(
            out.contains(&format!("{dropped_before} line(s) dropped")),
            "the warning must name the dropped count ({dropped_before}): {out}"
        );
        assert!(
            out.contains("slow stdout consumer"),
            "the warning names the cause: {out}"
        );
        // Exactly ONE warning is emitted for the streak (not one per dropped line).
        assert_eq!(
            out.matches("console output fell behind").count(),
            1,
            "a drop streak produces a single coalesced warning: {out}"
        );
        // After the warning is emitted, the streak resets to zero.
        assert_eq!(
            console.dropped_count(),
            0,
            "the drop counter resets once the warning is emitted"
        );
        assert!(
            out.contains("/after"),
            "the post-pressure line is written: {out}"
        );
    }
}
