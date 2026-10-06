//! The one line model behind the web server's console, shared by every surface
//! that shows it.
//!
//! `dux server` prints its log to stdout and the `start-web-server` flip shows
//! the same log in its status screen. Both read the lines built here, so a line
//! is worded once: the producer (`dux-web`'s console) builds a [`LogLine`] from
//! segments, stdout renders it with [`LogLine::render`] (ANSI or plain words),
//! and the flip's viewer draws the same segments through its theme. A segment
//! carries a [`LogRole`] that says what it is (a timestamp, a tone marker, a
//! URL), never a color, so each surface styles it in its own palette.
//!
//! The rich spelling of a line, [`LogLine::text`], is what the colored console
//! prints with its escapes stripped and what the viewer shows. A tone always
//! has a glyph in it (`➜`, `⚠`), so the viewer never carries tone on color
//! alone. Only stdout with color off swaps the glyphs for plain words, so a
//! redirected log stays ASCII.

use std::fmt::Display;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::os::fd::{AsFd, BorrowedFd};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
/// Black on bright white, for a QR code: the one place the log picks a
/// background, because a code must be dark on light whatever the terminal's own
/// colors are, and no scanner is obliged to read one the other way round.
const QR_COLORS: &str = "\x1b[30;107m";

/// Indent of every QR row, matching the banner's listener rows.
pub const QR_INDENT: &str = "  ";

/// How a log line reads at a glance: the glyph it carries and the color a
/// surface paints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTone {
    Info,
    Ok,
    Warn,
    Error,
}

impl LogTone {
    /// The glyph the rich spelling uses.
    pub fn glyph(self) -> &'static str {
        match self {
            LogTone::Info => "\u{279c}",  // ➜
            LogTone::Ok => "\u{2713}",    // ✓
            LogTone::Warn => "\u{26a0}",  // ⚠
            LogTone::Error => "\u{2717}", // ✗
        }
    }

    /// The plain word stdout prints in place of the glyph when color is off.
    pub fn label(self) -> &'static str {
        match self {
            LogTone::Info => "info",
            LogTone::Ok => "ok",
            LogTone::Warn => "warn",
            LogTone::Error => "error",
        }
    }

    fn ansi(self) -> &'static str {
        match self {
            LogTone::Info => CYAN,
            LogTone::Ok => GREEN,
            LogTone::Warn => YELLOW,
            LogTone::Error => RED,
        }
    }
}

/// What a segment of a line is. Surfaces map a role to a style; the text is the
/// same on all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRole {
    /// The `HH:MM:SS` stamp at the head of an event line.
    Timestamp,
    /// The tone glyph (or, plain, its word).
    Marker(LogTone),
    /// The words of an event or a banner warning.
    Message(LogTone),
    /// The `dux` name at the head of the banner.
    Name,
    /// The version beside it.
    Version,
    /// A listener's label (`Local (loopback)`, `Tailscale`).
    Label,
    /// A URL someone opens.
    Url,
    /// An access line's HTTP method.
    Method,
    /// An access line's status code, styled by its class.
    Status(u16),
    /// Part of a QR code. Its rich spelling is drawn dark on light (stdout in
    /// color forces black on bright white, the flip's viewer uses the darker
    /// and the lighter of its theme's text and background); its plain spelling
    /// is drawn for a dark terminal with no colors at all.
    QrCode,
    /// Spacing, separators and anything else with no meaning of its own.
    Plain,
}

/// One run of text with one role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSegment {
    /// The rich spelling, the one the viewer and colored stdout show.
    pub text: String,
    /// The spelling stdout uses when color is off, when it differs (a glyph's
    /// word). `None` means the same as `text`.
    pub plain: Option<String>,
    pub role: LogRole,
}

impl LogSegment {
    fn new(text: impl Into<String>, role: LogRole) -> Self {
        Self {
            text: text.into(),
            plain: None,
            role,
        }
    }

    fn marker(tone: LogTone, plain: &str) -> Self {
        Self {
            text: tone.glyph().to_string(),
            plain: Some(plain.to_string()),
            role: LogRole::Marker(tone),
        }
    }

    fn plain(text: impl Into<String>) -> Self {
        Self::new(text, LogRole::Plain)
    }

