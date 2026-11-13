//! The input debugger's own reader and its log entries.
//!
//! While the debugger is open it reads the host's raw bytes itself, through a
//! branch that never touches a pseudo-terminal, so it shows what the terminal
//! really sent: a key crossterm would drop, the bytes behind every key, and a
//! paste, a focus report or a mouse report for what it is. It needs no
//! selected agent, live child or ownership, so it works from anywhere.

use std::fmt::Write as _;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::components::ellipsis::pad_to_width;
use super::components::wrap_lines::display_width;
use super::input::{ESC_AMBIGUITY_TIMEOUT, read_raw_chunk, trim_debug_input_lines, wait_for_input};
use super::{App, PromptState};
use crate::host_keys::Host;
use crate::keybindings::BindingScope;
use crate::raw_input::{BRACKET_PASTE_END, BRACKET_PASTE_START, ParsedSequence};
use crate::theme::Theme;

/// How the time at the start of a row is written.
const DEBUG_TIME_FORMAT: &str = "%H:%M:%S%.3f";

/// The blank that stands where a key row has its time, opening the row under
/// it that holds that key's bytes. Exactly as wide as a time written with
/// [`DEBUG_TIME_FORMAT`], so the columns line up.
const BYTES_ROW_GUTTER: &str = "            ";

/// How wide the column naming what a row is ("Key", "Bytes", "Mouse") is.
const WHAT_COLUMN_WIDTH: usize = 6;

/// How wide a key row's label column is at least: 26 cells is the hex of a
/// 9-byte key (two digits and a space per byte, less the last space), so
/// ordinary keys line up across entries. An entry whose label or hex is
/// wider widens both of its rows together.
pub(super) const LABEL_COLUMN_WIDTH: usize = 26;

/// What the bytes row says for a key crossterm decoded before the debugger
/// could read the bytes itself.
const BYTES_NOT_SEEN: &str = "(bytes not seen)";

/// What the key row says for a sequence that is no key dux can read.
const CANNOT_READ: &str = "dux cannot read this key";

/// How long a bracketed paste may go without a byte before the debugger
/// stops waiting for its end marker. A paste cut short (a dropped link, a
/// file that holds the start marker) would otherwise swallow every key,
/// Escape included.
const UNFINISHED_PASTE_WAIT: Duration = Duration::from_secs(1);

/// A bracketed paste still arriving through the debugger's reader.
#[derive(Debug)]
struct DebugPaste {
    /// Content bytes so far.
    len: usize,
    /// The content, kept only for a paste that began after the debugger
    /// closed, which is handed to the main UI as a paste.
    body: Option<Vec<u8>>,
    /// When the last content byte arrived.
    last: Instant,
}

/// The input debugger's raw reader: its own sequence splitter and the paste it
/// is counting, kept apart from the forwarding path's so an unfinished paste
/// or sequence never leaks from one to the other.
#[derive(Debug, Default)]
pub(crate) struct DebugRawInput {
    parser: crate::raw_input::RawInputParser,
    paste: Option<DebugPaste>,
    /// Whether the debugger has taken over reading stdin from crossterm.
    reading: bool,
}

impl DebugRawInput {
    /// Whether this reader holds input meant for the main UI and must keep
    /// reading until it can hand it over: a paste that began after the
    /// debugger closed and is still arriving, or the unfinished start of a
    /// sequence read with the closing Escape.
    pub(super) fn holds_input_for_main_ui(&self) -> bool {
        self.paste
            .as_ref()
            .is_some_and(|paste| paste.body.is_some())
            || !self.parser.pending().is_empty()
    }

    /// Split `bytes` into sequences. Empty input means the Escape ambiguity
    /// timed out: a lone Escape or `ESC ]` held back is released as its own
    /// sequence, and a lone `ESC [` or `ESC O` as `introducer` says. Two bare
    /// Escapes are one Alt-Esc, as a classic terminal sends it, when they
    /// arrive together with something after them or the timeout passes behind
    /// them; a second Escape that ends a read is held with the first, because
    /// it may start the next key. Inside a bracketed paste nothing is
    /// released: what is held is most likely the start of the end marker, and
    /// the paste's own quiet timeout settles it otherwise.
    fn split(&mut self, bytes: &[u8], introducer: LoneIntroducer) -> Vec<ParsedSequence> {
        let mut sequences = self.parser.feed_sequences(bytes);
        if bytes.is_empty() && !self.parser.in_bracket_paste() {
            sequences.extend(self.parser.resolve_pending_esc());
            if introducer == LoneIntroducer::Release && is_lone_introducer(self.parser.pending()) {
                let held = self.parser.pending().to_vec();
                self.parser.replace_pending(&[]);
                sequences.push(ParsedSequence {
                    bytes: held,
                    in_bracket_paste: self.parser.in_bracket_paste(),
                });
            }
        }
        let mut merged = Vec::with_capacity(sequences.len());
        let mut sequences = sequences.into_iter().peekable();
        while let Some(mut sequence) = sequences.next() {
            if sequence.bytes == [0x1b] {
                match sequences.peek() {
                    Some(next) if next.bytes == [0x1b] => {
                        sequences.next();
                        sequence.bytes.push(0x1b);
                    }
                    None if self.parser.pending() == [0x1b] && !self.parser.in_bracket_paste() => {
                        if bytes.is_empty() {
                            self.parser.replace_pending(&[]);
                            sequence.bytes.push(0x1b);
                        } else {
                            self.parser.replace_pending(ESCAPE_PAIR);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            merged.push(sequence);
        }
        merged
    }
}

/// What a debugger row starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowStart {
    /// The time it was logged: the first row of an entry.
    Now,
    /// The blank under that time: a key's bytes row.
    Gutter,
}

/// How a paste ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteEnd {
    /// Its end marker arrived.
    Marker,
    /// It stopped arriving before its end marker did.
    Abandoned,
}

/// Whether `line` is the bytes row under a key row, rather than the start of
/// an entry.
pub(super) fn is_bytes_row(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|span| span.content == BYTES_ROW_GUTTER)
}

/// The start every debugger row shares: the time (or, for a bytes row, the
/// blank that stands for it), then what the row is.
pub(super) fn debug_row_prefix(
    theme: &Theme,
    what: &'static str,
    start: RowStart,
) -> Vec<Span<'static>> {
    let time = match start {
        RowStart::Now => Span::styled(
            Local::now().format(DEBUG_TIME_FORMAT).to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ),
        RowStart::Gutter => Span::raw(BYTES_ROW_GUTTER),
    };
    vec![
        time,
        Span::raw(" │ "),
        Span::styled(
            pad_to_width(what, WHAT_COLUMN_WIDTH),
            Style::default().fg(theme.help_section_header_fg),
        ),
        Span::raw(" │ "),
    ]
}

