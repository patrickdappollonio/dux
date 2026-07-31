//! Pure, PTY-independent scanner for the "needs attention" and progress signals
//! that agent CLIs embed in their raw terminal output.
//!
//! dux runs each agent inside an embedded terminal emulator. That emulator would
//! ordinarily hand us a ready-made [`alacritty_terminal::event::Event::Bell`] for
//! the classic terminal ding, but it has no event at all for the richer OSC
//! notification codes (`OSC 9`, `OSC 777`) or the `OSC 9;4` progress report:
//! alacritty's `Event` carries a bell, a title/icon change, a clipboard store, a
//! colour request and a PTY write, and nothing else, so those sequences reaching
//! its parser are simply consumed. Scanning the raw byte stream ourselves, just
//! before feeding it to the emulator, is therefore the ONLY detection path for
//! them. The bell is scanned here too rather than taken from `Event::Bell`, so
//! that one signal set comes from one mechanism and cannot double-fire.
//!
//! This module is deliberately free of any PTY, terminal, or engine dependency so
//! it can be exhaustively unit-tested at the byte level. It is a small streaming
//! state machine:
//!
//! - A bare `BEL` (`0x07`) encountered OUTSIDE any escape sequence is the classic
//!   terminal ding ([`AttentionEvent::Bell`]). A `BEL` that terminates an OSC, or
//!   that sits inside a DCS/tmux envelope, is structural and never a ding.
//! - `ESC ] 9 ; <message> (BEL | ESC \)` and `ESC ] 777 ; notify ; ...` are
//!   attention notifications ([`AttentionEvent::Notify`]).
//! - `ESC ] 9 ; 4 ; <state> ; <pct> (BEL | ESC \)` is a progress report
//!   ([`AttentionEvent::Progress`]); it is NEVER attention. States 1 (working
//!   with a value) and 3 (working indeterminate) mean busy; 0 (done/idle) and
//!   every other state mean idle. To distinguish a real progress report from a
//!   notification whose free text merely begins `4;`, the `<state>` field must be
//!   a short (1-2 char) ASCII-digit token; otherwise the whole thing is a Notify.
//!   A residual ambiguity remains and is inherent to the shared OSC 9 prefix: a
//!   notification body that literally begins `4;<one-or-two-digits>[;...]` is
//!   indistinguishable from progress and is classified as progress. Real notify
//!   bodies from Claude/Codex are prose, so this collision is vanishingly rare.
//! - Agents running under tmux wrap their escape codes in an outer
//!   `ESC P tmux ; <payload with every ESC doubled> ESC \` passthrough envelope;
//!   the scanner unwraps it and scans the inner content.
//! - A sequence can be split across two reads, so the scanner carries an
//!   unterminated trailing sequence to the next [`AttentionScanner::scan`] call,
//!   bounded by [`MAX_CARRY`] so a garbage stream can never grow it without
//!   bound. The carry remembers how far the terminator search already got (and
//!   for which sequence kind) so a slow-dripping unterminated sequence is scanned
//!   once, not re-scanned from the start on every chunk.
//!
//! # Trust boundary
//!
//! These signals are read from whatever the agent writes to its terminal, so any
//! content the agent displays (a file it `cat`s, a tool's output) that happens to
//! contain these escape codes can forge or mask an attention/progress signal.
//! This is inherent to terminal escape codes: a real terminal would pop the same
//! desktop notification for the same bytes. The blast radius is bounded by the
//! attention config switches and by the engine's short progress-authority window.

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
    /// A bare terminal bell (`0x07`) outside any escape sequence: the classic
    /// ding, config-gated by `attention_on_bell`. This is the ONLY bell path
    /// (the emulator's own `Event::Bell` is deliberately not handled, see
    /// `EventProxy::send_event`), so the bell cannot be armed twice for one ding
    /// and one signal set stays on one mechanism.
    Bell,
    /// An `OSC 9` / `OSC 777` desktop-notification style sequence: the agent is
    /// asking for the user's attention (permission prompt, finished turn, ...).
    Notify,
    /// An `OSC 9;4` progress report. `working` is `true` while the agent reports
    /// itself busy (states 1 and 3) and `false` when it reports done/idle
    /// (state 0) or any other state. This feeds the "working" indicator and is
    /// never treated as an attention request.
    Progress { working: bool },
}

