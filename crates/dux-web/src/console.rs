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
//! Any of them can also carry a [`ServerLog`], the `server.log` file: every
//! line the console produces is written there too, in the plain spelling
//! `dux server` prints with color off. [`Console::server_log_only`] is the
//! background server's console, which has nothing to print over the terminal
//! UI's frame but still keeps the file.
//!
//! The two real sinks receive the same lines, so the flip shows exactly what
//! `dux server` prints, access log included (the user decided on 2026-10-02 that
//! the two logs match completely; the flip's viewer used to leave the access
//! log out).
//!
//! The console is additive to `dux.log`, which keeps logging every lifecycle
//! event it already logged. The access log is the one line `dux.log` never
//! gets; it lives in the console and `server.log`.
//!
//! Color is hand-rolled minimal ANSI in [`dux_core::serve_log`]. [`detect`]
//! decides from the `[server] color` setting plus `IsTerminal`, `NO_COLOR` and
//! `TERM`; with color off a line prints its plain ASCII spelling.

use std::io::{IsTerminal, Write};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use dux_core::activity::ActivityRing;
use dux_core::logger::ServerLog;
use dux_core::serve_log::{Banner, ListenerRow, LogLine, LogTone};

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
    use dux_core::config_effective::{ServerColor, effective_server_color};
    match effective_server_color(inputs.setting) {
        ServerColor::Always => true,
        ServerColor::Never => false,
        ServerColor::Auto => {
            let no_color_active = inputs.no_color.is_some_and(|v| !v.is_empty());
            let term_dumb = inputs.term == Some("dumb");
            inputs.stdout_is_terminal && !no_color_active && !term_dumb
        }
    }
}