/// Whether `pending` is a lone `ESC [` or `ESC O`: Alt-[ or Alt-Shift-O as a
/// classic terminal sends them, or the start of a longer sequence that never
/// came.
fn is_lone_introducer(pending: &[u8]) -> bool {
    pending == b"\x1b[" || pending == b"\x1bO"
}

/// What the ambiguity timeout does with a lone `ESC [` or `ESC O`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoneIntroducer {
    /// Release it as its own key, so the open debugger can show Alt-[ and
    /// Alt-Shift-O.
    Release,
    /// Hold it until the rest arrives, as crossterm and the forwarding path
    /// do once the debugger has closed.
    Hold,
}

/// Two Escapes held at the end of a read: Alt-Esc once the ambiguity timeout
/// passes, or an Escape and the start of the next key if more arrives.
const ESCAPE_PAIR: &[u8] = b"\x1b\x1b";

/// `bytes` in hex, one pair per byte.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `\u{..}` for a character that would draw as nothing: a control character
/// (C0, Delete or C1), a format character such as a direction override, or a
/// zero-width one. `None` for a character that draws as itself.
fn escape_invisible(ch: char) -> Option<String> {
    let invisible = ch.is_control() || display_width(ch.encode_utf8(&mut [0; 4])) == 0;
    invisible.then(|| format!("\\u{{{:x}}}", u32::from(ch)))
}

/// A key label with every character that would draw as nothing written as its
/// code point, so a key whose code point is a control character still shows.
fn escape_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| escape_invisible(ch).unwrap_or_else(|| ch.to_string()))
        .collect()
}

/// `bytes` in caret notation, as `cat -v` shows a control byte: C0 and
/// Delete in caret notation (`^[` for Escape, `^?` for Delete), and any
/// other character [`escape_invisible`] names in `\u{..}`. Printable text
/// stays itself, a byte that is not UTF-8 is shown as `\xNN`.
fn caret(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        for ch in chunk.valid().chars() {
            match u8::try_from(u32::from(ch)) {
                Ok(byte @ 0x00..=0x1f) => {
                    out.push('^');
                    out.push(char::from(byte + 0x40));
                }
                Ok(0x7f) => out.push_str("^?"),
                _ => match escape_invisible(ch) {
                    Some(escaped) => out.push_str(&escaped),
                    None => out.push(ch),
                },
            }
        }
        for byte in chunk.invalid() {
            let _ = write!(out, "\\x{byte:02x}");
        }
    }
    out
}

/// What the key row says about a key beyond its label: the kind, then what the
/// label leaves out (Hyper, Meta, and the lock states).
fn key_details(key: &KeyEvent) -> String {
    let kind = match key.kind {
        KeyEventKind::Press => "press",
        KeyEventKind::Repeat => "repeat",
        KeyEventKind::Release => "release",
    };
    let mut details = vec![kind];
    if key.modifiers.contains(KeyModifiers::HYPER) {
        details.push("Hyper");
    }
    if key.modifiers.contains(KeyModifiers::META) {
        details.push("Meta");
    }
    if key.state.contains(KeyEventState::CAPS_LOCK) {
        details.push("Caps Lock");
    }
    if key.state.contains(KeyEventState::NUM_LOCK) {
        details.push("Num Lock");
    }
    details.join(", ")
}

/// The extra sentence for a sequence dux cannot read, when it says something
/// a person can act on: xterm's modifyOtherKeys format (`CSI 27 ; m ; code
/// ~`), which tmux sends unless told otherwise. It names tmux as the sender
/// only when this run verified it runs inside tmux.
fn undecodable_hint(seq: &[u8], host: Host) -> Option<&'static str> {
    if !(seq.starts_with(b"\x1b[27;") && seq.ends_with(b"~")) {
        return None;
    }
    Some(match host {
        Host::Tmux => {
            "tmux sent this in its xterm format, which dux cannot read; with tmux 3.5 or newer, \
             set -s extended-keys-format csi-u"
        }
        Host::Terminal => {
            "this is xterm's modifyOtherKeys format, which dux cannot read; if it came through \
             tmux 3.5 or newer, set -s extended-keys-format csi-u there"
        }
    })
}

