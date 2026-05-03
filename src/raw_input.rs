use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// Bracket paste mode start marker: `ESC [ 200 ~`
pub const BRACKET_PASTE_START: &[u8] = b"\x1b[200~";
/// Bracket paste mode end marker: `ESC [ 201 ~`
pub const BRACKET_PASTE_END: &[u8] = b"\x1b[201~";

/// Returns `true` if the byte sequence is an SGR mouse event (`\x1b[<…M` or
/// `\x1b[<…m`).
pub fn is_sgr_mouse(seq: &[u8]) -> bool {
    seq.len() >= 6
        && seq[0] == 0x1b
        && seq[1] == b'['
        && seq[2] == b'<'
        && matches!(seq.last(), Some(b'M' | b'm'))
}

/// Parse an SGR mouse sequence into a crossterm `MouseEvent`.
///
/// SGR format: `ESC [ < Cb ; Cx ; Cy {M|m}`
///   - Cb = button/modifier bits
///   - Cx = column (1-based)
///   - Cy = row (1-based)
///   - M = press/motion, m = release
pub fn parse_sgr_mouse(seq: &[u8]) -> Option<MouseEvent> {
    if !is_sgr_mouse(seq) {
        return None;
    }

    let final_byte = *seq.last()?;
    // Extract the parameter string between '<' and the final byte.
    let params = std::str::from_utf8(&seq[3..seq.len() - 1]).ok()?;
    let mut parts = params.split(';');
    let cb: u16 = parts.next()?.parse().ok()?;
    let cx: u16 = parts.next()?.parse().ok()?;
    let cy: u16 = parts.next()?.parse().ok()?;

    // Convert 1-based to 0-based coordinates.
    let column = cx.saturating_sub(1);
    let row = cy.saturating_sub(1);

    let is_release = final_byte == b'm';
    let is_motion = cb & 32 != 0;
    let button_bits = cb & 0b11000011; // mask out motion bit (bit 5) and modifier bits (4,3)

    // Extract modifier keys from SGR button byte (bits 2, 3, 4).
    let mut modifiers = KeyModifiers::empty();
    if cb & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 16 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }

    let kind = if cb & 64 != 0 {
        // Scroll events.
        match button_bits & 0x03 {
            0 => MouseEventKind::ScrollUp,
            1 => MouseEventKind::ScrollDown,
            2 => MouseEventKind::ScrollLeft,
            3 => MouseEventKind::ScrollRight,
            _ => return None,
        }
    } else if is_release {
        let button = match button_bits & 0x03 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => return None,
        };
        MouseEventKind::Up(button)
    } else if is_motion {
        let button = match button_bits & 0x03 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            3 => {
                return Some(MouseEvent {
                    kind: MouseEventKind::Moved,
                    column,
                    row,
                    modifiers,
                });
            }
            _ => return None,
        };
        MouseEventKind::Drag(button)
    } else {
        let button = match button_bits & 0x03 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => return None,
        };
        MouseEventKind::Down(button)
    };

    Some(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

/// Rewrite an SGR mouse sequence with coordinates translated by the given
/// screen offset.
///
/// `origin_col` and `origin_row` are the 0-based screen position of the
/// terminal content area's top-left corner (i.e. `term_area.x` and
/// `term_area.y` from the layout).
///
/// The SGR wire format uses 1-based coordinates, so a screen click at
/// column `origin_col + 1` (the first content column) maps to translated
/// column 1. Returns `None` if the sequence is not a valid SGR mouse event
/// or the click falls on or before the origin (i.e. outside the content
/// area).
pub fn translate_sgr_mouse(seq: &[u8], origin_col: u16, origin_row: u16) -> Option<Vec<u8>> {
    if !is_sgr_mouse(seq) {
        return None;
    }
    let final_byte = *seq.last()?;
    let params = std::str::from_utf8(&seq[3..seq.len() - 1]).ok()?;
    let mut parts = params.split(';');
    let cb: u16 = parts.next()?.parse().ok()?;
    let cx: u16 = parts.next()?.parse().ok()?; // 1-based screen column
    let cy: u16 = parts.next()?.parse().ok()?; // 1-based screen row

    // Translate: wire values are 1-based and origin is 0-based, so
    // translated = wire - origin. A result of 0 means the click landed on
    // the border/header (before content column/row 1), so reject it.
    let tx = cx.checked_sub(origin_col)?;
    let ty = cy.checked_sub(origin_row)?;
    if tx == 0 || ty == 0 {
        return None;
    }

    Some(format!("\x1b[<{cb};{tx};{ty}{}", final_byte as char).into_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceStatus {
    Complete(usize),
    Incomplete,
}

/// Incremental parser for terminal input bytes.
///
/// Terminals encode a bare Escape, Alt-key chords, CSI/OSC/SS3 controls,
/// bracket paste markers, UTF-8, and SGR mouse reports on the same byte
/// stream. This parser keeps incomplete trailing bytes in one place so
/// callers do not need to splice buffers manually or guess whether a pending
/// `ESC` is standalone.
#[derive(Debug, Clone, Default)]
pub struct RawInputParser {
    pending: Vec<u8>,
    in_bracket_paste: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSequence {
    pub bytes: Vec<u8>,
    pub in_bracket_paste: bool,
}

impl RawInputParser {
    pub fn feed_sequences(&mut self, bytes: &[u8]) -> Vec<ParsedSequence> {
        self.pending.extend_from_slice(bytes);
        let mut sequences = Vec::new();

        loop {
            if self.pending.is_empty() {
                break;
            }
            match scan_one_sequence(&self.pending) {
                SequenceStatus::Complete(len) => {
                    let bytes: Vec<u8> = self.pending.drain(..len).collect();
                    let in_bracket_paste = self.in_bracket_paste;
                    if bytes == BRACKET_PASTE_START {
                        self.in_bracket_paste = true;
                    } else if bytes == BRACKET_PASTE_END {
                        self.in_bracket_paste = false;
                    }
                    sequences.push(ParsedSequence {
                        bytes,
                        in_bracket_paste,
                    });
                }
                SequenceStatus::Incomplete => break,
            }
        }

        sequences
    }

    pub fn resolve_pending_esc(&mut self) -> Option<Vec<u8>> {
        if self.pending == [0x1b] {
            self.pending.clear();
            Some(vec![0x1b])
        } else {
            None
        }
    }

    pub fn in_bracket_paste(&self) -> bool {
        self.in_bracket_paste
    }

    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    pub fn replace_pending(&mut self, bytes: &[u8]) {
        self.pending.clear();
        self.pending.extend_from_slice(bytes);
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.in_bracket_paste = false;
    }
}

fn scan_one_sequence(buf: &[u8]) -> SequenceStatus {
    if buf.is_empty() {
        return SequenceStatus::Incomplete;
    }

    let b = buf[0];
    if b == 0x1b {
        // ESC — start of an escape sequence.
        if buf.len() < 2 {
            // Bare ESC at end of buffer — could be incomplete.
            return SequenceStatus::Incomplete;
        }

        match buf[1] {
            0x1b => {
                // A bare ESC followed by another escape sequence. Keep the
                // first ESC as its own sequence so the second ESC can still
                // prefix CSI/OSC/SS3 input such as SGR mouse reports.
                SequenceStatus::Complete(1)
            }
            b'[' => {
                let mut i = 2;
                loop {
                    if i >= buf.len() {
                        return SequenceStatus::Incomplete;
                    }
                    let c = buf[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&c) {
                        return SequenceStatus::Complete(i);
                    }
                }
            }
            b'O' => {
                if buf.len() < 3 {
                    SequenceStatus::Incomplete
                } else {
                    SequenceStatus::Complete(3)
                }
            }
            b']' => {
                let mut i = 2;
                loop {
                    if i >= buf.len() {
                        return SequenceStatus::Incomplete;
                    }
                    if buf[i] == 0x07 {
                        return SequenceStatus::Complete(i + 1);
                    }
                    if buf[i] == 0x1b {
                        if i + 1 >= buf.len() {
                            return SequenceStatus::Incomplete;
                        }
                        if buf[i + 1] == b'\\' {
                            return SequenceStatus::Complete(i + 2);
                        }
                        // Malformed OSC: complete the bytes before the new ESC
                        // and let the outer parser reconsider that ESC next.
                        return SequenceStatus::Complete(i);
                    }
                    i += 1;
                }
            }
            _ => SequenceStatus::Complete(2),
        }
    } else if (0xc0..0xfe).contains(&b) {
        let expected = if b < 0xe0 {
            2
        } else if b < 0xf0 {
            3
        } else {
            4
        };
        if expected <= buf.len() {
            SequenceStatus::Complete(expected)
        } else {
            SequenceStatus::Incomplete
        }
    } else {
        SequenceStatus::Complete(1)
    }
}

/// Splits a raw byte buffer into complete terminal input sequences.
///
/// Returns `(complete, remainder)` where `complete` is a list of byte-slice
/// ranges representing fully received sequences and `remainder` is the
/// trailing bytes that could be the start of an incomplete sequence (e.g. a
/// partial CSI or UTF-8 character).
///
/// The splitter does **not** interpret what sequences mean — it only finds
/// boundaries so that each sequence can be matched against intercepted
/// bindings or forwarded to the PTY verbatim.
pub fn split_sequences(buf: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut sequences: Vec<&[u8]> = Vec::new();
    let mut i = 0;

    while i < buf.len() {
        let start = i;
        match scan_one_sequence(&buf[start..]) {
            SequenceStatus::Complete(len) => {
                i += len;
                sequences.push(&buf[start..i]);
            }
            SequenceStatus::Incomplete => return (sequences, &buf[start..]),
        }
    }

    (sequences, &buf[buf.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_printable_ascii() {
        let (seqs, rem) = split_sequences(b"a");
        assert_eq!(seqs, vec![b"a".as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn single_control_char() {
        let (seqs, rem) = split_sequences(b"\x07");
        assert_eq!(seqs, vec![b"\x07".as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn del_byte() {
        let (seqs, rem) = split_sequences(b"\x7f");
        assert_eq!(seqs, vec![b"\x7f".as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn utf8_two_byte() {
        let input = "ñ".as_bytes(); // 0xc3 0xb1
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input]);
        assert!(rem.is_empty());
    }

    #[test]
    fn utf8_three_byte() {
        let input = "€".as_bytes(); // 0xe2 0x82 0xac
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input]);
        assert!(rem.is_empty());
    }

    #[test]
    fn utf8_four_byte() {
        let input = "𐍈".as_bytes(); // 4-byte UTF-8
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input]);
        assert!(rem.is_empty());
    }

    #[test]
    fn csi_cursor_up() {
        let input = b"\x1b[A";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input.as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn csi_page_up() {
        let input = b"\x1b[5~";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input.as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn csi_modified_key() {
        // Ctrl+Right: ESC [ 1 ; 5 C
        let input = b"\x1b[1;5C";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input.as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn ss3_f1() {
        let input = b"\x1bOP";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input.as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn alt_key() {
        let input = b"\x1ba";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![input.as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn bare_esc_at_end() {
        let (seqs, rem) = split_sequences(b"\x1b");
        assert!(seqs.is_empty());
        assert_eq!(rem, b"\x1b");
    }

    #[test]
    fn incomplete_csi() {
        // ESC [ 5 — missing final byte
        let input = b"\x1b[5";
        let (seqs, rem) = split_sequences(input);
        assert!(seqs.is_empty());
        assert_eq!(rem, input.as_slice());
    }

    #[test]
    fn incomplete_utf8() {
        // First byte of a 2-byte UTF-8 sequence, missing continuation
        let input = b"\xc3";
        let (seqs, rem) = split_sequences(input);
        assert!(seqs.is_empty());
        assert_eq!(rem, input.as_slice());
    }

    #[test]
    fn incomplete_ss3() {
        // ESC O — missing the third byte
        let input = b"\x1bO";
        let (seqs, rem) = split_sequences(input);
        assert!(seqs.is_empty());
        assert_eq!(rem, input.as_slice());
    }

    #[test]
    fn mixed_buffer() {
        // 'a' + Ctrl-G + CSI Up + UTF-8 ñ
        let mut input = Vec::new();
        input.push(b'a');
        input.push(0x07);
        input.extend_from_slice(b"\x1b[A");
        input.extend_from_slice("ñ".as_bytes());

        let (seqs, rem) = split_sequences(&input);
        assert_eq!(seqs.len(), 4);
        assert_eq!(seqs[0], b"a");
        assert_eq!(seqs[1], b"\x07");
        assert_eq!(seqs[2], b"\x1b[A");
        assert_eq!(seqs[3], "ñ".as_bytes());
        assert!(rem.is_empty());
    }

    #[test]
    fn mixed_with_trailing_incomplete() {
        // Complete 'x' + incomplete CSI
        let input = b"x\x1b[";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs, vec![b"x".as_slice()]);
        assert_eq!(rem, b"\x1b[");
    }

    #[test]
    fn empty_buffer() {
        let (seqs, rem) = split_sequences(b"");
        assert!(seqs.is_empty());
        assert!(rem.is_empty());
    }

    #[test]
    fn multiple_csi_sequences() {
        // PageUp + PageDown
        let input = b"\x1b[5~\x1b[6~";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], b"\x1b[5~");
        assert_eq!(seqs[1], b"\x1b[6~");
        assert!(rem.is_empty());
    }

    #[test]
    fn space_byte() {
        let (seqs, rem) = split_sequences(b" ");
        assert_eq!(seqs, vec![b" ".as_slice()]);
        assert!(rem.is_empty());
    }

    #[test]
    fn sgr_mouse_left_press() {
        let seq = b"\x1b[<0;50;10M";
        assert!(is_sgr_mouse(seq));
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        assert_eq!(ev.column, 49);
        assert_eq!(ev.row, 9);
    }

    #[test]
    fn sgr_mouse_left_release() {
        let seq = b"\x1b[<0;50;10m";
        assert!(is_sgr_mouse(seq));
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Up(MouseButton::Left));
        assert_eq!(ev.column, 49);
        assert_eq!(ev.row, 9);
    }

    #[test]
    fn sgr_mouse_right_press() {
        let seq = b"\x1b[<2;1;1M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Right));
        assert_eq!(ev.column, 0);
        assert_eq!(ev.row, 0);
    }

    #[test]
    fn sgr_mouse_scroll_up() {
        let seq = b"\x1b[<64;10;20M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::ScrollUp);
    }

    #[test]
    fn sgr_mouse_scroll_down() {
        let seq = b"\x1b[<65;10;20M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::ScrollDown);
    }

    #[test]
    fn sgr_mouse_left_drag() {
        let seq = b"\x1b[<32;5;5M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Drag(MouseButton::Left));
    }

    #[test]
    fn sgr_mouse_motion_no_button() {
        let seq = b"\x1b[<35;5;5M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Moved);
    }

    #[test]
    fn sgr_mouse_split_and_parse() {
        // Verify mouse sequences are correctly split as complete CSI sequences.
        let input = b"\x1b[<0;50;10M\x1b[<0;50;10m";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 2);
        assert!(rem.is_empty());
        assert!(is_sgr_mouse(seqs[0]));
        assert!(is_sgr_mouse(seqs[1]));
    }

    #[test]
    fn bare_esc_before_sgr_mouse_does_not_strip_mouse_prefix() {
        let input = b"\x1b\x1b[<35;138;12M";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], b"\x1b");
        assert_eq!(seqs[1], b"\x1b[<35;138;12M");
        assert!(rem.is_empty());
        assert!(is_sgr_mouse(seqs[1]));
    }

    #[test]
    fn parser_keeps_fragmented_csi_pending_until_complete() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b[").is_empty());
        assert_eq!(parser.pending(), b"\x1b[");

        let seqs = parser.feed_sequences(b"5~");
        assert_eq!(seqs[0].bytes, b"\x1b[5~".to_vec());
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_resolves_timed_out_bare_esc() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b").is_empty());
        assert_eq!(parser.pending(), b"\x1b");
        assert_eq!(parser.resolve_pending_esc(), Some(b"\x1b".to_vec()));
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_does_not_resolve_pending_csi_as_bare_esc() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b[").is_empty());
        assert_eq!(parser.pending(), b"\x1b[");
        assert!(parser.resolve_pending_esc().is_none());
        assert_eq!(parser.pending(), b"\x1b[");
    }

    #[test]
    fn parser_keeps_alt_key_as_single_sequence() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b").is_empty());
        let seqs = parser.feed_sequences(b"x");
        assert_eq!(seqs[0].bytes, b"\x1bx".to_vec());
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_does_not_strip_mouse_prefix_after_pending_esc() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b").is_empty());
        let seqs = parser.feed_sequences(b"\x1b[<35;138;12M");
        assert_eq!(seqs[0].bytes, b"\x1b".to_vec());
        assert_eq!(seqs[1].bytes, b"\x1b[<35;138;12M".to_vec());
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_handles_fragmented_esc_then_sgr_mouse() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b").is_empty());
        let first = parser.feed_sequences(b"\x1b[<35;");
        assert_eq!(
            first,
            vec![ParsedSequence {
                bytes: b"\x1b".to_vec(),
                in_bracket_paste: false,
            }]
        );
        assert_eq!(parser.pending(), b"\x1b[<35;");

        let second = parser.feed_sequences(b"138;12M");
        assert_eq!(
            second,
            vec![ParsedSequence {
                bytes: b"\x1b[<35;138;12M".to_vec(),
                in_bracket_paste: false,
            }]
        );
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_completes_malformed_osc_before_new_escape() {
        let mut parser = RawInputParser::default();
        assert!(parser.feed_sequences(b"\x1b]11;rgb").is_empty());
        let seqs = parser.feed_sequences(b"\x1b[A");
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0].bytes, b"\x1b]11;rgb");
        assert_eq!(seqs[1].bytes, b"\x1b[A");
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn parser_tracks_bracket_paste_context() {
        let mut parser = RawInputParser::default();
        let mut input = Vec::new();
        input.extend_from_slice(BRACKET_PASTE_START);
        input.push(0x07);
        input.extend_from_slice(BRACKET_PASTE_END);

        let seqs = parser.feed_sequences(&input);
        assert_eq!(seqs.len(), 3);
        assert_eq!(seqs[0].bytes, BRACKET_PASTE_START);
        assert!(!seqs[0].in_bracket_paste);
        assert_eq!(seqs[1].bytes, b"\x07");
        assert!(seqs[1].in_bracket_paste);
        assert_eq!(seqs[2].bytes, BRACKET_PASTE_END);
        assert!(seqs[2].in_bracket_paste);
        assert!(!parser.in_bracket_paste());
    }

    #[test]
    fn osc_bel_terminated() {
        // OSC color response: ESC ] 11 ; rgb:0000/0000/0000 BEL
        let input = b"\x1b]11;rgb:0000/0000/0000\x07";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], input.as_slice());
        assert!(rem.is_empty());
    }

    #[test]
    fn osc_st_terminated() {
        // OSC terminated by ST (ESC \)
        let input = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], input.as_slice());
        assert!(rem.is_empty());
    }

    #[test]
    fn osc_palette_color_response() {
        // OSC 4 palette query response: ESC ] 4 ; 1 ; rgb:0000/0000/0000 BEL
        // This is the exact sequence that was appearing as garbage text.
        let input = b"\x1b]4;1;rgb:0000/0000/0000\x07";
        let (seqs, rem) = split_sequences(input);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], input.as_slice());
        assert!(rem.is_empty());
    }

    #[test]
    fn osc_incomplete() {
        // Incomplete OSC — no terminator yet.
        let input = b"\x1b]11;rgb:0000/0000";
        let (seqs, rem) = split_sequences(input);
        assert!(seqs.is_empty());
        assert_eq!(rem, input.as_slice());
    }

    #[test]
    fn osc_incomplete_st() {
        // OSC followed by ESC but no backslash yet.
        let input = b"\x1b]11;rgb:0000/0000/0000\x1b";
        let (seqs, rem) = split_sequences(input);
        assert!(seqs.is_empty());
        assert_eq!(rem, input.as_slice());
    }

    #[test]
    fn osc_followed_by_other_input() {
        // OSC response followed by a normal keypress.
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x07");
        input.push(b'a');
        let (seqs, rem) = split_sequences(&input);
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], b"\x1b]11;rgb:0000/0000/0000\x07");
        assert_eq!(seqs[1], b"a");
        assert!(rem.is_empty());
    }

    #[test]
    fn osc_not_fragmented_into_garbage() {
        // Before the fix, ESC ] was treated as a 2-byte alt-key sequence,
        // leaving "11;rgb:0000/0000/0000\x07" as separate bytes that would
        // be forwarded to the PTY child as garbage text input.
        let input = b"\x1b]11;rgb:0000/0000/0000\x07";
        let (seqs, rem) = split_sequences(input);
        // Must be exactly 1 sequence — not fragmented.
        assert_eq!(
            seqs.len(),
            1,
            "OSC must not be fragmented into multiple sequences"
        );
        // The full OSC including terminator must be preserved.
        assert_eq!(seqs[0], input.as_slice());
        assert!(rem.is_empty());
    }

    #[test]
    fn non_mouse_csi_not_detected() {
        assert!(!is_sgr_mouse(b"\x1b[A"));
        assert!(!is_sgr_mouse(b"\x1b[5~"));
        assert!(parse_sgr_mouse(b"\x1b[A").is_none());
    }

    #[test]
    fn parse_sgr_mouse_no_modifiers() {
        // Left click at (50,10): ESC [ < 0 ; 50 ; 10 M
        let seq = b"\x1b[<0;50;10M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        assert_eq!(ev.column, 49);
        assert_eq!(ev.row, 9);
        assert!(ev.modifiers.is_empty());
    }

    #[test]
    fn parse_sgr_mouse_shift_modifier() {
        // Shift+left click: cb = 0 | 4 = 4
        let seq = b"\x1b[<4;10;5M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        assert!(ev.modifiers.contains(KeyModifiers::SHIFT));
        assert!(!ev.modifiers.contains(KeyModifiers::ALT));
        assert!(!ev.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_sgr_mouse_ctrl_modifier() {
        // Ctrl+left click: cb = 0 | 16 = 16
        let seq = b"\x1b[<16;10;5M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        assert!(!ev.modifiers.contains(KeyModifiers::SHIFT));
        assert!(ev.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_sgr_mouse_shift_alt_ctrl() {
        // All modifiers: cb = 0 | 4 | 8 | 16 = 28
        let seq = b"\x1b[<28;10;5M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        assert!(ev.modifiers.contains(KeyModifiers::SHIFT));
        assert!(ev.modifiers.contains(KeyModifiers::ALT));
        assert!(ev.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_sgr_mouse_shift_drag() {
        // Shift+left drag: cb = 32 (motion) | 4 (shift) = 36
        let seq = b"\x1b[<36;20;10M";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Drag(MouseButton::Left));
        assert!(ev.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn parse_sgr_mouse_shift_release() {
        // Shift+left release: cb = 4, final byte = 'm'
        let seq = b"\x1b[<4;10;5m";
        let ev = parse_sgr_mouse(seq).unwrap();
        assert_eq!(ev.kind, MouseEventKind::Up(MouseButton::Left));
        assert!(ev.modifiers.contains(KeyModifiers::SHIFT));
    }

    // -- translate_sgr_mouse tests --
    //
    // These prove the coordinate-offset bug: when the terminal area starts
    // at screen row 3, column 5 and we receive a click at screen position
    // (50, 15), forwarding the raw bytes verbatim sends row=15 to the child
    // process instead of the correct row=12 (15 - 3). The child highlights
    // text 3 rows too low.

    #[test]
    fn translate_sgr_mouse_adjusts_coordinates() {
        // Screen click at 1-based (50, 15), terminal area origin at (5, 3).
        // Expected translated coords: (50-5, 15-3) = (45, 12).
        let seq = b"\x1b[<0;50;15M";
        let translated = translate_sgr_mouse(seq, 5, 3).unwrap();
        assert_eq!(translated, b"\x1b[<0;45;12M");
    }

    #[test]
    fn translate_sgr_mouse_top_left_corner() {
        // Click at the very first content cell: 1-based (6, 4) with origin (5, 3).
        // Translated: (1, 1) — the top-left of the child's viewport.
        let seq = b"\x1b[<0;6;4M";
        let translated = translate_sgr_mouse(seq, 5, 3).unwrap();
        assert_eq!(translated, b"\x1b[<0;1;1M");
    }

    #[test]
    fn translate_sgr_mouse_on_border_returns_none() {
        // Click at 1-based (5, 3) with origin (5, 3) → translated col = 0,
        // which is the border itself, not content. Should return None.
        let seq = b"\x1b[<0;5;3M";
        assert!(translate_sgr_mouse(seq, 5, 3).is_none());
    }

    #[test]
    fn translate_sgr_mouse_before_origin_returns_none() {
        // Click at 1-based (2, 1) with origin (5, 3) → would underflow.
        let seq = b"\x1b[<0;2;1M";
        assert!(translate_sgr_mouse(seq, 5, 3).is_none());
    }

    #[test]
    fn translate_sgr_mouse_release_event() {
        // Release events use 'm' as the final byte; translation must preserve it.
        let seq = b"\x1b[<0;50;15m";
        let translated = translate_sgr_mouse(seq, 5, 3).unwrap();
        assert_eq!(translated, b"\x1b[<0;45;12m");
    }

    #[test]
    fn translate_sgr_mouse_scroll_event() {
        // Scroll-up (cb=64) should also be translated.
        let seq = b"\x1b[<64;20;10M";
        let translated = translate_sgr_mouse(seq, 5, 3).unwrap();
        assert_eq!(translated, b"\x1b[<64;15;7M");
    }

    #[test]
    fn translate_sgr_mouse_drag_with_modifiers() {
        // Shift+left drag: cb = 32 (motion) | 4 (shift) = 36.
        let seq = b"\x1b[<36;30;20M";
        let translated = translate_sgr_mouse(seq, 5, 3).unwrap();
        assert_eq!(translated, b"\x1b[<36;25;17M");
    }

    #[test]
    fn translate_sgr_mouse_non_sgr_returns_none() {
        // A regular CSI sequence (cursor up) is not an SGR mouse event.
        let seq = b"\x1b[A";
        assert!(translate_sgr_mouse(seq, 5, 3).is_none());
    }

    #[test]
    fn translate_sgr_mouse_zero_origin() {
        // With origin (0, 0), coordinates should pass through unchanged.
        let seq = b"\x1b[<0;10;5M";
        let translated = translate_sgr_mouse(seq, 0, 0).unwrap();
        assert_eq!(translated, b"\x1b[<0;10;5M");
    }

    #[test]
    fn bracket_paste_markers_split_correctly() {
        // ESC[200~ starts bracket paste, ESC[201~ ends it.
        let mut input = Vec::new();
        input.extend_from_slice(BRACKET_PASTE_START);
        input.extend_from_slice(b"hello");
        input.extend_from_slice(BRACKET_PASTE_END);

        let (seqs, rem) = split_sequences(&input);
        assert!(rem.is_empty());
        // 1 start marker + 5 chars + 1 end marker = 7 sequences
        assert_eq!(seqs.len(), 7);
        assert_eq!(seqs[0], BRACKET_PASTE_START);
        assert_eq!(seqs[1], b"h");
        assert_eq!(seqs[2], b"e");
        assert_eq!(seqs[3], b"l");
        assert_eq!(seqs[4], b"l");
        assert_eq!(seqs[5], b"o");
        assert_eq!(seqs[6], BRACKET_PASTE_END);
    }

    #[test]
    fn bracket_paste_constants_are_valid() {
        assert_eq!(BRACKET_PASTE_START, b"\x1b[200~");
        assert_eq!(BRACKET_PASTE_END, b"\x1b[201~");
    }
}