/// The class of a captured passthrough sequence, so the engine can gate each kind
/// independently (clipboard writes are forwarded more conservatively than
/// notifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedKind {
    /// An `OSC 9` / `OSC 777` desktop notification.
    Notify,
    /// An `OSC 99` kitty-notification-protocol sequence (any part, including `d=0`
    /// continuations and `p=close`, so a multi-part notification stays intact).
    KittyNotify,
    /// An `OSC 9;4` progress report.
    Progress,
    /// An `OSC 52` clipboard SET (never a `?` read).
    ClipboardSet,
}

/// A whitelisted escape sequence captured verbatim (in canonical `ESC ] … ESC \`
/// form) for forwarding to the host terminal. Distinct from [`AttentionEvent`]:
/// events drive dux's own in-app attention chrome, captures are the raw bytes
/// replayed to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSeq {
    pub kind: CapturedKind,
    pub bytes: Vec<u8>,
}

/// Which kind of sequence the scanner is mid-way through when it carries an
/// incomplete tail across a chunk boundary. Recorded alongside the resume offset
/// so the offset is only reapplied when the next chunk re-derives the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarryKind {
    /// `ESC ]` operating-system-command.
    Osc,
    /// `ESC P` device-control string (the tmux passthrough envelope).
    Dcs,
}

/// The resume state for a carried, still-incomplete leading sequence: the kind of
/// sequence and how many payload bytes (after the two-byte `ESC <kind>` header)
/// have already been searched for a terminator without finding one.
#[derive(Debug, Clone, Copy)]
struct CarryResume {
    kind: CarryKind,
    /// Payload-relative offset (bytes after `ESC <kind>`) already scanned.
    searched: usize,
}

/// The result of searching a payload for its terminator.
enum TermScan {
    /// Terminator found: `payload_len` bytes precede it, and it is `term_len`
    /// bytes long.
    Found { payload_len: usize, term_len: usize },
    /// No terminator yet. `safe_offset` is the payload-relative index from which a
    /// future search can safely resume (never past a trailing lone `ESC`, which
    /// may still become `ESC \`).
    Incomplete { safe_offset: usize },
}

/// Streaming scanner. One instance lives in each PTY reader loop and is fed every
/// chunk of raw output in order. It carries an unterminated trailing sequence
/// across calls so a code split between two reads is still recognized.
#[derive(Default)]
pub struct AttentionScanner {
    /// The tail of the previous chunk that ended in the middle of an escape
    /// sequence, prepended to the next chunk. Bounded by [`MAX_CARRY`].
    carry: Vec<u8>,
    /// Where the terminator search for the carried leading sequence left off, so
    /// a slow-dripping unterminated sequence is scanned once rather than
    /// re-scanned from index 0 on every chunk. `None` when there is no carry, or
    /// when only a lone `ESC` (kind not yet known) is carried.
    resume: Option<CarryResume>,
    /// How many times a runaway (over-[`MAX_CARRY`]) partial sequence has been
    /// dropped. Purely observability: the reader loop reads this to log a drop.
    overflow_drops: u64,
}

impl AttentionScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan the next chunk of raw output and return every complete signal found.
    /// Any trailing partial sequence is carried to the next call.
    pub fn scan(&mut self, data: &[u8]) -> Vec<AttentionEvent> {
        self.scan_full(data, None)
    }

    /// Like [`AttentionScanner::scan`], but additionally appends every whitelisted
    /// passthrough sequence (in canonical form) to `capture` when it is `Some`.
    /// The `capture = None` path is byte-for-byte identical to `scan`.
    pub fn scan_full(
        &mut self,
        data: &[u8],
        capture: Option<&mut Vec<CapturedSeq>>,
    ) -> Vec<AttentionEvent> {
        let mut events = Vec::new();

        // Prepend the carried partial (if any) to this chunk.
        let mut buf = std::mem::take(&mut self.carry);
        let resume = self.resume.take();
        buf.extend_from_slice(data);

        let (consumed, carry_state) = scan_buf(&buf, &mut events, resume, capture);
        let tail = &buf[consumed..];

        // Retain the unconsumed tail as the new carry, unless it has grown past
        // the cap (a runaway unterminated sequence): drop it rather than let it
        // accumulate. The next real `ESC` will resync the state machine.
        if tail.len() <= MAX_CARRY {
            self.carry.clear();
            self.carry.extend_from_slice(tail);
            self.resume = carry_state;
        } else {
            self.carry.clear();
            self.resume = None;
            self.overflow_drops = self.overflow_drops.saturating_add(1);
        }

        events
    }

    /// Total number of runaway partial sequences dropped so far. The reader loop
    /// compares this across calls to log a debug line when a drop happens; the
    /// scanner itself stays pure and never logs.
    pub fn overflow_drops(&self) -> u64 {
        self.overflow_drops
    }
}