impl App {
    /// One poll of the input debugger's own reader, run instead of either
    /// input path while the debugger is open, or while it still holds input
    /// read after it closed (an arriving paste, an unfinished sequence). On
    /// the first poll it hands over from crossterm: every event crossterm
    /// already holds is handled first, because a poll of stdin cannot see
    /// crossterm's queue. Returns whether the app should exit.
    pub(super) fn poll_debug_raw_input(&mut self, stdin: impl AsFd) -> Result<bool> {
        if !self.debug_raw_input.reading {
            let held = || match crate::io_retry::retry_on_interrupt(|| event::poll(Duration::ZERO))
            {
                Ok(true) => crate::io_retry::retry_on_interrupt(event::read).ok(),
                _ => None,
            };
            if self.hand_debug_input_over(held) {
                return Ok(true);
            }
            if !self.input_debugger_open() {
                return Ok(false);
            }
        }

        // The same capped wait as the forwarding poll: this loop is the
        // background web server's engine servicer too.
        let fd = stdin.as_fd();
        let mut buf = [0u8; 4096];
        let poll_ms = 100u64.min(self.max_poll_ms());
        if !wait_for_input(&fd, Duration::from_millis(poll_ms))? {
            return self.resolve_pending_debug_esc(&fd, &mut buf);
        }
        let n = read_raw_chunk(fd, &mut buf)?;
        if n == 0 {
            return Ok(false);
        }
        self.mark_frame_dirty();
        if self.record_debug_input_bytes(&buf[..n]) {
            return Ok(true);
        }

        // Keep reading what is already there for one frame, so a paste is
        // counted whole before the next draw.
        let drain_deadline = Instant::now() + Duration::from_millis(16);
        while Instant::now() < drain_deadline
            && (self.input_debugger_open() || self.debug_raw_input.holds_input_for_main_ui())
        {
            if !wait_for_input(&fd, Duration::ZERO)? {
                break;
            }
            let n = read_raw_chunk(fd, &mut buf)?;
            if n == 0 {
                break;
            }
            if self.record_debug_input_bytes(&buf[..n]) {
                return Ok(true);
            }
        }
        self.resolve_pending_debug_esc(&fd, &mut buf)
    }

    /// Take over reading from crossterm: start a fresh reader and handle every
    /// event `next_held` still gives through the structured path, which
    /// records them with their bytes not seen. Stops early when one of them
    /// closes the debugger, leaving the rest to the structured path. Returns
    /// whether the app should exit.
    pub(super) fn hand_debug_input_over(
        &mut self,
        mut next_held: impl FnMut() -> Option<Event>,
    ) -> bool {
        self.debug_raw_input = DebugRawInput {
            reading: true,
            ..DebugRawInput::default()
        };
        while self.input_debugger_open() {
            let Some(event) = next_held() else {
                break;
            };
            if self.handle_terminal_event(event) {
                return true;
            }
        }
        false
    }

    /// Forget the debugger's reader: its unfinished sequence and any paste it
    /// was counting, which logs nothing.
    pub(super) fn end_debug_raw_input(&mut self) {
        self.debug_raw_input = DebugRawInput::default();
    }

    /// Run on every idle poll and after every drain: finish a paste whose end
    /// marker has not come for [`UNFINISHED_PASTE_WAIT`], then wait out the
    /// Escape ambiguity the way the forwarding poll does and resolve a lone
    /// Escape, a pair of them, or (while the debugger is open) a lone `ESC [`
    /// or `ESC O`, into the debugger or the main UI, never into a
    /// pty. Returns whether the app should exit.
    fn resolve_pending_debug_esc(&mut self, fd: &impl AsFd, buf: &mut [u8; 4096]) -> Result<bool> {
        if let Some(exit) = self.expire_unfinished_debug_paste() {
            return Ok(exit);
        }
        let debugger_open = self.input_debugger_open();
        let parser = &mut self.debug_raw_input.parser;
        parser.expire_held_host_reply(Instant::now());
        let resolves = parser.pending_resolves_on_timeout()
            || parser.pending() == ESCAPE_PAIR
            || (debugger_open && is_lone_introducer(parser.pending()))
            || (!debugger_open && settles_after_close(parser.pending()));
        if parser.in_bracket_paste() || !resolves {
            return Ok(false);
        }
        std::thread::sleep(ESC_AMBIGUITY_TIMEOUT);
        if wait_for_input(fd, Duration::ZERO)? {
            let n = read_raw_chunk(fd, buf)?;
            if n != 0 {
                return Ok(self.record_debug_input_bytes(&buf[..n]));
            }
        }
        Ok(self.record_debug_input_bytes(&[]))
    }

    /// Finish a paste that has gone [`UNFINISHED_PASTE_WAIT`] without a byte,
    /// with whatever the reader still holds as part of it: the debugger logs
    /// it as a paste whose end never came, and a paste meant for the main UI
    /// is handed over as it is, which also ends the reader. `None` when no
    /// paste has waited that long; otherwise whether the app should exit.
    fn expire_unfinished_debug_paste(&mut self) -> Option<bool> {
        let mut paste = self
            .debug_raw_input
            .paste
            .take_if(|paste| paste.last.elapsed() >= UNFINISHED_PASTE_WAIT)?;
        let parser = &mut self.debug_raw_input.parser;
        paste.len += parser.pending().len();
        if let Some(body) = paste.body.as_mut() {
            body.extend_from_slice(parser.pending());
        }
        parser.clear();
        let exit = self.finish_debug_paste(paste, PasteEnd::Abandoned);
        if !self.input_debugger_open() {
            self.end_debug_raw_input();
        }
        Some(exit)
    }

    /// Split raw input into sequences with the debugger's own splitter and
    /// hand each one to whoever it belongs to: an open paste, the debugger,
    /// or, once the debugger has closed, the reader that takes over from it.
    /// Bytes that follow the closing Escape in the same read go on to the
    /// forwarding path when it is next, and are otherwise replayed as the
    /// events crossterm would have made of them. A paste still open, or a
    /// sequence still unfinished, keeps this reader until it ends. Returns
    /// whether the app should exit.
    pub(super) fn record_debug_input_bytes(&mut self, bytes: &[u8]) -> bool {
        let introducer = if self.input_debugger_open() {
            LoneIntroducer::Release
        } else {
            LoneIntroducer::Hold
        };
        let mut sequences = self.debug_raw_input.split(bytes, introducer);
        let parser = &mut self.debug_raw_input.parser;
        if bytes.is_empty()
            && introducer == LoneIntroducer::Hold
            && !parser.in_bracket_paste()
            && settles_after_close(parser.pending())
        {
            // After the close only an escape fragment outlasts the timeout; a
            // leftover that starts none (a lone UTF-8 lead byte nothing
            // follows) is settled by it, replayed and dropped when no key.
            sequences.push(ParsedSequence {
                bytes: parser.pending().to_vec(),
                in_bracket_paste: false,
            });
            parser.replace_pending(&[]);
        }
        let mut tail = Vec::new();
        for sequence in sequences {
            let seq = sequence.bytes.as_slice();
            let exit = if self.debug_raw_input.paste.is_some() {
                self.feed_debug_paste(seq)
            } else if self.input_debugger_open() {
                self.record_debug_sequence(seq)
            } else if self.should_poll_raw_input() {
                tail.extend_from_slice(seq);
                false
            } else {
                self.replay_after_debugger_close(seq)
            };
            if exit {
                return true;
            }
        }
        if self.input_debugger_open() || self.debug_raw_input.paste.is_some() {
            return false;
        }
        if self.should_poll_raw_input() {
            tail.extend_from_slice(self.debug_raw_input.parser.pending());
        } else if !self.debug_raw_input.parser.pending().is_empty() {
            // The unfinished start of a sequence read with the closing Escape:
            // keep reading until the rest arrives, as crossterm would. What the
            // ambiguity timeout settles (a lone Escape, an `ESC ]`) the split
            // has already released, and a held host reply expires on its own.
            return false;
        }
        self.end_debug_raw_input();
        !tail.is_empty() && self.process_raw_input_bytes(&tail)
    }