    fn ansi_prefix(&self) -> Option<&'static str> {
        match self.role {
            LogRole::Timestamp | LogRole::Version => Some(DIM),
            LogRole::Marker(tone) => Some(tone.ansi()),
            LogRole::Name => Some("\x1b[1m\x1b[36m"),
            LogRole::Label | LogRole::Method => Some(BOLD),
            LogRole::Url => Some(CYAN),
            LogRole::Status(code) => status_ansi(code),
            LogRole::QrCode => Some(QR_COLORS),
            LogRole::Message(_) | LogRole::Plain => None,
        }
    }
}

/// The status-class color of an access line's code: 2xx green, 3xx cyan, 4xx
/// yellow, 5xx red, anything else unstyled.
fn status_ansi(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => Some(GREEN),
        300..=399 => Some(CYAN),
        400..=499 => Some(YELLOW),
        500..=599 => Some(RED),
        _ => None,
    }
}

/// One line of the server's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub segments: Vec<LogSegment>,
}

impl LogLine {
    /// A timestamped event: `<hms> <glyph> <message>`.
    pub fn event(hms: &str, tone: LogTone, message: &str) -> Self {
        Self {
            segments: vec![
                LogSegment::new(hms, LogRole::Timestamp),
                LogSegment::plain(" "),
                LogSegment::marker(tone, tone.label()),
                LogSegment::plain(" "),
                LogSegment::new(message, LogRole::Message(tone)),
            ],
        }
    }

    /// One access-log line: `<hms> <METHOD> <path> <status> <latency>ms`. The
    /// path is printed verbatim; the caller strips the query string, which can
    /// carry secrets.
    pub fn access(hms: &str, method: &str, path: &str, status: u16, latency_ms: u128) -> Self {
        Self {
            segments: vec![
                LogSegment::new(hms, LogRole::Timestamp),
                LogSegment::plain(" "),
                LogSegment::new(method, LogRole::Method),
                LogSegment::plain(format!(" {path} ")),
                LogSegment::new(status.to_string(), LogRole::Status(status)),
                LogSegment::plain(format!(" {latency_ms}ms")),
            ],
        }
    }

    /// [`Self::access`] for a request over the control socket: the same line,
    /// saying it came that way.
    pub fn access_over_control_socket(
        hms: &str,
        method: &str,
        path: &str,
        status: u16,
        latency_ms: u128,
    ) -> Self {
        let mut line = Self::access(hms, method, path, status, latency_ms);
        line.segments.push(LogSegment::plain(" via socket"));
        line
    }

    /// The line's tone: the one its marker carries, or `None` for a line with no
    /// marker (the banner header, an access line).
    pub fn tone(&self) -> Option<LogTone> {
        self.segments.iter().find_map(|segment| match segment.role {
            LogRole::Marker(tone) => Some(tone),
            _ => None,
        })
    }

    /// The rich spelling: what colored stdout prints without its escapes, and
    /// what the viewer shows.
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// The stdout spelling: ANSI-styled glyphs when `color`, plain words and no
    /// escapes otherwise.
    pub fn render(&self, color: bool) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            if color {
                match segment.ansi_prefix() {
                    Some(prefix) => {
                        out.push_str(prefix);
                        out.push_str(&segment.text);
                        out.push_str(RESET);
                    }
                    None => out.push_str(&segment.text),
                }
            } else {
                out.push_str(segment.plain.as_deref().unwrap_or(&segment.text));
            }
        }
        out
    }
}

/// One labeled, bound listener row in the startup banner. A new kind of address
/// (a MagicDNS name, say) is one more row: the banner renders every row the same
/// way on every surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerRow {
    /// The label for this leg (e.g. `Local (loopback)`, `Tailscale`).
    pub label: String,
    /// The full URL a user opens.
    pub url: String,
}

/// Everything the post-bind banner says. Built from the addresses that actually
/// bound, so it shows truth rather than a pre-bind guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    /// The dux display version (`vX.Y.Z` or `development`), rendered verbatim.
    pub version: String,
    /// The mode line (e.g. `plain HTTP`).
    pub mode: String,
    /// One row per bound listener.
    pub listeners: Vec<ListenerRow>,
    /// Warning rows for degraded or best-effort legs.
    pub warnings: Vec<String>,
    /// The reachability note, shown when the server can be reached beyond
    /// loopback (now or later).
    pub security_note: Option<String>,
}

