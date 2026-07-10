//! Pure, PTY-independent scanner for the "needs attention" and progress signals
//! that agent CLIs embed in their raw terminal output.
//!
//! dux runs each agent inside an embedded terminal emulator. That emulator hands
//! us a ready-made [`alacritty_terminal::event::Event::Bell`] for the classic
//! terminal ding, but it silently drops the richer OSC notification codes
//! (`OSC 9`, `OSC 777`) and the `OSC 9;4` progress report. To see those we scan
//! the raw byte stream ourselves, just before feeding it to the emulator.
//!
//! This module is deliberately free of any PTY, terminal, or engine dependency so
//! it can be exhaustively unit-tested at the byte level. It is a small streaming
//! state machine:
//!
//! - `ESC ] 9 ; <message> (BEL | ESC \)` and `ESC ] 777 ; notify ; ...` are
//!   attention notifications ([`AttentionEvent::Notify`]).
//! - `ESC ] 9 ; 4 ; <state> ; <pct> (BEL | ESC \)` is a progress report
//!   ([`AttentionEvent::Progress`]); it is NEVER attention. States 1 (working
//!   with a value) and 3 (working indeterminate) mean busy; 0 (done/idle) and
//!   every other state mean idle.
//! - Agents running under tmux wrap their escape codes in an outer
//!   `ESC P tmux ; <payload with every ESC doubled> ESC \` passthrough envelope;
//!   the scanner unwraps it and scans the inner content.
//! - A sequence can be split across two reads, so the scanner carries an
//!   unterminated trailing sequence to the next [`AttentionScanner::scan`] call,
//!   bounded by [`MAX_CARRY`] so a garbage stream can never grow it without
//!   bound.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Maximum number of bytes the scanner retains across chunk boundaries while
/// waiting for an escape sequence to complete. A partial sequence longer than
/// this is abandoned (its carried prefix dropped) so a stream that looks like an
/// endless unterminated OSC cannot grow the carry buffer without bound. Real
/// notification and progress sequences are far shorter than this.
const MAX_CARRY: usize = 4096;

/// A signal extracted from an agent's raw output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionEvent {
    /// An `OSC 9` / `OSC 777` desktop-notification style sequence: the agent is
    /// asking for the user's attention (permission prompt, finished turn, ...).
    Notify,
    /// An `OSC 9;4` progress report. `working` is `true` while the agent reports
    /// itself busy (states 1 and 3) and `false` when it reports done/idle
    /// (state 0) or any other state. This feeds the "working" indicator and is
    /// never treated as an attention request.
    Progress { working: bool },
}

/// Streaming scanner. One instance lives in each PTY reader loop and is fed every
/// chunk of raw output in order. It carries an unterminated trailing sequence
/// across calls so a code split between two reads is still recognized.
#[derive(Default)]
pub struct AttentionScanner {
    /// The tail of the previous chunk that ended in the middle of an escape
    /// sequence, prepended to the next chunk. Bounded by [`MAX_CARRY`].
    carry: Vec<u8>,
}

impl AttentionScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan the next chunk of raw output and return every complete signal found.
    /// Any trailing partial sequence is carried to the next call.
    pub fn scan(&mut self, data: &[u8]) -> Vec<AttentionEvent> {
        let mut events = Vec::new();

        // Prepend the carried partial (if any) to this chunk.
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(data);

        let consumed = scan_buf(&buf, &mut events);
        let tail = &buf[consumed..];

        // Retain the unconsumed tail as the new carry, unless it has grown past
        // the cap (a runaway unterminated sequence): drop it rather than let it
        // accumulate. The next real `ESC` will resync the state machine.
        if tail.len() <= MAX_CARRY {
            self.carry.clear();
            self.carry.extend_from_slice(tail);
        } else {
            self.carry.clear();
        }

        events
    }
}

/// Scan a complete-or-partial buffer, pushing every complete event. Returns the
/// number of bytes consumed from the front; the remainder (from the returned
/// index onward) is an incomplete trailing sequence the caller should carry.
fn scan_buf(buf: &[u8], events: &mut Vec<AttentionEvent>) -> usize {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != ESC {
            i += 1;
            continue;
        }
        // buf[i] == ESC. Need the following byte to know the sequence type.
        let Some(&kind) = buf.get(i + 1) else {
            return i; // incomplete: carry from this ESC.
        };
        match kind {
            b']' => match find_osc_terminator(&buf[i + 2..]) {
                Some((payload_len, term_len)) => {
                    let payload = &buf[i + 2..i + 2 + payload_len];
                    if let Some(ev) = classify_osc(payload) {
                        events.push(ev);
                    }
                    i += 2 + payload_len + term_len;
                }
                None => return i, // incomplete OSC: carry from this ESC.
            },
            b'P' => match find_st(&buf[i + 2..]) {
                Some((inner_len, term_len)) => {
                    let inner = &buf[i + 2..i + 2 + inner_len];
                    if let Some(unwrapped) = tmux_unwrap(inner) {
                        // The unwrapped payload is complete (bounded by the DCS
                        // terminator), so scan it fully and ignore its carry.
                        scan_buf(&unwrapped, events);
                    }
                    i += 2 + inner_len + term_len;
                }
                None => return i, // incomplete DCS: carry from this ESC.
            },
            // Any other escape sequence is irrelevant. Skip just the ESC and keep
            // scanning; the next iteration resyncs on the following byte.
            _ => i += 1,
        }
    }
    buf.len()
}