    /// Record one complete raw sequence while the debugger is open. A paste
    /// start opens a paste to count; a mouse report does what a mouse event
    /// does in the debugger (the wheel scrolls, a click is logged); a focus
    /// report is logged and applied as the structured path applies it; a
    /// device report is a reply to dux's own query and is ignored; an
    /// unmodified Escape closes the debugger. Returns whether the app should
    /// exit.
    fn record_debug_sequence(&mut self, seq: &[u8]) -> bool {
        if seq == BRACKET_PASTE_START {
            self.open_debug_paste();
            return false;
        }
        if let Some(mouse) = crate::raw_input::parse_sgr_mouse(seq) {
            return self.handle_mouse(mouse);
        }
        if let Some(gained) = crate::raw_input::parse_focus_event(seq) {
            let row =
                self.debug_event_row("Focus", if gained { "Focus gained" } else { "Focus lost" });
            self.push_debug_rows(vec![row]);
            return self.handle_terminal_event(if gained {
                Event::FocusGained
            } else {
                Event::FocusLost
            });
        }
        if crate::raw_input::is_device_report(seq) {
            return false;
        }
        if let Some(key) = crate::key_decode::decode_key(seq) {
            self.record_debug_key(key, Some(seq));
        } else {
            let rows = self.debug_undecodable_rows(seq);
            self.push_debug_rows(rows);
        }
        false
    }

    /// One sequence read after the debugger closed, when crossterm reads
    /// next: handled as the event crossterm would have made of it. A paste
    /// start opens a paste this reader collects and hands over whole; a
    /// device report, or anything that is no key, is dropped, as crossterm
    /// drops it. Returns whether the app should exit.
    fn replay_after_debugger_close(&mut self, seq: &[u8]) -> bool {
        if seq == BRACKET_PASTE_START {
            self.open_debug_paste();
            return false;
        }
        let event = if let Some(mouse) = crate::raw_input::parse_sgr_mouse(seq) {
            Event::Mouse(mouse)
        } else if let Some(gained) = crate::raw_input::parse_focus_event(seq) {
            if gained {
                Event::FocusGained
            } else {
                Event::FocusLost
            }
        } else if crate::raw_input::is_device_report(seq) {
            return false;
        } else if let Some(key) = crate::key_decode::decode_key(seq) {
            Event::Key(key)
        } else {
            return false;
        };
        self.handle_terminal_event(event)
    }

    /// Start a paste. While the debugger is open it is counted; one that
    /// starts after the debugger closed keeps its content, for the main UI.
    fn open_debug_paste(&mut self) {
        self.debug_raw_input.paste = Some(DebugPaste {
            len: 0,
            body: (!self.input_debugger_open()).then(Vec::new),
            last: Instant::now(),
        });
    }

    /// One sequence inside an open paste: its end marker finishes the paste,
    /// anything else is content. Returns whether the app should exit.
    fn feed_debug_paste(&mut self, seq: &[u8]) -> bool {
        if seq == BRACKET_PASTE_END {
            return match self.debug_raw_input.paste.take() {
                Some(paste) => self.finish_debug_paste(paste, PasteEnd::Marker),
                None => false,
            };
        }
        if let Some(paste) = self.debug_raw_input.paste.as_mut() {
            paste.len += seq.len();
            if let Some(body) = paste.body.as_mut() {
                body.extend_from_slice(seq);
            }
            paste.last = Instant::now();
        }
        false
    }

    /// A paste that is over, because its end marker came or because it
    /// stopped arriving: a counted one is logged, one with content goes to
    /// the main UI as a paste. Returns whether the app should exit.
    fn finish_debug_paste(&mut self, paste: DebugPaste, end: PasteEnd) -> bool {
        match paste.body {
            Some(body) => self
                .handle_terminal_event(Event::Paste(String::from_utf8_lossy(&body).into_owned())),
            None if end == PasteEnd::Marker => {
                self.record_debug_paste(paste.len);
                false
            }
            None => {
                let row = self.debug_event_row(
                    "Paste",
                    &format!("{} bytes (its end marker never arrived)", paste.len),
                );
                self.push_debug_rows(vec![row]);
                false
            }
        }
    }

    /// Log a paste of `bytes` content bytes, which the debugger counts and
    /// never shows.
    pub(super) fn record_debug_paste(&mut self, bytes: usize) {
        let row = self.debug_event_row("Paste", &format!("{bytes} bytes"));
        self.push_debug_rows(vec![row]);
    }

    /// Log a key, or close the debugger on an unmodified Escape. The key came
    /// through crossterm (`bytes` is `None`: the hand-over, or a key queued
    /// before the debugger opened) or through the debugger's own reader,
    /// which passes the bytes it came as.
    pub(super) fn record_debug_key(&mut self, key: KeyEvent, bytes: Option<&[u8]>) {
        if closes_input_debugger(&key) {
            self.close_input_debugger();
            return;
        }
        let rows = self.debug_key_rows(key, bytes);
        self.push_debug_rows(rows);
    }