impl Banner {
    /// The banner's lines: a header, one row per listener, then the warning rows
    /// and the security note.
    pub fn lines(&self) -> Vec<LogLine> {
        let mut out = vec![LogLine {
            segments: vec![
                LogSegment::new("dux", LogRole::Name),
                LogSegment::plain(" "),
                LogSegment::new(self.version.clone(), LogRole::Version),
                LogSegment::plain(format!("  {}", self.mode)),
            ],
        }];
        out.extend(listener_lines(&self.listeners));
        for note in self.warnings.iter().chain(self.security_note.iter()) {
            out.push(LogLine {
                segments: vec![
                    LogSegment::plain("  "),
                    LogSegment::marker(LogTone::Warn, LogTone::Warn.label()),
                    LogSegment::plain(" "),
                    LogSegment::new(note.clone(), LogRole::Message(LogTone::Warn)),
                ],
            });
        }
        out
    }
}

/// Listener rows as the banner draws them, for addresses that become known after
/// the banner was printed (this machine's MagicDNS name, a `tailscale serve`
/// route), so they read exactly like the rows above them.
pub fn listener_lines(rows: &[ListenerRow]) -> Vec<LogLine> {
    rows.iter()
        .map(|row| LogLine {
            segments: vec![
                LogSegment::plain("  "),
                LogSegment::marker(LogTone::Info, "->"),
                LogSegment::plain(" "),
                LogSegment::new(row.label.clone(), LogRole::Label),
                LogSegment::plain(": "),
                LogSegment::new(row.url.clone(), LogRole::Url),
            ],
        })
        .collect()
}

/// QR codes for `urls` (the Tailscale IP URL, then the MagicDNS one), laid out
/// by [`crate::qr::layout`] in `columns` columns, side by side when they fit and
/// stacked when they do not, each with its URL under it, after a caption.
/// Empty when there is nothing to draw.
///
/// Each code segment carries both spellings: the rich one drawn dark on light,
/// and the plain one drawn for a dark terminal, so stdout with color off still
/// shows a code a camera reads.
pub fn qr_lines(hms: &str, urls: &[String], columns: usize) -> Vec<LogLine> {
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let available = columns.saturating_sub(QR_INDENT.len());
    let dark = crate::qr::layout(&refs, available, crate::qr::Polarity::DarkModulesFilled);
    let light = crate::qr::layout(&refs, available, crate::qr::Polarity::LightModulesFilled);
    if dark.is_empty() {
        return Vec::new();
    }
    let mut out = vec![LogLine::event(
        hms,
        LogTone::Info,
        "Scan to open dux from your phone:",
    )];
    for (dark_row, light_row) in dark.iter().zip(&light) {
        let mut segments = vec![LogSegment::plain(QR_INDENT)];
        for (dark_seg, light_seg) in dark_row.iter().zip(light_row) {
            segments.push(match (dark_seg, light_seg) {
                (crate::qr::Segment::Code(rich), crate::qr::Segment::Code(plain)) => LogSegment {
                    text: rich.clone(),
                    plain: Some(plain.clone()),
                    role: LogRole::QrCode,
                },
                (other, _) if other.text().trim().is_empty() => LogSegment::plain(other.text()),
                (other, _) => LogSegment::new(other.text(), LogRole::Url),
            });
        }
        out.push(LogLine { segments });
    }
    out
}

/// What a serve learned before it bound: warnings to print first, the
/// best-effort bind failures for the banner, and whether a Tailscale address was
/// found at all (which is what tells "waiting for the interface" from "the
/// address would not bind"). `dux server` fills it in its own startup; the flip
/// fills it in the terminal UI's pre-flight and hands it across.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupNotes {
    /// Warnings raised before binding, printed as timestamped warning lines
    /// ahead of the banner.
    pub warnings: Vec<String>,
    /// Best-effort (Tailscale) bind failures, shown as banner warning rows.
    pub bind_warnings: Vec<String>,
    /// Whether a Tailscale address was detected.
    pub tailscale_detected: bool,
}

/// Where `dux server`'s stdout and stderr go, which decides where its warnings
/// are printed so each lands once per place a person reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdStreams {
    pub stdout_is_terminal: bool,
    pub stderr_is_terminal: bool,
    /// Both name the same open file (device and inode), as with `> log 2>&1`,
    /// `nohup`, or a service manager's one log stream.
    pub same_file: bool,
}