/// Locate the terminator of an OSC sequence (`BEL` or `ESC \`) within `s`.
/// Returns `(payload_len, terminator_len)` where `payload_len` is the number of
/// bytes before the terminator. Returns `None` if no complete terminator is
/// present yet.
fn find_osc_terminator(s: &[u8]) -> Option<(usize, usize)> {
    let mut j = 0;
    while j < s.len() {
        match s[j] {
            BEL => return Some((j, 1)),
            ESC => match s.get(j + 1) {
                Some(b'\\') => return Some((j, 2)),
                Some(_) => j += 1,   // stray ESC inside the payload; skip it.
                None => return None, // ESC at the very end: need more bytes.
            },
            _ => j += 1,
        }
    }
    None
}

/// Locate a String Terminator (`ESC \`) within `s`, used for the DCS/tmux
/// envelope. Returns `(inner_len, terminator_len)` or `None` if incomplete.
fn find_st(s: &[u8]) -> Option<(usize, usize)> {
    let mut j = 0;
    while j < s.len() {
        if s[j] == ESC {
            match s.get(j + 1) {
                Some(b'\\') => return Some((j, 2)),
                Some(_) => j += 1,
                None => return None,
            }
        } else {
            j += 1;
        }
    }
    None
}

/// Unwrap a tmux passthrough envelope. `inner` is the DCS content (everything
/// between `ESC P` and the terminating `ESC \`). tmux passthrough looks like
/// `tmux;<payload>` with every `ESC` in the payload doubled; this strips the
/// `tmux;` prefix and un-doubles the escapes. Returns `None` when `inner` is not
/// a tmux passthrough.
fn tmux_unwrap(inner: &[u8]) -> Option<Vec<u8>> {
    let payload = inner.strip_prefix(b"tmux;")?;
    let mut out = Vec::with_capacity(payload.len());
    let mut j = 0;
    while j < payload.len() {
        if payload[j] == ESC && payload.get(j + 1) == Some(&ESC) {
            out.push(ESC);
            j += 2;
        } else {
            out.push(payload[j]);
            j += 1;
        }
    }
    Some(out)
}

/// Classify an OSC payload (the bytes between `ESC ]` and its terminator).
fn classify_osc(payload: &[u8]) -> Option<AttentionEvent> {
    let (cmd, rest) = split_once(payload, b';');
    match cmd {
        b"9" => {
            let rest = rest?; // bare "9" with no message: nothing to do.
            let (first, prog_rest) = split_once(rest, b';');
            if first == b"4" {
                // OSC 9;4 progress: the state is the field after "4;".
                let state = prog_rest
                    .map(|r| split_once(r, b';').0)
                    .unwrap_or(b"" as &[u8]);
                let working = state == b"1" || state == b"3";
                Some(AttentionEvent::Progress { working })
            } else if rest.is_empty() {
                None
            } else {
                Some(AttentionEvent::Notify)
            }
        }
        b"777" => match rest {
            Some(r) if r.starts_with(b"notify") => Some(AttentionEvent::Notify),
            _ => None,
        },
        _ => None,
    }
}

