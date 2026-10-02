//! The web server's console: the timestamped log of the server's life, the one
//! producer behind both places it is shown.
//!
//! Every line is a [`dux_core::serve_log::LogLine`], built here once. Where it
//! goes depends on how the console was made:
//!
//! - [`Console::stdout`] is `dux server`'s terminal: a writer thread prints each
//!   line, colored or plain per [`detect`].
//! - [`Console::capture`] is the `start-web-server` flip: it writes nothing to
//!   the terminal (the flip's status screen owns it) and records each line into
//!   the `ActivityRing` that screen's log viewer draws.
//! - [`Console::noop`] does neither.
//!
//! The two real sinks receive the same lines, so the flip shows exactly what
//! `dux server` prints, access log included (the user decided on 2026-10-02 that
//! the two logs match completely; the flip's viewer used to leave the access
//! log out).
//!
//! The console is additive to `dux.log`, which keeps logging every lifecycle
//! event it already logged. The access log is the one console-only line.
//!
//! Color is hand-rolled minimal ANSI in [`dux_core::serve_log`]. [`detect`]
//! decides from the `[server] color` setting plus `IsTerminal`, `NO_COLOR` and
//! `TERM`; with color off a line prints its plain ASCII spelling.

use std::io::{IsTerminal, Write};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use dux_core::activity::ActivityRing;
use dux_core::serve_log::{Banner, LogLine, LogTone};

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