impl StdStreams {
    /// Read the two descriptors.
    pub fn of(stdout: BorrowedFd<'_>, stderr: BorrowedFd<'_>) -> Self {
        Self {
            stdout_is_terminal: stdout.is_terminal(),
            stderr_is_terminal: stderr.is_terminal(),
            same_file: same_file(stdout, stderr),
        }
    }

    /// The process's own stdout and stderr.
    pub fn current() -> Self {
        Self::of(std::io::stdout().as_fd(), std::io::stderr().as_fd())
    }

    /// Whether warning and error lines are also written to stderr: only when
    /// stdout went somewhere a person is not reading (a file, a pipe) while
    /// stderr is still the terminal, and the two are not one file. Anything
    /// else would print the same line twice into one place: both to the
    /// journal under a service manager, both into one file under `2>&1`.
    ///
    /// Accepted: `dux server | tee log` echoes, because stdout is a pipe and
    /// stderr the terminal, so the terminal shows a warning twice (once through
    /// `tee`, once on stderr).
    pub fn echo_warnings(&self) -> bool {
        !self.stdout_is_terminal && self.stderr_is_terminal && !self.same_file
    }

    /// Whether the no-login alarm is printed to stderr the moment it is known,
    /// ahead of the log: whenever stdout is not a terminal and stderr is another
    /// place, so a redirected or piped stdout cannot hide it. On an interactive
    /// terminal the log line is enough (and a start that fails before the log
    /// opens prints it then).
    pub fn early_alarm_on_stderr(&self) -> bool {
        !self.stdout_is_terminal && !self.same_file
    }
}

/// Whether two descriptors are the same open file, by device and inode. An fd
/// whose metadata cannot be read is taken as different.
fn same_file(a: BorrowedFd<'_>, b: BorrowedFd<'_>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let id = |fd: BorrowedFd<'_>| {
        fd.try_clone_to_owned()
            .map(std::fs::File::from)
            .and_then(|file| file.metadata())
            .map(|meta| (meta.dev(), meta.ino()))
            .ok()
    };
    matches!((id(a), id(b)), (Some(x), Some(y)) if x == y)
}