/// Split `s` at the first occurrence of `sep`. Returns the head and, if `sep`
/// was present, the tail after it (`None` when `sep` is absent).
fn split_once(s: &[u8], sep: u8) -> (&[u8], Option<&[u8]>) {
    match s.iter().position(|&b| b == sep) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_once(bytes: &[u8]) -> Vec<AttentionEvent> {
        AttentionScanner::new().scan(bytes)
    }

    #[test]
    fn osc9_notify_bel_terminated() {
        assert_eq!(
            scan_once(b"\x1b]9;Claude needs your permission\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc9_notify_st_terminated() {
        assert_eq!(
            scan_once(b"\x1b]9;done\x1b\\"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc777_notify() {
        assert_eq!(
            scan_once(b"\x1b]777;notify;Title;Body\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc777_non_notify_ignored() {
        assert_eq!(scan_once(b"\x1b]777;something;else\x07"), vec![]);
    }

    #[test]
    fn osc9_empty_message_ignored() {
        assert_eq!(scan_once(b"\x1b]9;\x07"), vec![]);
    }

    #[test]
    fn osc94_progress_working_state1() {
        assert_eq!(
            scan_once(b"\x1b]9;4;1;50\x07"),
            vec![AttentionEvent::Progress { working: true }]
        );
    }

    #[test]
    fn osc94_progress_indeterminate_state3() {
        assert_eq!(
            scan_once(b"\x1b]9;4;3;0\x07"),
            vec![AttentionEvent::Progress { working: true }]
        );
    }

    #[test]
    fn osc94_progress_idle_state0() {
        assert_eq!(
            scan_once(b"\x1b]9;4;0;0\x07"),
            vec![AttentionEvent::Progress { working: false }]
        );
    }

    #[test]
    fn osc94_progress_error_state2_is_idle() {
        assert_eq!(
            scan_once(b"\x1b]9;4;2;0\x07"),
            vec![AttentionEvent::Progress { working: false }]
        );
    }

    #[test]
    fn osc94_never_emits_notify() {
        // The 9;4 progress code shares the OSC 9 prefix but must never flag
        // attention.
        let events = scan_once(b"\x1b]9;4;1;10\x07");
        assert!(!events.contains(&AttentionEvent::Notify));
    }

    #[test]
    fn plain_text_around_notify() {
        assert_eq!(
            scan_once(b"hello world\x1b]9;ping\x07more text"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn multiple_events_one_chunk() {
        assert_eq!(
            scan_once(b"\x1b]9;4;1;0\x07\x1b]9;ping\x07\x1b]9;4;0;0\x07"),
            vec![
                AttentionEvent::Progress { working: true },
                AttentionEvent::Notify,
                AttentionEvent::Progress { working: false },
            ]
        );
    }

    #[test]
    fn split_across_two_chunks() {
        let mut scanner = AttentionScanner::new();
        // Split right in the middle of the payload.
        assert_eq!(scanner.scan(b"\x1b]9;Cla"), vec![]);
        assert_eq!(
            scanner.scan(b"ude needs you\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn split_right_after_esc() {
        let mut scanner = AttentionScanner::new();
        assert_eq!(scanner.scan(b"text\x1b"), vec![]);
        assert_eq!(scanner.scan(b"]9;hi\x07"), vec![AttentionEvent::Notify]);
    }

    #[test]
    fn split_st_terminator_across_chunks() {
        let mut scanner = AttentionScanner::new();
        // The ESC of the ST lands at the end of the first chunk.
        assert_eq!(scanner.scan(b"\x1b]9;msg\x1b"), vec![]);
        assert_eq!(scanner.scan(b"\\rest"), vec![AttentionEvent::Notify]);
    }

    #[test]
    fn tmux_wrapped_notify() {
        // ESC P tmux; <ESC doubled> ] 9 ; msg BEL ESC \
        let bytes = b"\x1bPtmux;\x1b\x1b]9;wrapped\x07\x1b\\";
        assert_eq!(scan_once(bytes), vec![AttentionEvent::Notify]);
    }

    #[test]
    fn tmux_wrapped_progress() {
        let bytes = b"\x1bPtmux;\x1b\x1b]9;4;1;20\x07\x1b\\";
        assert_eq!(
            scan_once(bytes),
            vec![AttentionEvent::Progress { working: true }]
        );
    }

    #[test]
    fn garbage_flood_bounds_carry() {
        // A long unterminated OSC must not grow the carry without bound and must
        // not panic.
        let mut scanner = AttentionScanner::new();
        let mut junk = vec![ESC, b']'];
        junk.extend(std::iter::repeat_n(b'x', 100_000));
        assert_eq!(scanner.scan(&junk), vec![]);
        assert!(scanner.carry.len() <= MAX_CARRY);
        // A real notification after the flood still parses once terminated.
        assert_eq!(
            scanner.scan(b"\x07\x1b]9;after\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn incomplete_sequence_never_double_emits() {
        let mut scanner = AttentionScanner::new();
        // Feed one byte at a time; the event must fire exactly once.
        let bytes = b"\x1b]9;hi\x07";
        let mut total = Vec::new();
        for b in bytes {
            total.extend(scanner.scan(&[*b]));
        }
        assert_eq!(total, vec![AttentionEvent::Notify]);
    }

    #[test]
    fn unrelated_osc_ignored() {
        // OSC 0 (window title) and OSC 11 (color query) must not flag attention.
        assert_eq!(scan_once(b"\x1b]0;my title\x07"), vec![]);
        assert_eq!(scan_once(b"\x1b]11;?\x07"), vec![]);
    }
}