/// The warning for an unrecognized `[server] color` value.
pub fn unknown_color_warning(setting: &str) -> String {
    format!("[server] color = \"{setting}\" is not auto/always/never. Using \"auto\".")
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

/// The bound on the writer channel. Emitters `try_send`, so a stalled stdout
/// consumer drops lines rather than blocking the emitting tokio worker; sized for
/// a momentary stall at a fixed, bounded cost.
const WRITER_CHANNEL_BOUND: usize = 1024;

/// A message handed to the writer thread.
enum WriterMsg {
    /// One already-formatted line to write + flush.
    Line(String),
    /// A barrier: the writer thread sends `()` back once it has processed every
    /// message queued before this one. [`Console::flush`] waits on it so the
    /// last lines of a run (its shutdown) reach the terminal before the process
    /// exits.
    Sync(SyncSender<()>),
}

/// Where the console prints. Production hands lines to a dedicated writer THREAD
/// over a bounded channel so a stalled stdout consumer can never park an emitting
/// tokio worker on a blocking `write()`; tests inject an in-memory buffer.
enum Sink {
    /// Nothing printed: the flip and any disabled console.
    Noop,
    /// The bounded sender to the writer thread. A full channel means a slow
    /// consumer, so the line is dropped (accounted on `dropped`) instead of
    /// blocking the runtime. The writer thread owns the actual writer.
    Writer {
        tx: SyncSender<WriterMsg>,
        /// Lines dropped while the channel was full, awaiting a one-line warning
        /// the next successful send emits. Relaxed: it only gates a human-facing
        /// warning, so exact cross-thread ordering does not matter.
        dropped: AtomicU64,
    },
}

/// Where a line's timestamp comes from: the wall clock in production, a fixed
/// value in tests so two consoles can be compared line for line.
type Clock = fn() -> String;

/// The shared console handle. Cheap to clone (`Arc`).
///
/// ## Shutdown
///
/// The writer thread lives as long as any `Console` clone does: when the LAST
/// one drops, the channel closes and the thread exits on its own. It is a plain
/// `std::thread`, so a hard process exit simply tears it down; a caller that
/// must not lose queued lines calls [`Console::flush`] first.
#[derive(Clone)]
pub struct Console(Arc<ConsoleInner>);

struct ConsoleInner {
    color: bool,
    sink: Sink,
    /// The flip's log viewer buffer. Every line is pushed here as well, and
    /// connect/disconnect move its connection counter. `None` for every console
    /// but the flip's.
    capture: Option<ActivityRing>,
    clock: Clock,
}

impl Console {
    /// A real console writing to stdout. `color` comes from [`detect`].
    pub fn stdout(color: bool) -> Self {
        Self::with_writer(
            color,
            Box::new(std::io::stdout()),
            WRITER_CHANNEL_BOUND,
            now_hms,
        )
    }

    /// Build a writer-backed console over an owned writer: spawn the dedicated
    /// writer thread (owns the writer; writes + flushes per line; exits when the
    /// channel closes) and keep only the bounded sender + the drop counter.
    fn with_writer(color: bool, writer: Box<dyn Write + Send>, bound: usize, clock: Clock) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WriterMsg>(bound);
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
            capture: None,
            clock,
        }))
    }

    /// A no-op console: nothing printed, nothing captured.
    pub fn noop() -> Self {
        Self(Arc::new(ConsoleInner {
            color: false,
            sink: Sink::Noop,
            capture: None,
            clock: now_hms,
        }))
    }

    /// The flip's console: prints nothing (the status screen owns the terminal)
    /// and records every line into `ring`, the log viewer's buffer.
    pub fn capture(ring: ActivityRing) -> Self {
        Self::capture_with_clock(ring, now_hms)
    }

    fn capture_with_clock(ring: ActivityRing, clock: Clock) -> Self {
        Self(Arc::new(ConsoleInner {
            color: false,
            sink: Sink::Noop,
            capture: Some(ring),
            clock,
        }))
    }

    /// A buffer-backed console stamped at a fixed time, plus a handle to read
    /// what it printed. Test-only.
    #[cfg(test)]
    pub(crate) fn test_capture(color: bool) -> (Self, TestSink) {
        let buf = SharedBuffer::new();
        let console = Self::with_writer(
            color,
            Box::new(buf.clone()),
            WRITER_CHANNEL_BOUND,
            fixed_test_clock,
        );
        let tx = console.writer_tx();
        (console, TestSink { buf, tx })
    }

    /// The flip's console stamped at the same fixed time as
    /// [`Self::test_capture`], so the two can be compared line for line.
    #[cfg(test)]
    pub(crate) fn test_ring_capture(ring: ActivityRing) -> Self {
        Self::capture_with_clock(ring, fixed_test_clock)
    }

    /// A writer-backed console with a custom channel bound and writer.
    #[cfg(test)]
    fn test_capture_bounded(color: bool, bound: usize, writer: Box<dyn Write + Send>) -> Self {
        Self::with_writer(color, writer, bound, fixed_test_clock)
    }

    #[cfg(test)]
    fn writer_tx(&self) -> SyncSender<WriterMsg> {
        match &self.0.sink {
            Sink::Writer { tx, .. } => tx.clone(),
            Sink::Noop => unreachable!("a writer console always has a sender"),
        }
    }

    #[cfg(test)]
    fn dropped_count(&self) -> u64 {
        match &self.0.sink {
            Sink::Writer { dropped, .. } => dropped.load(Ordering::Relaxed),
            Sink::Noop => 0,
        }
    }

    /// Whether this console prints to a terminal.
    pub fn is_active(&self) -> bool {
        !matches!(self.0.sink, Sink::Noop)
    }

    /// Whether a line handed to this console goes anywhere at all (printed or
    /// captured). The access-log middleware checks this before doing any work.
    pub fn is_recording(&self) -> bool {
        self.is_active() || self.0.capture.is_some()
    }

    /// Block until every line handed over so far has been written. A no-op for a
    /// console that prints nothing. Called before the process exits so the
    /// last lines of a run are not lost in the writer's queue.
    pub fn flush(&self) {
        let Sink::Writer { tx, .. } = &self.0.sink else {
            return;
        };
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel::<()>(0);
        if tx.send(WriterMsg::Sync(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// Hand one formatted line to the writer thread. On a full channel the line
    /// is dropped and counted; the next successful send emits one warning first,
    /// so the gap is visible.
    fn write_line(&self, line: String) {
        let Sink::Writer { tx, dropped } = &self.0.sink else {
            return;
        };
        let drop_streak = dropped.load(Ordering::Relaxed);
        if drop_streak > 0 {
            let warn = LogLine::event(
                &(self.0.clock)(),
                LogTone::Warn,
                &format!(
                    "console output fell behind: {} dropped (slow stdout consumer)",
                    dux_core::text::count_of(
                        usize::try_from(drop_streak).unwrap_or(usize::MAX),
                        "line"
                    )
                ),
            )
            .render(self.0.color);
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

    /// Send one line to every sink this console has. The one place a line
    /// leaves the console, which is what keeps the two surfaces identical.
    fn line(&self, line: LogLine) {
        if self.is_active() {
            self.write_line(line.render(self.0.color));
        }
        if let Some(ring) = &self.0.capture {
            ring.push(line);
        }
    }

    /// A timestamped event line. Returns early, before any formatting, on a
    /// console that records nothing.
    fn emit(&self, tone: LogTone, message: &str) {
        if !self.is_recording() {
            return;
        }
        self.line(LogLine::event(&(self.0.clock)(), tone, message));
    }

    // ── The public emit surface ────────────────────────────────────────────

    /// An informational line (the shutdown progress, for one).
    pub fn info(&self, message: &str) {
        self.emit(LogTone::Info, message);
    }

    /// A warning line (a startup warning, for one).
    pub fn warn(&self, message: &str) {
        self.emit(LogTone::Warn, message);
    }

    /// An error line.
    pub fn error(&self, message: &str) {
        self.emit(LogTone::Error, message);
    }

    /// The post-bind startup banner: a header, one row per bound listener, then
    /// the warning rows and the reachability note.
    pub fn banner(&self, banner: &Banner) {
        if !self.is_recording() {
            return;
        }
        for line in banner.lines() {
            self.line(line);
        }
    }

    pub fn client_connected(&self, ip: IpAddr) {
        if let Some(ring) = &self.0.capture {
            ring.connection_opened();
        }
        self.emit(LogTone::Info, &format!("client connected from {ip}"));
    }

    pub fn client_disconnected(&self, ip: IpAddr) {
        if let Some(ring) = &self.0.capture {
            ring.connection_closed();
        }
        self.emit(LogTone::Info, &format!("client disconnected from {ip}"));
    }

    /// A best-effort listener bind that degraded (e.g. a busy Tailscale leg).
    pub fn bind_degraded(&self, message: &str) {
        self.emit(LogTone::Warn, message);
    }

    /// A listener the server ADDED or DROPPED while running: the Tailscale leg
    /// following its interface. Deliberately not [`Self::bind_degraded`]: a leg
    /// arriving is good news, and a leg leaving on `auto` is expected news, so
    /// neither belongs under a ⚠ that means something went wrong.
    pub fn leg_changed(&self, message: &str) {
        self.emit(LogTone::Info, message);
    }

    /// One access-log line. Gated by the caller on the `access_log` setting.
    /// Returns early, before any formatting, on a console that records nothing.
    pub fn access(&self, method: &str, path: &str, status: u16, latency_ms: u128) {
        if !self.is_recording() {
            return;
        }
        self.line(LogLine::access(
            &(self.0.clock)(),
            method,
            path,
            status,
            latency_ms,
        ));
    }
}

/// The dedicated writer thread body: own the writer, drain the channel, write +
/// flush each line, acknowledge each barrier once everything before it has been
/// written. The loop ends, and the thread exits, when the channel closes.
fn writer_loop(mut writer: Box<dyn Write + Send>, rx: std::sync::mpsc::Receiver<WriterMsg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            WriterMsg::Line(line) => {
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
            WriterMsg::Sync(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

/// Current wall-clock time as `HH:MM:SS`. Wall-clock, not a tick counter, per the
/// project's animation/refresh tenet.
fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
fn fixed_test_clock() -> String {
    "12:00:00".to_string()
}

// ── Test-only shared buffer sink ─────────────────────────────────────────────

/// A read handle on a buffer-backed test console (see [`Console::test_capture`]).
#[cfg(test)]
pub(crate) struct TestSink {
    buf: SharedBuffer,
    tx: SyncSender<WriterMsg>,
}

#[cfg(test)]
impl TestSink {
    /// Block until the writer thread has processed every line sent so far.
    pub(crate) fn sync(&self) {
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel::<()>(0);
        if self.tx.send(WriterMsg::Sync(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// The bytes written so far, WITHOUT draining the writer thread first.
    pub(crate) fn raw_contents(&self) -> String {
        self.buf.contents()
    }

    /// The accumulated console output, after draining the writer thread.
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

/// Strip ANSI escapes, for tests comparing a colored console with the viewer.
#[cfg(test)]
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dux_core::serve_log::ListenerRow;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn ring_texts(ring: &ActivityRing) -> Vec<String> {
        ring.snapshot().lines.iter().map(LogLine::text).collect()
    }

    // ── decide_color matrix ────────────────────────────────────────────────

    #[test]
    fn decide_color_always_is_on_regardless_of_env() {
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
        assert!(decide_color(&ColorInputs {
            setting: "rainbow",
            stdout_is_terminal: true,
            no_color: None,
            term: None,
        }));
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

    // ── Banner + events through the writer ─────────────────────────────────

    fn sample_banner() -> Banner {
        Banner {
            version: "v0.1.0".to_string(),
            mode: "plain HTTP".to_string(),
            listeners: vec![ListenerRow {
                label: "Local".to_string(),
                url: "http://127.0.0.1:8080".to_string(),
            }],
            warnings: vec![],
            security_note: None,
        }
    }

    #[test]
    fn noop_console_records_nothing_and_is_inactive() {
        let console = Console::noop();
        assert!(!console.is_active());
        assert!(!console.is_recording());
        console.client_connected(ip("10.0.0.1"));
        console.client_disconnected(ip("10.0.0.1"));
        console.access("GET", "/", 200, 1);
        console.banner(&sample_banner());
        console.flush();
    }

    #[test]
    fn console_emits_event_lines_to_the_buffer() {
        let (console, sink) = Console::test_capture(false);
        console.client_connected(ip("10.0.0.1"));
        console.client_disconnected(ip("10.0.0.1"));
        let out = sink.contents();
        assert!(out.contains("12:00:00 info client connected from 10.0.0.1"));
        assert!(out.contains("client disconnected from 10.0.0.1"));
    }

    #[test]
    fn console_access_line_goes_to_the_buffer() {
        let (console, sink) = Console::test_capture(false);
        console.access("POST", "/api/login", 401, 250);
        assert_eq!(sink.contents(), "12:00:00 POST /api/login 401 250ms\n");
    }

    #[test]
    fn console_banner_writes_every_line() {
        let (console, sink) = Console::test_capture(false);
        console.banner(&sample_banner());
        assert_eq!(
            sink.contents(),
            "dux v0.1.0  plain HTTP\n  -> Local: http://127.0.0.1:8080\n"
        );
    }

    #[test]
    fn stdout_console_is_active() {
        assert!(Console::stdout(false).is_active());
    }

    // ── The flip's capture console ─────────────────────────────────────────

    #[test]
    fn capture_console_records_the_banner_events_and_access_lines() {
        let ring = ActivityRing::new(100);
        let console = Console::test_ring_capture(ring.clone());
        console.banner(&sample_banner());
        console.client_connected(ip("10.0.0.1"));
        console.access("GET", "/api/v1/build", 200, 3);
        console.warn("careful");
        assert_eq!(
            ring_texts(&ring),
            vec![
                "dux v0.1.0  plain HTTP".to_string(),
                "  \u{279c} Local: http://127.0.0.1:8080".to_string(),
                "12:00:00 \u{279c} client connected from 10.0.0.1".to_string(),
                "12:00:00 GET /api/v1/build 200 3ms".to_string(),
                "12:00:00 \u{26a0} careful".to_string(),
            ]
        );
    }

    #[test]
    fn capture_console_prints_nothing_but_records() {
        let ring = ActivityRing::new(10);
        let console = Console::capture(ring);
        assert!(!console.is_active(), "the flip's console never prints");
        assert!(console.is_recording(), "but it records every line");
    }

    #[test]
    fn capture_console_tracks_active_connection_count() {
        let ring = ActivityRing::new(10);
        let console = Console::capture(ring.clone());
        console.client_connected(ip("10.0.0.1"));
        console.client_connected(ip("10.0.0.2"));
        assert_eq!(ring.connections(), 2);
        console.client_disconnected(ip("10.0.0.1"));
        assert_eq!(ring.connections(), 1);
        assert_eq!(ring.snapshot().lines.len(), 3);
    }

    #[test]
    fn a_colored_stdout_console_and_the_capture_record_the_same_text() {
        let (stdout, sink) = Console::test_capture(true);
        let ring = ActivityRing::new(100);
        let capture = Console::test_ring_capture(ring.clone());
        for console in [&stdout, &capture] {
            console.banner(&sample_banner());
            console.client_connected(ip("10.0.0.1"));
            console.access("GET", "/", 404, 1);
            console.error("broken");
        }
        let printed: Vec<String> = sink.contents().lines().map(strip_ansi).collect();
        assert_eq!(printed, ring_texts(&ring));
    }

    #[test]
    fn flush_waits_for_every_queued_line() {
        let (console, sink) = Console::test_capture(false);
        for n in 0..50 {
            console.info(&format!("line {n}"));
        }
        console.flush();
        assert!(sink.buf.contents().contains("line 49"));
    }

    // ── Writer-thread: ordering + drop-on-full ──────────────────────────────

    #[test]
    fn writer_thread_preserves_order_across_concurrent_senders() {
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
        for s in 0..SENDERS {
            let mut last = -1i64;
            for line in out.lines().filter(|l| l.contains(&format!("10.0.{s}."))) {
                let n: i64 = line
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .trim()
                    .parse()
                    .expect("a numeric trailing octet");
                assert!(n > last, "sender {s}'s lines must stay in send order");
                last = n;
            }
            assert_eq!(last, (PER_SENDER - 1) as i64);
        }
    }

    /// A writer whose every `write` first signals that it was entered, then
    /// blocks until a token is released on the gate. See the drop test.
    struct GatedWriter {
        buf: SharedBuffer,
        gate: std::sync::mpsc::Receiver<()>,
        entered: std::sync::mpsc::Sender<()>,
    }

    impl Write for GatedWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
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
        // bound = 2 + a writer wedged on its first write: lines 2 and 3 fill the
        // channel, 4-6 are dropped, and the next send after relief emits ONE
        // warning naming the count.
        let buf = SharedBuffer::new();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let writer = GatedWriter {
            buf: buf.clone(),
            gate: release_rx,
            entered: entered_tx,
        };
        let console = Console::test_capture_bounded(false, 2, Box::new(writer));

        console.access("GET", "/1", 200, 1);
        entered_rx
            .recv()
            .expect("the writer thread must enter write(/1)");
        console.access("GET", "/2", 200, 1);
        console.access("GET", "/3", 200, 1);
        for n in 4..=6 {
            console.access("GET", &format!("/{n}"), 200, 1);
        }
        assert_eq!(console.dropped_count(), 3);
        let dropped_before = console.dropped_count();

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
        console.access("GET", "/after", 200, 1);
        sync(&tx);

        let out = buf.contents();
        assert!(out.contains(&format!(
            "console output fell behind: {dropped_before} lines dropped (slow stdout consumer)"
        )));
        assert_eq!(out.matches("console output fell behind").count(), 1);
        assert_eq!(console.dropped_count(), 0);
        assert!(out.contains("/after"));
    }
}