/// The warning shown when the best-effort Tailscale listener cannot bind
/// because something else holds that address. Names the address, the cause and
/// both remedies. One wording for every serving mode.
pub fn tailscale_bind_warning(addr: SocketAddr, err: &dyn Display) -> String {
    format!(
        "could not bind the Tailscale address {addr}: {err}. Something else is already \
         listening there, so dux is serving on the remaining address(es) only. Stop that \
         process or change [server].port to also serve on Tailscale."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
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

    fn sample_banner() -> Banner {
        Banner {
            version: "v0.1.0".to_string(),
            mode: "plain HTTP".to_string(),
            listeners: vec![ListenerRow {
                label: "Local (loopback)".to_string(),
                url: "http://127.0.0.1:3890".to_string(),
            }],
            warnings: vec!["Tailscale: waiting for the interface (auto).".to_string()],
            security_note: Some("Reachable by other devices on your tailnet.".to_string()),
        }
    }

    // ── QR codes and tailnet rows ──────────────────────────────────────────

    const QR_IP: &str = "http://100.101.102.103:3890";
    const QR_NAME: &str = "https://demo-box.example-tailnet.ts.net";

    #[test]
    fn a_qr_block_is_a_caption_then_the_shared_layout_in_both_spellings() {
        let urls = [QR_IP.to_string(), QR_NAME.to_string()];
        let lines = qr_lines("12:00:00", &urls, 120);
        assert_eq!(
            lines[0].text(),
            format!(
                "12:00:00 {} Scan to open dux from your phone:",
                LogTone::Info.glyph()
            )
        );
        let refs = [QR_IP, QR_NAME];
        let dark = crate::qr::layout(
            &refs,
            120 - QR_INDENT.len(),
            crate::qr::Polarity::DarkModulesFilled,
        );
        let light = crate::qr::layout(
            &refs,
            120 - QR_INDENT.len(),
            crate::qr::Polarity::LightModulesFilled,
        );
        let rows = &lines[1..];
        assert_eq!(rows.len(), dark.len());
        for ((line, dark), light) in rows.iter().zip(&dark).zip(&light) {
            let codes: Vec<&LogSegment> = line
                .segments
                .iter()
                .filter(|s| s.role == LogRole::QrCode)
                .collect();
            let dark_codes: Vec<&str> = dark
                .iter()
                .filter_map(|s| match s {
                    crate::qr::Segment::Code(c) => Some(c.as_str()),
                    crate::qr::Segment::Text(_) => None,
                })
                .collect();
            let light_codes: Vec<&str> = light
                .iter()
                .filter_map(|s| match s {
                    crate::qr::Segment::Code(c) => Some(c.as_str()),
                    crate::qr::Segment::Text(_) => None,
                })
                .collect();
            assert_eq!(
                codes.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
                dark_codes,
                "the rich spelling is drawn dark on light"
            );
            assert_eq!(
                codes
                    .iter()
                    .map(|s| s.plain.as_deref().unwrap_or(&s.text))
                    .collect::<Vec<_>>(),
                light_codes,
                "the plain spelling is drawn for a dark terminal"
            );
        }
        let labels = lines.last().unwrap();
        assert!(labels.text().contains(QR_IP) && labels.text().contains(QR_NAME));
        assert!(labels.segments.iter().any(|s| s.role == LogRole::Url));
        assert!(qr_lines("12:00:00", &[], 120).is_empty());
    }

    #[test]
    fn a_qr_code_renders_black_on_white_in_color_and_plain_without() {
        let lines = qr_lines("12:00:00", &[QR_IP.to_string()], 120);
        let colored = lines[1].render(true);
        assert!(colored.contains("\x1b[30;107m"), "{colored:?}");
        let plain = lines[1].render(false);
        assert!(!plain.contains('\x1b'), "{plain:?}");
    }

    #[test]
    fn tailnet_rows_read_like_the_banners_listener_rows() {
        let row = ListenerRow {
            label: "Tailscale (MagicDNS)".to_string(),
            url: "http://demo-box.example-tailnet.ts.net:3890".to_string(),
        };
        let banner = Banner {
            listeners: vec![row.clone()],
            ..sample_banner()
        };
        assert_eq!(
            listener_lines(std::slice::from_ref(&row))[0],
            banner.lines()[1]
        );
    }

    #[test]
    fn an_event_line_reads_the_same_rich_and_colored() {
        let line = LogLine::event("12:00:00", LogTone::Info, "client connected from 10.0.0.1");
        assert_eq!(
            line.text(),
            "12:00:00 \u{279c} client connected from 10.0.0.1"
        );
        let colored = line.render(true);
        assert!(colored.contains("\x1b["), "{colored}");
        assert_eq!(strip_ansi(&colored), line.text());
    }

    #[test]
    fn an_event_line_without_color_uses_the_word_and_no_escape() {
        let line = LogLine::event("12:00:00", LogTone::Warn, "careful");
        assert_eq!(line.render(false), "12:00:00 warn careful");
    }

    #[test]
    fn every_tone_has_a_glyph_in_its_rich_spelling() {
        for tone in [LogTone::Info, LogTone::Ok, LogTone::Warn, LogTone::Error] {
            let line = LogLine::event("t", tone, "m");
            assert!(line.text().contains(tone.glyph()));
            assert!(line.render(true).contains(tone.ansi()));
            assert!(line.render(false).contains(tone.label()));
        }
    }

    #[test]
    fn an_access_line_has_its_shape_and_status_class() {
        let line = LogLine::access("12:00:00", "GET", "/api/v1/build", 200, 3);
        assert_eq!(line.text(), "12:00:00 GET /api/v1/build 200 3ms");
        assert_eq!(line.render(false), line.text());
        assert!(
            LogLine::access("t", "GET", "/", 204, 1)
                .render(true)
                .contains(GREEN)
        );
        assert!(
            LogLine::access("t", "GET", "/", 308, 1)
                .render(true)
                .contains(CYAN)
        );
        assert!(
            LogLine::access("t", "GET", "/", 404, 1)
                .render(true)
                .contains(YELLOW)
        );
        assert!(
            LogLine::access("t", "GET", "/", 500, 1)
                .render(true)
                .contains(RED)
        );
    }

    #[test]
    fn the_banner_reads_header_listeners_warnings_then_the_note() {
        let lines: Vec<String> = sample_banner().lines().iter().map(LogLine::text).collect();
        assert_eq!(
            lines,
            vec![
                "dux v0.1.0  plain HTTP".to_string(),
                "  \u{279c} Local (loopback): http://127.0.0.1:3890".to_string(),
                "  \u{26a0} Tailscale: waiting for the interface (auto).".to_string(),
                "  \u{26a0} Reachable by other devices on your tailnet.".to_string(),
            ]
        );
    }

    #[test]
    fn the_banner_without_color_keeps_its_ascii_spelling() {
        let lines: Vec<String> = sample_banner()
            .lines()
            .iter()
            .map(|l| l.render(false))
            .collect();
        assert_eq!(lines[1], "  -> Local (loopback): http://127.0.0.1:3890");
        assert_eq!(
            lines[2],
            "  warn Tailscale: waiting for the interface (auto)."
        );
        assert!(lines.iter().all(|l| !l.contains('\x1b')));
    }

    #[test]
    fn the_colored_banner_strips_to_its_rich_spelling() {
        for line in sample_banner().lines() {
            assert_eq!(strip_ansi(&line.render(true)), line.text());
        }
    }

    // ── Where stdout and stderr go ─────────────────────────────────────────

    /// A file of its own, removed by the system once the test drops it.
    fn temp_file() -> std::fs::File {
        tempfile::tempfile().expect("temp file")
    }

    /// A real terminal: the master side of a pseudo-terminal answers isatty.
    fn terminal() -> Box<dyn portable_pty::MasterPty + Send> {
        portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("a pty")
            .master
    }

    fn fd_of(master: &dyn portable_pty::MasterPty) -> BorrowedFd<'_> {
        let raw = master.as_raw_fd().expect("a pty fd");
        // SAFETY: the master outlives the borrow, which ends with the test.
        unsafe { BorrowedFd::borrow_raw(raw) }
    }

    #[test]
    fn warnings_are_echoed_only_when_stdout_is_redirected_and_stderr_is_a_terminal() {
        let tty = terminal();
        let file = temp_file();
        let (reader, writer) = std::io::pipe().expect("a pipe");

        // `dux server > access.log`: stdout a file, stderr the terminal.
        assert!(StdStreams::of(file.as_fd(), fd_of(&*tty)).echo_warnings());
        // `dux server | less`: stdout a pipe, stderr the terminal.
        assert!(StdStreams::of(writer.as_fd(), fd_of(&*tty)).echo_warnings());
        // Interactive: both the terminal. The log line is enough.
        assert!(!StdStreams::of(fd_of(&*tty), fd_of(&*tty)).echo_warnings());
        // `> log 2>&1`, nohup: both the same file.
        assert!(!StdStreams::of(file.as_fd(), file.as_fd()).echo_warnings());
        // systemd and friends: neither a terminal.
        assert!(!StdStreams::of(writer.as_fd(), reader.as_fd()).echo_warnings());
        assert!(!StdStreams::of(file.as_fd(), writer.as_fd()).echo_warnings());
    }

    /// The no-login alarm goes to stderr ahead of everything only when the log
    /// line on stdout would not be seen there anyway: never on an interactive
    /// terminal (the log line is enough), never into the very file stdout is.
    #[test]
    fn the_early_alarm_is_printed_only_when_stdout_is_elsewhere() {
        let tty = terminal();
        let file = temp_file();
        let other = temp_file();
        assert!(!StdStreams::of(fd_of(&*tty), fd_of(&*tty)).early_alarm_on_stderr());
        assert!(StdStreams::of(file.as_fd(), fd_of(&*tty)).early_alarm_on_stderr());
        assert!(StdStreams::of(file.as_fd(), other.as_fd()).early_alarm_on_stderr());
        assert!(!StdStreams::of(file.as_fd(), file.as_fd()).early_alarm_on_stderr());
        let dup = file.try_clone().expect("a second fd on the same file");
        assert!(!StdStreams::of(file.as_fd(), dup.as_fd()).early_alarm_on_stderr());
    }

    #[test]
    fn the_tailscale_bind_warning_names_the_address_cause_and_remedies() {
        let addr: SocketAddr = "100.64.0.1:3890".parse().unwrap();
        let w = tailscale_bind_warning(addr, &"address in use");
        assert!(w.contains("100.64.0.1:3890"));
        assert!(w.contains("address in use"));
        assert!(w.contains("Stop that process"));
        assert!(w.contains("[server].port"));
    }
}