/// Scan a complete-or-partial buffer, pushing every complete event. `resume`, when
/// present, describes an incomplete sequence carried from the previous call that
/// begins at index 0 of `buf`; its terminator search resumes from the recorded
/// offset (only when the kind still matches). Returns the number of bytes consumed
/// from the front and, if the tail is an incomplete sequence, the resume state to
/// carry for it.
fn scan_buf(
    buf: &[u8],
    events: &mut Vec<AttentionEvent>,
    resume: Option<CarryResume>,
    mut capture: Option<&mut Vec<CapturedSeq>>,
) -> (usize, Option<CarryResume>) {
    let mut i = 0;
    while i < buf.len() {
        // A bare BEL outside any sequence is the classic ding. (A BEL that
        // terminates an OSC is consumed inside `find_osc_terminator`, and a BEL
        // inside a DCS payload is consumed with the envelope, so neither reaches
        // this top-level scan.)
        if buf[i] == BEL {
            events.push(AttentionEvent::Bell);
            i += 1;
            continue;
        }
        if buf[i] != ESC {
            i += 1;
            continue;
        }
        // buf[i] == ESC. Need the following byte to know the sequence type.
        let Some(&kind) = buf.get(i + 1) else {
            return (i, None); // incomplete lone ESC: carry, kind unknown.
        };
        match kind {
            b']' => {
                let start = resume_offset(resume, CarryKind::Osc, i);
                match find_osc_terminator(&buf[i + 2..], start) {
                    TermScan::Found {
                        payload_len,
                        term_len,
                    } => {
                        let payload = &buf[i + 2..i + 2 + payload_len];
                        if let Some(ev) = classify_osc(payload) {
                            events.push(ev);
                        }
                        if let Some(cap) = capture.as_deref_mut()
                            && let Some(seq) = capture_osc(payload)
                        {
                            cap.push(seq);
                        }
                        i += 2 + payload_len + term_len;
                    }
                    TermScan::Incomplete { safe_offset } => {
                        return (
                            i,
                            Some(CarryResume {
                                kind: CarryKind::Osc,
                                searched: safe_offset,
                            }),
                        );
                    }
                }
            }
            b'P' => {
                let start = resume_offset(resume, CarryKind::Dcs, i);
                match find_st(&buf[i + 2..], start) {
                    TermScan::Found {
                        payload_len: inner_len,
                        term_len,
                    } => {
                        let inner = &buf[i + 2..i + 2 + inner_len];
                        if let Some(unwrapped) = tmux_unwrap(inner) {
                            // The unwrapped payload is complete (bounded by the DCS
                            // terminator), so scan it fully and ignore its carry.
                            // Capture from the unwrapped bytes so what we record is
                            // canonical/unwrapped (dux re-wraps on forward).
                            scan_buf(&unwrapped, events, None, capture.as_deref_mut());
                        }
                        i += 2 + inner_len + term_len;
                    }
                    TermScan::Incomplete { safe_offset } => {
                        return (
                            i,
                            Some(CarryResume {
                                kind: CarryKind::Dcs,
                                searched: safe_offset,
                            }),
                        );
                    }
                }
            }
            // Any other escape sequence is irrelevant. Skip just the ESC and keep
            // scanning; the next iteration resyncs on the following byte.
            _ => i += 1,
        }
    }
    (buf.len(), None)
}

/// The payload-relative offset from which to resume the terminator search for the
/// sequence at `esc_index`. The carried resume state only applies to the leading
/// sequence (index 0) and only when its recorded kind matches; otherwise the
/// search starts fresh at 0.
fn resume_offset(resume: Option<CarryResume>, kind: CarryKind, esc_index: usize) -> usize {
    match resume {
        Some(r) if esc_index == 0 && r.kind == kind => r.searched,
        _ => 0,
    }
}