/// Whether `setting` is a recognized `[server] color` value. An unrecognized
/// value is honored as `auto` but the caller warns so a typo is visible.
pub fn is_known_color_setting(setting: &str) -> bool {
    dux_core::config_effective::ServerColor::parse(setting).is_some()
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

/// How long [`Console::flush`] waits for the writer before giving up. A writer
/// blocked on a pipe nobody reads (`dux server | less`, stopped) would otherwise
/// hold the exit, the force-exit hatch included, for as long as it stays blocked.
/// A safety bound on an exit path rather than a preference, so not a setting.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// The bound on the writer channel. Emitters `try_send`, so a stalled stdout
/// consumer drops lines rather than blocking the emitting tokio worker; sized for
/// a momentary stall at a fixed, bounded cost.
const WRITER_CHANNEL_BOUND: usize = 1024;

/// The bound on the stderr copy's queue. Warnings are rare, so a short queue
/// that fills means stderr is stuck, and then the copy is dropped and counted
/// rather than waited for.
const ECHO_CHANNEL_BOUND: usize = 64;

/// A message handed to a writer thread.
enum WriterMsg {
    /// One already-formatted line to write + flush.
    Line(String),
    /// A barrier: the writer thread sends `()` back once it has processed every
    /// message queued before this one. [`Console::flush`] waits on it so the
    /// last lines of a run (its shutdown) reach their stream before the process
    /// exits.
    Sync(SyncSender<()>),
}

/// One output stream behind its own writer THREAD and bounded queue, so a
/// stalled consumer can never park the thread that emitted a line on a blocking
/// `write()`: a full queue drops the line and counts it instead, and the next
/// line that fits is preceded by one note saying how many were lost. stdout has
/// one; the stderr copy of warnings has its own, so neither can hold up the
/// other or the emitter.
struct LineWriter {
    tx: SyncSender<WriterMsg>,
    /// Lines dropped while the queue was full, awaiting the note. Relaxed: it
    /// only gates a human-facing note, so exact ordering does not matter.
    dropped: AtomicU64,
    /// Whether this stream's lines carry color.
    color: bool,
    /// The stream, for its drop note.
    name: &'static str,
}

impl LineWriter {
    fn spawn(writer: Box<dyn Write + Send>, bound: usize, color: bool, name: &'static str) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<WriterMsg>(bound);
        std::thread::Builder::new()
            .name(format!("dux-console-{name}"))
            .spawn(move || writer_loop(writer, rx))
            .expect("spawn console writer thread");
        Self {
            tx,
            dropped: AtomicU64::new(0),
            color,
            name,
        }
    }

    /// Queue `line` without blocking. After a drop streak, a note naming the
    /// count goes first, once.
    fn send(&self, line: &LogLine, clock: Clock) {
        let drop_streak = self.dropped.load(Ordering::Relaxed);
        if drop_streak > 0 {
            let note = LogLine::event(
                &clock(),
                LogTone::Warn,
                &format!(
                    "console output fell behind: {} dropped (slow {} consumer)",
                    dux_core::text::count_of(
                        usize::try_from(drop_streak).unwrap_or(usize::MAX),
                        "line"
                    ),
                    self.name
                ),
            );
            // Clear the streak only once the note itself got through, so a
            // still-full queue keeps counting rather than losing the notice.
            if self
                .tx
                .try_send(WriterMsg::Line(note.render(self.color)))
                .is_ok()
            {
                self.dropped.fetch_sub(drop_streak, Ordering::Relaxed);
            }
        }
        match self.tx.try_send(WriterMsg::Line(line.render(self.color))) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            // The writer thread is gone (only on process teardown).
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Wait, until `deadline` at the latest, for everything queued so far to be
    /// written. The barrier waits for room in a full queue (a slow consumer is
    /// still moving) and the acknowledgement has the same deadline (a stuck one
    /// is not), so a consumer that never drains cannot hold the exit.
    fn flush_by(&self, deadline: Instant) {
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let mut barrier = WriterMsg::Sync(ack_tx);
        loop {
            match self.tx.try_send(barrier) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    barrier = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        let _ = ack_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    }
}

/// Where the console prints.
enum Sink {
    /// Nothing printed: the flip and any disabled console.
    Noop,
    /// stdout, behind its writer thread.
    Writer(LineWriter),
}

/// Where a line's timestamp comes from: the wall clock in production, a fixed
/// value in tests so two consoles can be compared line for line.
type Clock = fn() -> String;

/// The shared console handle. Cheap to clone (`Arc`).
///
/// ## Shutdown
///
/// A writer thread lives as long as any `Console` clone does: when the LAST one
/// drops, its queue closes and the thread exits on its own. It is a plain
/// `std::thread`, so a hard process exit simply tears it down; a caller that
/// must not lose queued lines calls [`Console::flush`] first.
#[derive(Clone)]
pub struct Console(Arc<ConsoleInner>);

struct ConsoleInner {
    sink: Sink,
    /// The flip's log viewer buffer. Every line is pushed here as well, and
    /// connect/disconnect move its connection counter. `None` for every console
    /// but the flip's.
    capture: Option<ActivityRing>,
    /// `server.log`. Every line that reaches the console is also written here,
    /// whatever else the console does with it.
    file: Option<Arc<ServerLog>>,
    /// The full date and time that leads each line written to `server.log`, in
    /// the format `dux.log` uses. The terminal and the flip keep the short clock.
    stamp: Clock,
    clock: Clock,
    /// Where warning and error lines are ALSO written, in their plain spelling:
    /// stderr, for `dux server` when its stdout went to a file or pipe while
    /// stderr is still the terminal (see
    /// [`dux_core::serve_log::StdStreams::echo_warnings`]), so
    /// `dux server > access.log` never hides one. Behind its own writer thread,
    /// so a stalled stderr never parks the emitter.
    echo: Option<LineWriter>,
    /// Whether [`Console::qr_codes`] shows anything. Off until a serve path that
    /// wants the codes turns it on, from `[server] qr_codes`.
    qr_codes: std::sync::atomic::AtomicBool,
    /// The width the codes are laid out in. Zero asks the terminal each time,
    /// which is the production setting; tests pin a width.
    qr_columns: std::sync::atomic::AtomicUsize,
}

/// Columns the flip's log viewer takes from the terminal around its text: the
/// screen's side margins, the panel's border and its padding.
const VIEWER_CHROME: usize = 8;

impl Console {
    /// A real console writing to stdout. `color` comes from [`detect`];
    /// warning and error lines also go to stderr when `streams` says they would
    /// otherwise go unseen.
    pub fn stdout(color: bool, streams: dux_core::serve_log::StdStreams) -> Self {
        let mut console = Self::with_writer(
            color,
            Box::new(std::io::stdout()),
            WRITER_CHANNEL_BOUND,
            now_hms,
        );
        if streams.echo_warnings() {
            console.set_echo(Box::new(std::io::stderr()));
        }
        console
    }

    /// Echo warning and error lines to `writer`. Only while building: the
    /// console is not shared yet.
    fn set_echo(&mut self, writer: Box<dyn Write + Send>) {
        if let Some(inner) = Arc::get_mut(&mut self.0) {
            inner.echo = Some(LineWriter::spawn(
                writer,
                ECHO_CHANNEL_BOUND,
                false,
                "stderr",
            ));
        }
    }

    /// This console, echoing its warnings to `writer`. Test-only.
    #[cfg(test)]
    pub(crate) fn with_test_echo(mut self, writer: Box<dyn Write + Send>) -> Self {
        self.set_echo(writer);
        self
    }

    /// A buffer-backed console stamped at a fixed time whose warnings are or are
    /// not echoed, plus handles on what it printed to stdout and to stderr.
    #[cfg(test)]
    pub(crate) fn test_capture_echoing(
        color: bool,
        stdout_is_terminal: bool,
    ) -> (Self, TestSink, SharedBuffer) {
        let (mut console, sink) = Self::test_capture(color);
        let stderr = SharedBuffer::new();
        if !stdout_is_terminal {
            console.set_echo(Box::new(stderr.clone()));
        }
        (console, sink, stderr)
    }

    /// Queue a warning or error line on the echo, if there is one. Never blocks.
    fn echo(&self, line: &LogLine) {
        let Some(echo) = &self.0.echo else {
            return;
        };
        if matches!(line.tone(), Some(LogTone::Warn | LogTone::Error)) {
            echo.send(line, self.0.clock);
        }
    }

    /// Build a writer-backed console over an owned writer.
    fn with_writer(color: bool, writer: Box<dyn Write + Send>, bound: usize, clock: Clock) -> Self {
        Self(Arc::new(ConsoleInner {
            sink: Sink::Writer(LineWriter::spawn(writer, bound, color, "stdout")),
            capture: None,
            file: None,
            stamp: now_rfc3339,
            clock,
            echo: None,
            qr_codes: std::sync::atomic::AtomicBool::new(false),
            qr_columns: std::sync::atomic::AtomicUsize::new(0),
        }))
    }

    /// A no-op console: nothing printed, nothing captured.
    pub fn noop() -> Self {
        Self(Arc::new(ConsoleInner {
            sink: Sink::Noop,
            capture: None,
            file: None,
            stamp: now_rfc3339,
            clock: now_hms,
            echo: None,
            qr_codes: std::sync::atomic::AtomicBool::new(false),
            qr_columns: std::sync::atomic::AtomicUsize::new(0),
        }))
    }

    /// This console, also writing every line to `log`. Only while building: the
    /// console is not shared yet.
    pub fn with_server_log(mut self, log: Arc<ServerLog>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.0) {
            inner.file = Some(log);
        }
        self
    }

    /// The file this console writes the server log to, as its writer opened it
    /// (a link at the configured path already resolved), or `None` when it
    /// writes none: the log could not be opened, or nothing here serves.
    pub fn server_log_path(&self) -> Option<std::path::PathBuf> {
        self.0.file.as_ref().map(|log| log.path().to_path_buf())
    }

    /// The background server's console: prints nothing (the terminal UI owns the
    /// terminal) and writes every line to `log`.
    pub fn server_log_only(log: Arc<ServerLog>) -> Self {
        Self::noop().with_server_log(log)
    }

    /// [`Self::server_log_only`] stamped at the fixed test time. Test-only.
    #[cfg(test)]
    pub(crate) fn test_file_only(log: Arc<ServerLog>) -> Self {
        let mut console = Self::noop().with_server_log(log);
        if let Some(inner) = Arc::get_mut(&mut console.0) {
            inner.clock = fixed_test_clock;
        }
        console.with_fixed_stamp()
    }

    /// The flip's console: prints nothing (the status screen owns the terminal)
    /// and records every line into `ring`, the log viewer's buffer.
    pub fn capture(ring: ActivityRing) -> Self {
        Self::capture_with_clock(ring, now_hms)
    }

    fn capture_with_clock(ring: ActivityRing, clock: Clock) -> Self {
        Self(Arc::new(ConsoleInner {
            sink: Sink::Noop,
            capture: Some(ring),
            file: None,
            stamp: now_rfc3339,
            clock,
            echo: None,
            qr_codes: std::sync::atomic::AtomicBool::new(false),
            qr_columns: std::sync::atomic::AtomicUsize::new(0),
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
        )
        .with_fixed_stamp();
        let tx = console.writer_tx();
        (console, TestSink { buf, tx })
    }

    /// The flip's console stamped at the same fixed time as
    /// [`Self::test_capture`], so the two can be compared line for line.
    #[cfg(test)]
    pub(crate) fn test_ring_capture(ring: ActivityRing) -> Self {
        Self::capture_with_clock(ring, fixed_test_clock).with_fixed_stamp()
    }

    /// A console whose writer is stuck for good, as on a pipe nobody reads.
    #[cfg(test)]
    pub(crate) fn test_stuck_writer() -> Self {
        struct Stuck;
        impl Write for Stuck {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                loop {
                    std::thread::park();
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let console = Self::with_writer(false, Box::new(Stuck), 1, fixed_test_clock);
        // One line wedges the writer, the next fills the queue behind it.
        console.info("wedged");
        console.info("queued");
        console
    }

    /// A writer-backed console with a custom channel bound and writer.
    #[cfg(test)]
    fn test_capture_bounded(color: bool, bound: usize, writer: Box<dyn Write + Send>) -> Self {
        Self::with_writer(color, writer, bound, fixed_test_clock)
    }

    #[cfg(test)]
    fn writer_tx(&self) -> SyncSender<WriterMsg> {
        match &self.0.sink {
            Sink::Writer(writer) => writer.tx.clone(),
            Sink::Noop => unreachable!("a writer console always has a sender"),
        }
    }

    #[cfg(test)]
    fn dropped_count(&self) -> u64 {
        match &self.0.sink {
            Sink::Writer(writer) => writer.dropped.load(Ordering::Relaxed),
            Sink::Noop => 0,
        }
    }

    /// Whether this console prints to stdout (a terminal, a file or a pipe, as
    /// `dux server`'s does), rather than nowhere or only into the flip's viewer.
    pub fn is_active(&self) -> bool {
        !matches!(self.0.sink, Sink::Noop)
    }

    /// Whether a line handed to this console goes anywhere at all (printed or
    /// captured). The access-log middleware checks this before doing any work.
    pub fn is_recording(&self) -> bool {
        self.is_active() || self.0.capture.is_some() || self.0.file.is_some()
    }

    /// Write one line to `server.log`, if this console keeps one.
    fn write_to_file(&self, line: &LogLine) {
        if let Some(file) = &self.0.file {
            file.write_line(&format!("{} {}", (self.0.stamp)(), line.render(false)));
        }
    }

    /// Wait, at most [`FLUSH_TIMEOUT`] in all, until every line handed over so
    /// far has been written, to stdout and to the stderr copy alike. Called
    /// before the process exits so the last lines of a run are not lost in a
    /// queue. A slow consumer still gets them; one that never drains cannot hold
    /// the exit. A no-op for a console that prints nothing.
    pub fn flush(&self) {
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        if let Sink::Writer(writer) = &self.0.sink {
            writer.flush_by(deadline);
        }
        if let Some(echo) = &self.0.echo {
            echo.flush_by(deadline);
        }
    }

    /// Send one line to every sink this console has. The one place a line
    /// leaves the console, which is what keeps the two surfaces identical.
    fn line(&self, line: LogLine) {
        self.line_echoed(line, true);
    }

    /// [`Self::line`], with the stderr copy left out when the caller already
    /// printed this line there.
    fn line_echoed(&self, line: LogLine, echo: bool) {
        self.write_to_file(&line);
        self.deliver(line, echo);
    }

    /// Everything [`Self::line_echoed`] does except the file.
    fn deliver(&self, line: LogLine, echo: bool) {
        if let Sink::Writer(writer) = &self.0.sink {
            writer.send(&line, self.0.clock);
            if echo {
                self.echo(&line);
            }
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

    /// A progress line: the shutdown starting and how it went. Informational,
    /// but echoed to stderr like a warning when stdout is redirected, because a
    /// shutdown can wait out its whole grace and a terminal that shows nothing
    /// for that long reads as a hang.
    pub fn progress(&self, message: &str) {
        if !self.is_recording() {
            return;
        }
        let line = LogLine::event(&(self.0.clock)(), LogTone::Info, message);
        self.write_to_file(&line);
        if let Sink::Writer(writer) = &self.0.sink {
            writer.send(&line, self.0.clock);
            if let Some(echo) = &self.0.echo {
                echo.send(&line, self.0.clock);
            }
        }
        if let Some(ring) = &self.0.capture {
            ring.push(line);
        }
    }

    /// A warning line (a startup warning, for one).
    pub fn warn(&self, message: &str) {
        self.emit(LogTone::Warn, message);
    }

    /// A warning line the caller already printed to stderr itself (the
    /// non-loopback alarm, printed before anything loads), so it is not echoed
    /// there a second time.
    pub fn warn_already_on_stderr(&self, message: &str) {
        if !self.is_recording() {
            return;
        }
        self.line_echoed(
            LogLine::event(&(self.0.clock)(), LogTone::Warn, message),
            false,
        );
    }

    /// An error line.
    pub fn error(&self, message: &str) {
        self.emit(LogTone::Error, message);
    }

    /// The post-bind startup banner: a header, one row per bound listener, then
    /// the warning rows and the reachability note.
    ///
    /// In the flip's buffer the banner, and every line logged before it (the
    /// startup warnings), are pinned, so however busy the server gets they are
    /// never evicted from the top of the viewer.
    pub fn banner(&self, banner: &Banner) {
        if !self.is_recording() {
            return;
        }
        let lines = banner.lines();
        for line in &lines {
            self.write_to_file(line);
        }
        if let Sink::Writer(writer) = &self.0.sink {
            for line in &lines {
                writer.send(line, self.0.clock);
                self.echo(line);
            }
        }
        if let Some(ring) = &self.0.capture {
            ring.pin_startup(lines);
        }
    }

    /// Turn the QR codes on or off for this console, from `[server] qr_codes`.
    /// `dux server` passes the setting AND whether stdout is a terminal, so a
    /// piped log is never filled with blocks.
    pub fn set_qr_codes(&self, enabled: bool) {
        self.0
            .qr_codes
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Lay the codes out in exactly `columns` columns instead of asking the
    /// terminal. Test-only.
    #[cfg(test)]
    pub(crate) fn set_qr_columns(&self, columns: usize) {
        self.0
            .qr_columns
            .store(columns, std::sync::atomic::Ordering::Relaxed);
    }

    /// QR codes for `urls` (the Tailscale IP URL, then the MagicDNS one), as
    /// log lines built by [`dux_core::serve_log::qr_lines`], so `dux server`
    /// prints them and the flip's viewer shows them identically. Laid out for
    /// the terminal's width (less the viewer's chrome for the flip's console).
    /// Nothing when the codes are off or there is nothing to show.
    pub fn qr_codes(&self, urls: &[String]) {
        if !self.0.qr_codes.load(std::sync::atomic::Ordering::Relaxed)
            || urls.is_empty()
            || !self.is_recording()
        {
            return;
        }
        let columns = match self.0.qr_columns.load(std::sync::atomic::Ordering::Relaxed) {
            0 if self.is_active() => terminal_columns(),
            0 => terminal_columns().saturating_sub(VIEWER_CHROME),
            pinned => pinned,
        };
        // A drawing for the screen, so it stays out of `server.log`.
        for line in dux_core::serve_log::qr_lines(&(self.0.clock)(), urls, columns) {
            self.deliver(line, true);
        }
    }

    /// Listener rows for addresses that became known after the banner (this
    /// machine's MagicDNS URL, a `tailscale serve` route), drawn exactly like
    /// the banner's own rows on both surfaces.
    pub fn tailnet_rows(&self, rows: &[ListenerRow]) {
        if !self.is_recording() {
            return;
        }
        for line in dux_core::serve_log::listener_lines(rows) {
            self.line(line);
        }
    }

    /// Hand the serve's live URL list to the flip's ring, which its header
    /// lists, so the header follows the Tailscale leg and this machine's name.
    /// Any other console does nothing: `dux server` prints its banner once and
    /// the rows above say what changed.
    pub fn serve_urls(&self, urls: &[String]) {
        if let Some(ring) = &self.0.capture {
            ring.set_serve_urls(urls.to_vec());
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
    /// An access line for a request that came over the control socket, which
    /// has no address to show, so the line says how it came instead.
    pub fn access_over_control_socket(
        &self,
        method: &str,
        path: &str,
        status: u16,
        latency_ms: u128,
    ) {
        if !self.is_recording() {
            return;
        }
        self.line(LogLine::access_over_control_socket(
            &(self.0.clock)(),
            method,
            path,
            status,
            latency_ms,
        ));
    }

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

/// The full timestamp `dux.log` leads its lines with.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
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

#[cfg(test)]
fn fixed_test_stamp() -> String {
    "2026-10-06T12:00:00+00:00".to_string()
}

#[cfg(test)]
impl Console {
    /// This console with its file stamp pinned. Only while building.
    fn with_fixed_stamp(mut self) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.0) {
            inner.stamp = fixed_test_stamp;
        }
        self
    }
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
pub(crate) struct SharedBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(test)]
impl SharedBuffer {
    fn new() -> Self {
        Self(Arc::new(std::sync::Mutex::new(Vec::new())))
    }

    pub(crate) fn contents(&self) -> String {
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

/// The terminal's width in columns: asked of stdout, then `COLUMNS`, then the
/// classic 80.
fn terminal_columns() -> usize {
    if let Ok(size) = rustix::termios::tcgetwinsize(std::io::stdout())
        && size.ws_col > 0
    {
        return usize::from(size.ws_col);
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
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
    use std::time::Duration;

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

    fn test_paths(root: &std::path::Path) -> dux_core::config::DuxPaths {
        dux_core::config::DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        }
    }

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
        console.access_over_control_socket("GET", "/api/v1/workspace", 200, 3);
        assert!(
            sink.contents()
                .ends_with("12:00:00 GET /api/v1/workspace 200 3ms via socket\n"),
            "{}",
            sink.contents()
        );
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
        assert!(Console::stdout(false, dux_core::serve_log::StdStreams::current()).is_active());
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

    /// The startup (its warnings and the banner, reachability note included)
    /// is pinned in the flip's buffer, so a busy server never evicts it.
    #[test]
    fn a_capture_console_pins_the_startup_lines() {
        let ring = ActivityRing::new(2);
        let console = Console::test_ring_capture(ring.clone());
        console.warn("startup warning");
        console.banner(&sample_banner());
        for n in 0..5 {
            console.info(&format!("event {n}"));
        }
        let pinned: Vec<String> = ring.pinned().iter().map(LogLine::text).collect();
        assert_eq!(
            pinned,
            vec![
                "12:00:00 \u{26a0} startup warning".to_string(),
                "dux v0.1.0  plain HTTP".to_string(),
                "  \u{279c} Local: http://127.0.0.1:8080".to_string(),
            ]
        );
        assert_eq!(ring_texts(&ring).len(), 5, "three pinned plus the last two");
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
        let dir = tempfile::tempdir().unwrap();
        let log_in = |name: &str| {
            let server = dux_core::config::ServerConfig {
                log_path: dir.path().join(name).to_string_lossy().into_owned(),
                ..Default::default()
            };
            let paths = test_paths(dir.path());
            Arc::new(dux_core::logger::open_server_log(&server, &paths).unwrap())
        };
        // The same lines also go to `server.log` whichever console wrote them:
        // one that prints, the flip's that captures, and the background server's
        // that does only this.
        let capture = Console::test_ring_capture(ring.clone()).with_server_log(log_in("flip.log"));
        let printing = Console::test_capture(false)
            .0
            .with_server_log(log_in("printing.log"));
        let file_only = Console::test_file_only(log_in("file-only.log"));
        for console in [&stdout, &capture, &printing, &file_only] {
            console.banner(&sample_banner());
            console.client_connected(ip("10.0.0.1"));
            console.access("GET", "/", 404, 1);
            console.error("broken");
        }
        let printed: Vec<String> = sink.contents().lines().map(strip_ansi).collect();
        assert_eq!(printed, ring_texts(&ring));

        // The file spells each line the way `dux server` prints it without color,
        // led by the full date and time `dux.log` uses.
        let expected = [
            "2026-10-06T12:00:00+00:00 dux v0.1.0  plain HTTP",
            "2026-10-06T12:00:00+00:00   -> Local: http://127.0.0.1:8080",
            "2026-10-06T12:00:00+00:00 12:00:00 info client connected from 10.0.0.1",
            "2026-10-06T12:00:00+00:00 12:00:00 GET / 404 1ms",
            "2026-10-06T12:00:00+00:00 12:00:00 error broken",
        ];
        for name in ["flip.log", "printing.log", "file-only.log"] {
            let written = std::fs::read_to_string(dir.path().join(name)).unwrap();
            assert_eq!(written.lines().collect::<Vec<_>>(), expected, "{name}");
        }
    }

    const QR_IP: &str = "http://100.101.102.103:3890";
    const QR_NAME: &str = "https://demo-box.example-tailnet.ts.net";

    /// The QR codes and the tailnet address rows reach `dux server`'s output
    /// and the flip's viewer as the same lines.
    #[test]
    fn qr_codes_and_tailnet_rows_reach_both_surfaces_as_the_same_lines() {
        let (stdout, sink) = Console::test_capture(true);
        let ring = ActivityRing::new(200);
        let dir = tempfile::tempdir().unwrap();
        let server = dux_core::config::ServerConfig {
            log_path: dir.path().join("qr.log").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let log =
            Arc::new(dux_core::logger::open_server_log(&server, &test_paths(dir.path())).unwrap());
        let capture = Console::test_ring_capture(ring.clone()).with_server_log(log);
        let rows = vec![dux_core::serve_log::ListenerRow {
            label: "Tailscale (HTTPS, tailscale serve)".to_string(),
            url: QR_NAME.to_string(),
        }];
        for console in [&stdout, &capture] {
            console.set_qr_codes(true);
            console.set_qr_columns(120);
            console.tailnet_rows(&rows);
            console.qr_codes(&[QR_IP.to_string(), QR_NAME.to_string()]);
        }
        let printed: Vec<String> = sink.contents().lines().map(strip_ansi).collect();
        assert_eq!(printed, ring_texts(&ring));
        assert!(
            printed
                .iter()
                .any(|l| l.contains("Tailscale (HTTPS, tailscale serve)"))
        );
        assert!(printed.iter().any(|l| l.contains('█') || l.contains('▀')));

        // The codes are a drawing for the screen: the file keeps the address row
        // and none of the blocks.
        let file = std::fs::read_to_string(dir.path().join("qr.log")).unwrap();
        assert!(
            file.contains("Tailscale (HTTPS, tailscale serve)"),
            "{file}"
        );
        assert!(!file.contains('█') && !file.contains('▀'), "{file}");
    }

    #[test]
    fn qr_codes_switched_off_or_with_nothing_to_show_print_nothing() {
        let (console, sink) = Console::test_capture(false);
        console.set_qr_columns(120);
        console.qr_codes(&[QR_IP.to_string()]);
        assert_eq!(sink.contents(), "", "off unless a serve path turns it on");
        console.set_qr_codes(true);
        console.qr_codes(&[]);
        assert_eq!(sink.contents(), "", "no address, no code");
    }

    #[test]
    fn qr_codes_stack_on_a_narrow_terminal() {
        let (console, sink) = Console::test_capture(false);
        console.set_qr_codes(true);
        console.set_qr_columns(60);
        console.qr_codes(&[QR_IP.to_string(), QR_NAME.to_string()]);
        let out = sink.contents();
        let ip_row = out
            .lines()
            .position(|l| l.contains(QR_IP))
            .expect("IP label");
        let name_row = out
            .lines()
            .position(|l| l.contains(QR_NAME))
            .expect("name label");
        assert!(name_row > ip_row + 1, "{out}");
        assert!(out.lines().all(|line| line.chars().count() <= 60), "{out}");
    }

    /// The flip's header lists the serve's live URLs from the ring. Not gated
    /// by `qr_codes`, and not printed on stdout, whose banner is printed once.
    #[test]
    fn the_live_url_list_goes_to_the_flips_ring_only() {
        let urls = vec![QR_IP.to_string(), QR_NAME.to_string()];
        let ring = ActivityRing::new(10);
        Console::test_ring_capture(ring.clone()).serve_urls(&urls);
        assert_eq!(ring.serve_urls(), urls);
        let (stdout, sink) = Console::test_capture(false);
        stdout.serve_urls(&urls);
        assert_eq!(sink.contents(), "");
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

    /// A writer stuck on a full pipe (`dux server | less` that stopped reading)
    /// must not hang a flush: the force-exit hatch flushes, and it has to exit.
    #[test]
    fn flush_gives_up_on_a_writer_that_never_returns() {
        let (_release, gate) = std::sync::mpsc::channel::<()>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let writer = GatedWriter {
            buf: SharedBuffer::new(),
            gate,
            entered: entered_tx,
        };
        let console = Console::test_capture_bounded(false, 2, Box::new(writer));
        console.info("stuck");
        entered_rx.recv().expect("the writer is wedged in write");
        // Fill the channel too, so even queueing the barrier would block.
        console.info("queued 1");
        console.info("queued 2");
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            console.flush();
            let _ = done_tx.send(());
        });
        assert!(
            done_rx.recv_timeout(Duration::from_secs(10)).is_ok(),
            "flush must return even though the writer never does"
        );
    }

    // ── Warnings when stdout is not a terminal ─────────────────────────────

    fn banner_with_a_warning() -> Banner {
        let mut banner = sample_banner();
        banner.security_note = Some("Reachable on your network with NO login.".to_string());
        banner
    }

    /// Everything a run warns about or fails at, plus ordinary lines.
    fn warn_and_info(console: &Console) {
        console.warn("Tailscale not detected.");
        console.warn(&unknown_color_warning("rainbow"));
        console.banner(&banner_with_a_warning());
        console.info("client connected from 127.0.0.1");
        console.progress("Requesting 1 agent and 0 terminals to gracefully shut down.");
        console.access("GET", "/", 200, 1);
        console.warn("1 agent did not stop in time. Force-closing 1 agent.");
        console.error("second interrupt received during shutdown.");
    }

    /// `dux server > access.log` must never hide a warning, nor leave the
    /// terminal silent while the shutdown waits: with stdout not a terminal,
    /// every warning and error line, and the shutdown's progress, also goes to
    /// stderr, and ordinary lines stay on stdout alone.
    #[test]
    fn with_stdout_redirected_warnings_and_errors_also_reach_stderr() {
        let (console, sink, stderr) = Console::test_capture_echoing(false, false);
        warn_and_info(&console);
        console.flush();
        let stdout = sink.contents();
        let stderr = stderr.contents();
        assert_eq!(
            stderr,
            "12:00:00 warn Tailscale not detected.\n\
             12:00:00 warn [server] color = \"rainbow\" is not auto/always/never. Using \"auto\".\n\
             \x20\x20warn Reachable on your network with NO login.\n\
             12:00:00 info Requesting 1 agent and 0 terminals to gracefully shut down.\n\
             12:00:00 warn 1 agent did not stop in time. Force-closing 1 agent.\n\
             12:00:00 error second interrupt received during shutdown.\n"
        );
        assert!(stdout.contains("client connected"), "{stdout}");
        assert!(
            !stderr.contains("client connected"),
            "info stays off stderr"
        );
        assert!(!stderr.contains("GET /"), "the access log stays off stderr");
        for line in stderr.lines() {
            assert_eq!(stdout.matches(line).count(), 1, "once on stdout: {line}");
            assert_eq!(stderr.matches(line).count(), 1, "once on stderr: {line}");
        }
    }

    /// A stderr nobody drains (a `2>&1` pipe that stopped reading) must not park
    /// the thread that raised the warning: the echo has its own bounded,
    /// non-blocking path, so even the force-exit line goes through.
    #[test]
    fn the_echo_never_blocks_the_line_that_raised_it() {
        let (_release, gate) = std::sync::mpsc::channel::<()>();
        let (entered, _entered_rx) = std::sync::mpsc::channel::<()>();
        let stuck = GatedWriter {
            buf: SharedBuffer::new(),
            gate,
            entered,
        };
        let (console, _sink) = Console::test_capture(false);
        let console = console.with_test_echo(Box::new(stuck));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for n in 0..50 {
                console.error(&format!("error {n}"));
            }
            let _ = done_tx.send(());
        });
        assert!(
            done_rx.recv_timeout(Duration::from_secs(10)).is_ok(),
            "raising warnings must not wait on stderr"
        );
    }

    /// A slow but moving stdout still gets the run's last lines: the flush
    /// waits (bounded) for room in the queue rather than giving up at once.
    #[test]
    fn flush_waits_for_a_slow_but_moving_consumer() {
        struct Slow(SharedBuffer);
        impl Write for Slow {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(30));
                self.0.write(data)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = SharedBuffer::new();
        let console = Console::test_capture_bounded(false, 2, Box::new(Slow(buf.clone())));
        // The writer takes the first line and sits in its slow write; the next
        // two fill the queue, so the flush's barrier has to wait for room.
        console.info("shutdown line 0");
        std::thread::sleep(Duration::from_millis(10));
        console.info("shutdown line 1");
        console.info("shutdown line 2");
        console.flush();
        assert!(
            buf.contents().contains("shutdown line 2"),
            "{}",
            buf.contents()
        );
    }

    /// On a terminal the log line is enough: nothing goes to stderr.
    #[test]
    fn with_stdout_on_a_terminal_each_warning_prints_once() {
        let (console, sink, stderr) = Console::test_capture_echoing(false, true);
        warn_and_info(&console);
        console.flush();
        assert_eq!(stderr.contents(), "");
        assert_eq!(
            sink.contents().matches("Tailscale not detected.").count(),
            1
        );
    }

    /// The security alarm was already printed to stderr before anything loaded,
    /// so its log line must not print it there a second time.
    #[test]
    fn a_warning_already_on_stderr_is_not_echoed_again() {
        let (console, sink, stderr) = Console::test_capture_echoing(false, false);
        console.warn_already_on_stderr("dux is binding 0.0.0.0:3890 with NO login gate.");
        console.flush();
        assert_eq!(stderr.contents(), "");
        assert!(sink.contents().contains("warn dux is binding 0.0.0.0:3890"));
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