    pub(super) fn input_debugger_open(&self) -> bool {
        matches!(self.prompt, PromptState::DebugInput { .. })
    }

    /// Close the debugger. Its reader is ended by whoever reads next: the end
    /// of the read the closing key came in, which first hands the rest of
    /// that read on, or the next poll.
    fn close_input_debugger(&mut self) {
        self.prompt = PromptState::None;
    }

    /// Append rows to the open debugger's log, keeping its cap and pinning the
    /// newest row on screen.
    fn push_debug_rows(&mut self, rows: Vec<Line<'static>>) {
        if let PromptState::DebugInput {
            lines,
            scroll_offset,
        } = &mut self.prompt
        {
            lines.extend(rows);
            *scroll_offset = trim_debug_input_lines(lines);
        }
    }

    /// A one-row entry: the time, what it is, and what it says.
    fn debug_event_row(&self, what: &'static str, text: &str) -> Line<'static> {
        let mut spans = debug_row_prefix(&self.theme, what, RowStart::Now);
        spans.push(Span::styled(
            text.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        Line::from(spans)
    }

    /// What fires when `key` is pressed, in each scope: nothing for a
    /// release; for a key dux reads as another (Ctrl-`[` with extended keys
    /// on is Esc), that key and what it fires.
    fn debug_fired_text(&self, key: KeyEvent) -> String {
        let Some(acted) = crate::key_encode::acted_on_key(key, self.extended_keys) else {
            return "(none: no action fires on a release)".to_string();
        };
        let resolved: Vec<String> = BindingScope::ALL
            .iter()
            .filter_map(|&scope| {
                self.bindings
                    .lookup(&acted, scope)
                    .map(|action| format!("{}: {}", scope.display_name(), action.config_name()))
            })
            .collect();
        let fired = if resolved.is_empty() {
            "(none)".to_string()
        } else {
            resolved.join(", ")
        };
        if acted.code == key.code && acted.modifiers == key.modifiers {
            fired
        } else {
            format!("dux reads it as {}: {fired}", key_label(acted))
        }
    }

    /// The two rows of a key entry: the time, the label, what the label
    /// leaves out and what fires when it is pressed in each scope, then the
    /// bytes it came as.
    fn debug_key_rows(&self, key: KeyEvent, bytes: Option<&[u8]>) -> Vec<Line<'static>> {
        let label = escape_label(&key_label(key));
        let col = LABEL_COLUMN_WIDTH
            .max(display_width(&label))
            .max(bytes.map_or(0, |bytes| hex(bytes).len()));
        let mut spans = debug_row_prefix(&self.theme, "Key", RowStart::Now);
        spans.extend([
            Span::styled(
                pad_to_width(&label, col),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" │ "),
            Span::raw(key_details(&key)),
            Span::raw(" │ "),
            Span::styled(
                self.debug_fired_text(key),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]);
        vec![Line::from(spans), self.debug_bytes_row(bytes, col)]
    }

    /// The two rows of a sequence dux cannot read as a key.
    fn debug_undecodable_rows(&self, seq: &[u8]) -> Vec<Line<'static>> {
        let col = LABEL_COLUMN_WIDTH
            .max(display_width(CANNOT_READ))
            .max(hex(seq).len());
        let mut spans = debug_row_prefix(&self.theme, "Key", RowStart::Now);
        spans.push(Span::styled(
            pad_to_width(CANNOT_READ, col),
            Style::default().fg(self.theme.warning_fg),
        ));
        if let Some(hint) = undecodable_hint(seq, self.host_keys.host) {
            spans.push(Span::raw(" │ "));
            spans.push(Span::raw(hint));
        }
        vec![Line::from(spans), self.debug_bytes_row(Some(seq), col)]
    }

    /// The bytes row: hex padded to `col`, the width of the row above, then
    /// caret notation; or that the bytes were not seen.
    fn debug_bytes_row(&self, bytes: Option<&[u8]>, col: usize) -> Line<'static> {
        let mut spans = debug_row_prefix(&self.theme, "Bytes", RowStart::Gutter);
        match bytes {
            Some(bytes) => {
                spans.push(Span::raw(pad_to_width(&hex(bytes), col)));
                spans.push(Span::raw(" │ "));
                spans.push(Span::styled(
                    caret(bytes),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            None => spans.push(Span::styled(
                BYTES_NOT_SEEN,
                Style::default().add_modifier(Modifier::DIM),
            )),
        }
        Line::from(spans)
    }
}

/// Whether a leftover held after the debugger closed is settled by the
/// Escape ambiguity timeout rather than held until more bytes arrive: anything
/// that does not start an escape sequence.
fn settles_after_close(pending: &[u8]) -> bool {
    pending.first().is_some_and(|&byte| byte != 0x1b)
}

/// A key's label as the bindings write it.
fn key_label(key: KeyEvent) -> String {
    let combination = crokey::KeyCombination::from(key).normalized();
    crate::keybindings::display_format().to_string(combination)
}

/// An unmodified Escape closes the debugger, so no binding can trap anyone in
/// it; a modified one (Ctrl-Esc) is a key worth inspecting, a release is
/// recorded like any other, and so is Ctrl-`[`, which dux reads as Esc
/// elsewhere when extended keys are on (with them off it arrives as the
/// Escape byte itself, and closes).
fn closes_input_debugger(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc && key.modifiers.is_empty() && key.kind != KeyEventKind::Release
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use chrono::Local;
    use ratatui::text::{Line, Span};

    use super::{
        BYTES_ROW_GUTTER, DEBUG_TIME_FORMAT, DebugPaste, UNFINISHED_PASTE_WAIT, caret,
        escape_label, hex, key_details, key_label, undecodable_hint,
    };
    use crate::app::components::wrap_lines::display_width;
    use crate::app::input::{DEBUG_INPUT_MAX_LINES, trim_debug_input_lines};
    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::text_input::TextInput;
    use crate::app::{App, FocusPane, InputTarget, PromptState, RunInputReader};
    use crate::host_keys::{Host, TmuxReport};
    use crate::key_decode::decode_key;

    fn open(app: &mut App) {
        app.prompt = PromptState::DebugInput {
            lines: Vec::new(),
            scroll_offset: 0,
        };
    }

    fn log_rows(app: &App) -> Vec<String> {
        let PromptState::DebugInput { lines, .. } = &app.prompt else {
            panic!("expected the input debugger");
        };
        lines.iter().map(ToString::to_string).collect()
    }

    /// The display column the `n`th divider (from 0) of `row` starts at.
    fn divider_column(row: &str, n: usize) -> usize {
        let (at, _) = row
            .match_indices('│')
            .nth(n)
            .unwrap_or_else(|| panic!("divider {n} in {row:?}"));
        display_width(&row[..at])
    }

    #[test]
    fn bytes_show_in_hex_and_in_caret_notation() {
        assert_eq!(hex(b"\x1b[13;2u"), "1b 5b 31 33 3b 32 75");
        assert_eq!(caret(b"\x1b[13;2u"), "^[[13;2u");
        assert_eq!(caret(b"\x00\x0d\x7f"), "^@^M^?");
        assert_eq!(caret(b"\x07"), "^G");
        assert_eq!(caret("é".as_bytes()), "é");
        assert_eq!(caret(b"\xff"), "\\xff");
        // A C1 control and a direction override would draw as nothing.
        assert_eq!(caret("\u{85}".as_bytes()), "\\u{85}");
        assert_eq!(caret("\u{202e}".as_bytes()), "\\u{202e}");
        // A key whose code point is a control character shows it, rather
        // than the caret spelling of a different key.
        let bell = decode_key(b"\x1b[7u").expect("a csi-u key");
        assert_eq!(escape_label(&key_label(bell)), "\\u{7}");
    }

    #[test]
    fn the_xterm_format_hint_names_tmux_only_on_a_verified_tmux() {
        let xterm = b"\x1b[27;2;13~";
        let under_tmux = undecodable_hint(xterm, Host::Tmux).expect("a hint under tmux");
        assert!(under_tmux.starts_with("tmux sent this"), "{under_tmux}");
        let elsewhere = undecodable_hint(xterm, Host::Terminal).expect("a hint elsewhere");
        assert!(
            elsewhere.starts_with("this is xterm's modifyOtherKeys format"),
            "{elsewhere}"
        );
        assert!(!elsewhere.contains("tmux sent"), "{elsewhere}");
        let setting = "set -s extended-keys-format csi-u";
        let reason = TmuxReport::XtermFormat.reason().expect("a reason");
        for text in [under_tmux, elsewhere, reason.as_str()] {
            assert!(text.contains(setting), "{text}");
        }
        for host in [Host::Tmux, Host::Terminal] {
            assert!(undecodable_hint(b"\x1b[200;5u", host).is_none());
        }
    }

    #[test]
    fn the_details_name_the_kind_and_what_the_label_leaves_out() {
        // Shift, Hyper, Meta, Caps Lock and Num Lock (1 + 16 + 32 + 64 + 128).
        let release = decode_key(b"\x1b[97;241:3u").expect("a csi-u key");
        assert_eq!(
            key_details(&release),
            "release, Hyper, Meta, Caps Lock, Num Lock"
        );
        let repeat = decode_key(b"\x1b[97;1:2u").expect("a csi-u key");
        assert_eq!(key_details(&repeat), "repeat");
    }

    /// Both rows of an entry put their dividers in the same columns, a wide
    /// label included, a mouse row lines up with a key row, and ordinary
    /// keys of different byte lengths share their divider column.
    #[test]
    fn an_entry_s_two_rows_keep_their_dividers_in_line() {
        assert_eq!(
            display_width(&Local::now().format(DEBUG_TIME_FORMAT).to_string()),
            BYTES_ROW_GUTTER.len()
        );
        let mut app = test_app(default_bindings());
        open(&mut app);
        for seq in [
            &b"\x1b[13;2u"[..],
            b"\x1b[27;2;13~",
            "\x1b[12354u".as_bytes(),
        ] {
            app.record_debug_input_bytes(seq);
        }
        app.record_debug_input_bytes(b"\x1b[<0;5;5M");
        let rows = log_rows(&app);
        assert_eq!(rows.len(), 7, "{rows:#?}");
        assert!(rows[4].contains('あ'), "{rows:#?}");
        for entry in rows[..6].chunks(2) {
            assert_eq!(
                divider_column(&entry[0], 2),
                divider_column(&entry[1], 2),
                "{entry:#?}"
            );
        }
        assert!(rows[6].contains("Mouse"), "{rows:#?}");
        assert_eq!(divider_column(&rows[6], 1), divider_column(&rows[0], 1));

        app.record_debug_input_bytes(b"a");
        app.record_debug_input_bytes(b"\x01");
        let rows = log_rows(&app);
        assert_eq!(rows.len(), 11, "{rows:#?}");
        for row in [&rows[7], &rows[9]] {
            assert_eq!(
                divider_column(row, 2),
                divider_column(&rows[0], 2),
                "{rows:#?}"
            );
        }
    }

    /// A cut that lands between a key row and its bytes row takes the bytes
    /// row too, so the log never opens on half an entry.
    #[test]
    fn trimming_never_leaves_a_bytes_row_at_the_top() {
        let bytes_row = || Line::from(vec![Span::raw(BYTES_ROW_GUTTER), Span::raw(" │ ")]);
        let mut lines: Vec<Line<'static>> = Vec::new();
        for index in 0..=DEBUG_INPUT_MAX_LINES / 2 {
            lines.push(Line::raw(format!("key {index}")));
            lines.push(bytes_row());
        }
        lines.push(Line::raw("focus"));
        assert!(super::is_bytes_row(
            &lines[lines.len() - DEBUG_INPUT_MAX_LINES]
        ));
        let offset = trim_debug_input_lines(&mut lines);
        assert!(!super::is_bytes_row(&lines[0]), "{:?}", lines[0]);
        assert_eq!(lines[0].to_string(), "key 2");
        assert_eq!(usize::from(offset), lines.len());
    }

    /// A paste longer than one read is counted across reads as one paste.
    #[test]
    fn a_paste_split_across_reads_is_one_paste() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[200~abc");
        app.record_debug_input_bytes(b"defg");
        app.record_debug_input_bytes(b"\x1b[201~");
        let rows = log_rows(&app);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert!(
            rows[0].contains("Paste") && rows[0].contains("7 bytes"),
            "{rows:#?}"
        );
    }

    /// A lone `ESC [` (Alt-[ from a classic terminal) is its own entry once
    /// the ambiguity passes, never glued to the next key.
    #[test]
    fn a_lone_csi_introducer_is_released_on_the_timeout() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[");
        assert_eq!(log_rows(&app), Vec::<String>::new());
        app.record_debug_input_bytes(b"");
        app.record_debug_input_bytes(b"a");
        let rows = log_rows(&app);
        assert_eq!(rows.len(), 4, "{rows:#?}");
        assert!(rows[0].contains("dux cannot read this key"), "{rows:#?}");
        assert!(rows[1].contains("1b 5b "), "{rows:#?}");
        assert!(
            rows[3].contains("61 ") && !rows[3].contains("1b"),
            "{rows:#?}"
        );
    }

    /// Two Escapes with nothing after them once the ambiguity passes are
    /// Alt-Esc, which is recorded; an Escape before a mouse report is still
    /// an Escape, which closes.
    #[test]
    fn a_classic_alt_escape_is_recorded_rather_than_closing() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b\x1b");
        app.record_debug_input_bytes(b"");
        assert!(app.input_debugger_open());
        let rows = log_rows(&app);
        assert!(rows[0].contains("Alt-Esc"), "{rows:#?}");
        assert!(rows[1].contains("1b 1b "), "{rows:#?}");
        app.record_debug_input_bytes(b"\x1b\x1b[<0;1;1M");
        assert!(!app.input_debugger_open());
    }

    /// The bindings column says what fires: nothing on a release, and Esc's
    /// actions for Ctrl-[ with extended keys on, which does not close the
    /// debugger.
    #[test]
    fn the_bindings_column_says_what_fires() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[97;5:3u");
        let rows = log_rows(&app);
        assert!(
            rows[0].contains("release") && rows[0].contains("(none: no action fires on a release)"),
            "{rows:#?}"
        );

        app.set_extended_keys(crate::keybindings::ExtendedKeys::On);
        app.record_debug_input_bytes(b"\x1b[91;5u");
        assert!(app.input_debugger_open());
        let rows = log_rows(&app);
        assert!(rows[2].contains("dux reads it as Esc: "), "{rows:#?}");
    }

    /// Bytes after the closing Escape in the same read reach the main UI: a
    /// focus report is applied, and a paste is a paste, not keys.
    #[test]
    fn what_follows_the_closing_escape_reaches_the_main_ui() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        assert!(
            app.terminal_focus
                .should_stamp_viewed(Instant::now(), Duration::ZERO)
        );
        app.record_debug_input_bytes(b"\x1b[27u\x1b[O");
        assert!(!app.input_debugger_open());
        assert!(
            !app.terminal_focus
                .should_stamp_viewed(Instant::now(), Duration::ZERO),
            "the focus report reached the focus state"
        );

        // A newline in a paste joins the filter's text with a space; as keys
        // it would be an Enter.
        app.agent_filter = Some(TextInput::new());
        app.focus = FocusPane::Left;
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[27u\x1b[200~h\ni\x1b[201~");
        assert_eq!(
            app.agent_filter.as_ref().map(|f| f.text.as_str()),
            Some("h i")
        );

        // A paste whose end arrives in a later read is still one paste.
        app.agent_filter = Some(TextInput::new());
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[27u\x1b[200~h\ni");
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);
        app.record_debug_input_bytes(b"\x1b[201~");
        assert_eq!(
            app.agent_filter.as_ref().map(|f| f.text.as_str()),
            Some("h i")
        );
        assert!(app.debug_raw_input.paste.is_none());
        assert!(!app.debug_raw_input.reading);
    }

    /// A paste's end marker split across reads, with the ambiguity timeout
    /// passing between them, still ends the paste: the held start of the
    /// marker is never released as content.
    #[test]
    fn a_paste_end_marker_split_by_the_timeout_still_ends_the_paste() {
        for (first, second) in [(&b"\x1b["[..], &b"201~"[..]), (b"\x1b", b"[201~")] {
            let mut app = test_app(default_bindings());
            open(&mut app);
            app.record_debug_input_bytes(&[b"\x1b[200~abc", first].concat());
            app.record_debug_input_bytes(b"");
            app.record_debug_input_bytes(second);
            assert!(app.debug_raw_input.paste.is_none(), "{first:?}");
            let rows = log_rows(&app);
            assert_eq!(rows.len(), 1, "{rows:#?}");
            assert!(
                rows[0].contains("Paste") && rows[0].contains("3 bytes"),
                "{rows:#?}"
            );
            app.record_debug_input_bytes(b"\x1b");
            app.record_debug_input_bytes(b"");
            assert!(!app.input_debugger_open());
        }

        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::new());
        app.focus = FocusPane::Left;
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[27u\x1b[200~hi\x1b[");
        app.record_debug_input_bytes(b"");
        app.record_debug_input_bytes(b"201~");
        assert_eq!(
            app.agent_filter.as_ref().map(|f| f.text.as_str()),
            Some("hi")
        );
        assert!(app.debug_raw_input.paste.is_none());
    }

    /// A key split across reads right after the closing Escape reaches the
    /// main UI as that key, not as its last byte typed on its own.
    #[test]
    fn a_key_split_after_the_closing_escape_reaches_the_main_ui_whole() {
        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::with_text("ab".to_owned()));
        app.focus = FocusPane::Left;
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[27u\x1b[");
        assert!(!app.input_debugger_open());
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);
        app.record_debug_input_bytes(b"D");
        let filter = app.agent_filter.as_ref().expect("the filter");
        assert_eq!((filter.text.as_str(), filter.cursor), ("ab", 1));
        assert!(!app.debug_raw_input.reading);
    }

    /// Once the debugger has closed, an unfinished `ESC [` is held through
    /// the ambiguity timeout, as crossterm holds it, so the rest of the key
    /// still completes it.
    #[test]
    fn an_unfinished_key_after_the_close_outlasts_the_timeout() {
        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::with_text("ab".to_owned()));
        app.focus = FocusPane::Left;
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b[27u\x1b[");
        app.record_debug_input_bytes(b"");
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);
        app.record_debug_input_bytes(b"D");
        let filter = app.agent_filter.as_ref().expect("the filter");
        assert_eq!((filter.text.as_str(), filter.cursor), ("ab", 1));
    }

    /// A second Escape that ends a read can start the next key: the first
    /// Escape closes and the Left arrow reaches the main UI.
    #[test]
    fn a_second_escape_can_start_the_next_key() {
        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::with_text("ab".to_owned()));
        app.focus = FocusPane::Left;
        open(&mut app);
        app.record_debug_input_bytes(b"\x1b\x1b");
        app.record_debug_input_bytes(b"[D");
        assert!(!app.input_debugger_open());
        let filter = app.agent_filter.as_ref().expect("the filter");
        assert_eq!((filter.text.as_str(), filter.cursor), ("ab", 1));
    }

    /// The debugger reads before the agent under it, and so does a paste
    /// meant for the main UI that began after it closed.
    #[test]
    fn the_debugger_s_reader_wins_over_the_agent_under_it() {
        let mut app = test_app(default_bindings());
        app.input_target = InputTarget::Agent;
        open(&mut app);
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);

        app.prompt = PromptState::None;
        app.debug_raw_input.paste = Some(DebugPaste {
            len: 0,
            body: Some(Vec::new()),
            last: Instant::now(),
        });
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);
        app.debug_raw_input.paste = None;
        assert_eq!(app.run_input_reader(), RunInputReader::Forwarding);
    }

    /// The real reader, over a socket: a key is recorded with its bytes, and
    /// a lone Escape closes once the ambiguity passes.
    #[test]
    fn the_reader_records_a_key_and_a_lone_escape_closes() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        assert!(!app.hand_debug_input_over(|| None));
        let (reader, mut writer) = UnixStream::pair().expect("a socket pair");
        writer.write_all(b"\x1b[108;6u").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        let rows = log_rows(&app);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert!(rows[1].contains("1b 5b 31 30 38 3b 36 75"), "{rows:#?}");
        writer.write_all(b"\x1b").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert!(!app.input_debugger_open());
    }

    /// A paste whose end marker never comes is finished after a quiet
    /// second, so Escape closes the debugger again.
    #[test]
    fn an_unfinished_paste_is_logged_after_a_quiet_second() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        assert!(!app.hand_debug_input_over(|| None));
        let (reader, mut writer) = UnixStream::pair().expect("a socket pair");
        writer.write_all(b"\x1b[200~abc").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        let paste = app.debug_raw_input.paste.as_mut().expect("a paste is open");
        assert_eq!(paste.len, 3);
        paste.last -= UNFINISHED_PASTE_WAIT;
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert!(app.debug_raw_input.paste.is_none());
        let rows = log_rows(&app);
        let last = rows.last().expect("a row");
        assert!(
            last.contains("Paste") && last.contains("3 bytes (its end marker never arrived)"),
            "{rows:#?}"
        );
        writer.write_all(b"\x1b").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert!(!app.input_debugger_open());
    }

    /// A paste that began after the closing Escape and never ends is still
    /// handed to the main UI as a paste, and the reader then lets go.
    #[test]
    fn an_unfinished_paste_after_the_close_is_delivered_as_a_paste() {
        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::new());
        app.focus = FocusPane::Left;
        open(&mut app);
        assert!(!app.hand_debug_input_over(|| None));
        let (reader, mut writer) = UnixStream::pair().expect("a socket pair");
        writer.write_all(b"\x1b[27u\x1b[200~h\ni").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert!(!app.input_debugger_open());
        assert_eq!(app.run_input_reader(), RunInputReader::Debugger);
        app.debug_raw_input
            .paste
            .as_mut()
            .expect("a paste is open")
            .last -= UNFINISHED_PASTE_WAIT;
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert_eq!(
            app.agent_filter.as_ref().map(|f| f.text.as_str()),
            Some("h i")
        );
        assert!(app.debug_raw_input.paste.is_none());
        assert!(!app.debug_raw_input.reading);
    }

    /// A leftover after the close that is no escape fragment (a lone UTF-8
    /// lead byte no continuation follows) is settled by the ambiguity
    /// timeout and the reader lets go, so the next keys are not swallowed
    /// as its continuation bytes.
    #[test]
    fn a_stray_lead_byte_after_the_close_is_settled_by_the_timeout() {
        let mut app = test_app(default_bindings());
        app.agent_filter = Some(TextInput::new());
        app.focus = FocusPane::Left;
        open(&mut app);
        assert!(!app.hand_debug_input_over(|| None));
        let (reader, mut writer) = UnixStream::pair().expect("a socket pair");
        writer.write_all(b"\x1b[27u\xe9").expect("write");
        assert!(!app.poll_debug_raw_input(&reader).expect("poll"));
        assert!(!app.input_debugger_open());
        // The reader lets go, so the main UI's own reader takes the next keys.
        assert!(!app.debug_raw_input.reading);
        assert_eq!(app.run_input_reader(), RunInputReader::Structured);
        for c in ['a', 'b'] {
            let key = crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c));
            assert!(!app.handle_terminal_event(crossterm::event::Event::Key(key)));
        }
        assert_eq!(
            app.agent_filter.as_ref().map(|f| f.text.as_str()),
            Some("ab")
        );
    }
}