/// Locate the terminator of an OSC sequence (`BEL` or `ESC \`) within `s`, starting
/// the search at payload-relative index `start` (bytes before `start` were already
/// searched on an earlier call and contained no terminator).
fn find_osc_terminator(s: &[u8], start: usize) -> TermScan {
    let mut j = start.min(s.len());
    while j < s.len() {
        match s[j] {
            BEL => {
                return TermScan::Found {
                    payload_len: j,
                    term_len: 1,
                };
            }
            ESC => match s.get(j + 1) {
                Some(b'\\') => {
                    return TermScan::Found {
                        payload_len: j,
                        term_len: 2,
                    };
                }
                Some(_) => j += 1, // stray ESC inside the payload; skip it.
                // ESC at the very end: resume from it next time, since it may
                // still become `ESC \`.
                None => return TermScan::Incomplete { safe_offset: j },
            },
            _ => j += 1,
        }
    }
    TermScan::Incomplete {
        safe_offset: s.len(),
    }
}

/// Locate a String Terminator (`ESC \`) within `s`, used for the DCS/tmux
/// envelope. Resumes from `start` like [`find_osc_terminator`].
fn find_st(s: &[u8], start: usize) -> TermScan {
    let mut j = start.min(s.len());
    while j < s.len() {
        if s[j] == ESC {
            match s.get(j + 1) {
                Some(b'\\') => {
                    return TermScan::Found {
                        payload_len: j,
                        term_len: 2,
                    };
                }
                Some(_) => j += 1,
                None => return TermScan::Incomplete { safe_offset: j },
            }
        } else {
            j += 1;
        }
    }
    TermScan::Incomplete {
        safe_offset: s.len(),
    }
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
                // OSC 9;4 ConEmu progress shares the OSC 9 prefix with plain
                // notifications. Only treat it as progress when the state field is
                // a short ASCII-digit token; anything else (a notify body that
                // merely starts with `4;`, or a bare `9;4` with no state) is a
                // notification.
                match prog_rest.map(|r| split_once(r, b';').0) {
                    Some(state) if is_progress_state(state) => {
                        let working = state == b"1" || state == b"3";
                        Some(AttentionEvent::Progress { working })
                    }
                    _ => Some(AttentionEvent::Notify),
                }
            } else if rest.is_empty() {
                None
            } else {
                Some(AttentionEvent::Notify)
            }
        }
        b"99" => {
            // OSC 99 is the kitty notification protocol: `99;<metadata>;<body>`
            // with colon-separated `key=value` metadata. It counts as attention
            // only for a FINAL notification (`d` absent or `d=1`) whose payload is
            // displayable text (`p` absent, `p=title`, or `p=body`); continuations
            // (`d=0`) and control parts (`p=close`/`?`/`icon`/`buttons`) are not.
            let rest = rest?;
            let (metadata, _body) = split_once(rest, b';');
            kitty_notify_is_attention(metadata).then_some(AttentionEvent::Notify)
        }
        b"777" => match rest {
            Some(r) if r.starts_with(b"notify") => Some(AttentionEvent::Notify),
            _ => None,
        },
        _ => None,
    }
}

/// Whether an OSC 99 metadata field (the colon-separated `key=value` part between
/// the `99;` and the body) denotes a final, displayable notification: `d` absent or
/// `d=1`, and `p` absent or `title`/`body`.
fn kitty_notify_is_attention(metadata: &[u8]) -> bool {
    let mut d_final = true;
    let mut p_ok = true;
    for token in metadata.split(|&b| b == b':') {
        let (key, value) = split_once(token, b'=');
        match key {
            b"d" => d_final = value != Some(b"0"),
            b"p" => p_ok = matches!(value, None | Some(b"title") | Some(b"body")),
            _ => {}
        }
    }
    d_final && p_ok
}

/// Whether an OSC 99 metadata field is a `p=?` query (which must never be captured
/// or forwarded, since a reply would be typed back into dux).
fn kitty_notify_is_query(metadata: &[u8]) -> bool {
    metadata.split(|&b| b == b':').any(|token| {
        let (key, value) = split_once(token, b'=');
        key == b"p" && value == Some(b"?")
    })
}

/// Classify an OSC payload for host PASSTHROUGH capture. Broader than
/// [`classify_osc`]: it also captures progress reports, clipboard SETs, and every
/// non-query OSC 99 part (including `d=0` continuations and `p=close`, for protocol
/// integrity). Returns the canonical `ESC ] <payload> ESC \` bytes to forward.
fn capture_osc(payload: &[u8]) -> Option<CapturedSeq> {
    // Refuse to capture any payload carrying a C0 control byte (< 0x20). The
    // canonical bytes are replayed verbatim to the host terminal, so an embedded
    // CR/LF/ESC could break out of the sequence or reposition the host cursor.
    // The whitelisted kinds legitimately contain none: progress is digits and
    // semicolons, clipboard SET is base64, and a real notification body is a
    // single prose line. A payload with a C0 byte is malformed or hostile, so it
    // is dropped from passthrough (its in-app attention classification may still
    // fire; only the raw forward is refused).
    if payload.iter().any(|&b| b < 0x20) {
        return None;
    }
    let (cmd, rest) = split_once(payload, b';');
    let kind = match cmd {
        b"9" => {
            let rest = rest?;
            let (first, prog_rest) = split_once(rest, b';');
            if first == b"4" {
                match prog_rest.map(|r| split_once(r, b';').0) {
                    Some(state) if is_progress_state(state) => CapturedKind::Progress,
                    // A `9;4` that is not a well-formed progress report is a
                    // notification whose text merely begins `4;`.
                    _ => CapturedKind::Notify,
                }
            } else if rest.is_empty() {
                return None;
            } else {
                CapturedKind::Notify
            }
        }
        b"99" => {
            // Capture every OSC 99 part except a `p=?` query, so a multi-part
            // notification (continuations, then close) reaches the host intact.
            let rest = rest?;
            let (metadata, _body) = split_once(rest, b';');
            if kitty_notify_is_query(metadata) {
                return None;
            }
            CapturedKind::KittyNotify
        }
        b"777" => match rest {
            Some(r) if r.starts_with(b"notify") => CapturedKind::Notify,
            _ => return None,
        },
        b"52" => {
            // `52;<selection>;<data>`. Forward SET only; never a `?` read, whose
            // reply would land in dux's own stdin and be typed at the prompt.
            let rest = rest?;
            let (_selection, data) = split_once(rest, b';');
            match data {
                Some(d) if d != b"?" => CapturedKind::ClipboardSet,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(CapturedSeq {
        kind,
        bytes: canonical_osc(payload),
    })
}

/// Rebuild an OSC payload into its canonical `ESC ] <payload> ESC \` byte form,
/// regardless of the terminator the agent originally used, so forwarded sequences
/// are uniform.
fn canonical_osc(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(ESC);
    out.push(b']');
    out.extend_from_slice(payload);
    out.push(ESC);
    out.push(b'\\');
    out
}

/// Wrap raw escape-sequence bytes in a tmux passthrough envelope: the inverse of
/// [`tmux_unwrap`]. `ESC P tmux ;` + the payload with every `ESC` doubled + `ESC \`.
/// dux uses this to re-wrap the sequences it captured (and unwrapped) so they pass
/// through the tmux dux itself runs under.
pub fn tmux_wrap(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 9);
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in bytes {
        if b == ESC {
            out.push(ESC);
            out.push(ESC);
        } else {
            out.push(b);
        }
    }
    out.push(ESC);
    out.push(b'\\');
    out
}

/// Whether `s` is a well-formed OSC 9;4 progress state: a 1-2 character ASCII-digit
/// token. This is the structural check that keeps a notification whose free text
/// begins `4;` from being swallowed as progress.
fn is_progress_state(s: &[u8]) -> bool {
    (1..=2).contains(&s.len()) && s.iter().all(|b| b.is_ascii_digit())
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
        // The BEL here terminates the OSC, so it is NOT a bare ding.
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
    fn osc94_two_digit_state_is_progress() {
        // A 2-digit numeric state is still a well-formed progress report (and any
        // state other than 1/3 reads as idle).
        assert_eq!(
            scan_once(b"\x1b]9;4;10;50\x07"),
            vec![AttentionEvent::Progress { working: false }]
        );
    }

    #[test]
    fn osc9_notify_body_starting_four_is_not_progress() {
        // A notification whose text begins "4;" but whose "state" is non-numeric
        // must be a Notify, not swallowed as progress.
        assert_eq!(
            scan_once(b"\x1b]9;4;hello there\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc9_notify_body_four_space_text_is_notify() {
        // "9;4 tests failing" has no second ';', so "first" is "4 tests failing"
        // (not "4") and it is a plain notification.
        assert_eq!(
            scan_once(b"\x1b]9;4 tests failing\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc9_bare_four_no_state_is_notify() {
        // Documented edge: a bare "9;4" with no state field is not a well-formed
        // progress report, so it classifies as a notification.
        assert_eq!(scan_once(b"\x1b]9;4\x07"), vec![AttentionEvent::Notify]);
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
    fn bare_bell_emits_bell() {
        assert_eq!(scan_once(b"\x07"), vec![AttentionEvent::Bell]);
        assert_eq!(scan_once(b"ding\x07here"), vec![AttentionEvent::Bell]);
    }

    #[test]
    fn bell_terminating_osc_does_not_emit_bell() {
        // The BEL is the OSC terminator, so exactly one Notify (no Bell).
        assert_eq!(scan_once(b"\x1b]9;hi\x07"), vec![AttentionEvent::Notify]);
    }

    #[test]
    fn bell_inside_dcs_payload_does_not_emit_bell() {
        // A 0x07 inside a (non-tmux) DCS envelope is structural, not a ding.
        assert_eq!(scan_once(b"\x1bP1;2q\x07\x1b\\"), vec![]);
    }

    #[test]
    fn bell_detected_across_chunk_split() {
        let mut scanner = AttentionScanner::new();
        assert_eq!(scanner.scan(b"a"), vec![]);
        assert_eq!(scanner.scan(b"\x07"), vec![AttentionEvent::Bell]);
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
        // not panic. The drop is counted for observability.
        let mut scanner = AttentionScanner::new();
        let mut junk = vec![ESC, b']'];
        junk.extend(std::iter::repeat_n(b'x', 100_000));
        assert_eq!(scanner.scan(&junk), vec![]);
        assert!(scanner.carry.len() <= MAX_CARRY);
        assert_eq!(scanner.overflow_drops(), 1);
        // A real notification after the flood still parses once terminated.
        assert_eq!(
            scanner.scan(b"\x1b]9;after\x07"),
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
    fn slow_drip_long_osc_fires_once() {
        // A long OSC dripped one byte at a time (R1): the resume offset means the
        // terminator search never rescans the carried prefix, and the event still
        // fires exactly once, at the end. Kept under MAX_CARRY so it is never
        // dropped.
        let mut scanner = AttentionScanner::new();
        let mut bytes = b"\x1b]9;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', 3990));
        bytes.push(BEL);
        assert!(bytes.len() <= MAX_CARRY);
        let mut total = Vec::new();
        for b in &bytes {
            total.extend(scanner.scan(&[*b]));
        }
        assert_eq!(total, vec![AttentionEvent::Notify]);
        assert_eq!(scanner.overflow_drops(), 0);
    }

    #[test]
    fn unrelated_osc_ignored() {
        // OSC 0 (window title) and OSC 11 (color query) must not flag attention.
        assert_eq!(scan_once(b"\x1b]0;my title\x07"), vec![]);
        assert_eq!(scan_once(b"\x1b]11;?\x07"), vec![]);
    }

    // --- OSC 99 (kitty notification protocol) classification ---

    #[test]
    fn osc99_bare_body_is_notify() {
        assert_eq!(
            scan_once(b"\x1b]99;;Build finished\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc99_title_and_body_are_notify() {
        assert_eq!(
            scan_once(b"\x1b]99;p=title;Hi\x1b\\"),
            vec![AttentionEvent::Notify]
        );
        assert_eq!(
            scan_once(b"\x1b]99;d=1:p=body;Details\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn osc99_continuation_and_control_are_not_notify() {
        // d=0 is a continuation, not the final notification.
        assert_eq!(scan_once(b"\x1b]99;d=0;partial\x07"), vec![]);
        // p=close tears down a notification; p=? is a query. Neither is attention.
        assert_eq!(scan_once(b"\x1b]99;p=close;\x07"), vec![]);
        assert_eq!(scan_once(b"\x1b]99;p=?;\x07"), vec![]);
    }

    #[test]
    fn osc99_inside_tmux_envelope_is_notify() {
        let bytes = b"\x1bPtmux;\x1b\x1b]99;;wrapped\x07\x1b\\";
        assert_eq!(scan_once(bytes), vec![AttentionEvent::Notify]);
    }

    // --- capture sink ---

    fn capture_once(bytes: &[u8]) -> Vec<CapturedSeq> {
        let mut cap = Vec::new();
        AttentionScanner::new().scan_full(bytes, Some(&mut cap));
        cap
    }

    #[test]
    fn capture_notify_progress_and_clipboard() {
        let caps =
            capture_once(b"\x1b]9;done\x07\x1b]9;4;1;50\x07\x1b]52;c;aGVsbG8=\x07\x1b]99;;hi\x07");
        let kinds: Vec<CapturedKind> = caps.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                CapturedKind::Notify,
                CapturedKind::Progress,
                CapturedKind::ClipboardSet,
                CapturedKind::KittyNotify,
            ]
        );
        // Canonical ST-terminated form regardless of the original terminator.
        assert_eq!(caps[0].bytes, b"\x1b]9;done\x1b\\");
    }

    #[test]
    fn capture_canonicalizes_bel_to_st() {
        let caps = capture_once(b"\x1b]9;ping\x07");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].bytes, b"\x1b]9;ping\x1b\\".to_vec());
    }

    #[test]
    fn capture_rejects_c0_control_bytes_in_payload() {
        // An OSC 9 whose body carries an embedded CR/LF must not be captured for
        // host forwarding (a control byte could break out of the sequence).
        assert!(capture_once(b"\x1b]9;line1\rline2\x07").is_empty());
        assert!(capture_once(b"\x1b]9;line1\nline2\x07").is_empty());
        // Attention classification is independent of capture: the same bytes still
        // register as a Notify for dux's own in-app chrome.
        assert_eq!(
            scan_once(b"\x1b]9;line1\rline2\x07"),
            vec![AttentionEvent::Notify]
        );
    }

    #[test]
    fn capture_never_records_clipboard_read() {
        // OSC 52 read (`?`) must never be captured (a reply would misroute).
        assert!(capture_once(b"\x1b]52;c;?\x07").is_empty());
    }

    #[test]
    fn capture_never_records_kitty_query() {
        assert!(capture_once(b"\x1b]99;p=?;\x07").is_empty());
    }

    #[test]
    fn capture_records_kitty_continuation_and_close() {
        // Continuations and close are captured for protocol integrity even though
        // they are not attention events.
        let caps = capture_once(b"\x1b]99;d=0;part\x07\x1b]99;p=close;\x07");
        assert_eq!(caps.len(), 2);
        assert!(caps.iter().all(|c| c.kind == CapturedKind::KittyNotify));
    }

    #[test]
    fn capture_ignores_unrelated_osc() {
        // Title, color query, and hyperlinks are not passthrough-captured.
        assert!(capture_once(b"\x1b]0;title\x07").is_empty());
        assert!(capture_once(b"\x1b]8;;https://x\x07").is_empty());
    }

    #[test]
    fn capture_from_inside_tmux_envelope_is_unwrapped() {
        let caps = capture_once(b"\x1bPtmux;\x1b\x1b]9;msg\x07\x1b\\");
        assert_eq!(caps.len(), 1);
        // Captured bytes are the canonical unwrapped sequence.
        assert_eq!(caps[0].bytes, b"\x1b]9;msg\x1b\\".to_vec());
    }

    #[test]
    fn capture_none_path_matches_scan() {
        // The capture=None path must be byte-identical to plain scan.
        let mut a = AttentionScanner::new();
        let mut b = AttentionScanner::new();
        let data = b"\x1b]9;hi\x07\x1b]9;4;1;0\x07text";
        assert_eq!(a.scan(data), b.scan_full(data, None));
    }

    #[test]
    fn tmux_wrap_is_inverse_of_unwrap() {
        let payload = b"\x1b]9;hello\x1b\\".to_vec();
        let wrapped = tmux_wrap(&payload);
        // Strip the ESC P and trailing ESC \ to get the DCS inner content.
        let inner = &wrapped[2..wrapped.len() - 2];
        assert_eq!(tmux_unwrap(inner), Some(payload));
    }
}
