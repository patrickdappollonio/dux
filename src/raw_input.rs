use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

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
                    modifiers: crossterm::event::KeyModifiers::empty(),
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
        modifiers: crossterm::event::KeyModifiers::empty(),
    })
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
        let b = buf[i];

        if b == 0x1b {
            // ESC — start of an escape sequence.
            if i + 1 >= buf.len() {
                // Bare ESC at end of buffer — could be incomplete.
                return (sequences, &buf[start..]);
            }

            let next = buf[i + 1];
            match next {
                b'[' => {
                    // CSI sequence: ESC [ <params> <final byte 0x40-0x7e>
                    i += 2; // skip ESC [
                    loop {
                        if i >= buf.len() {
                            // Incomplete CSI — return as remainder.
                            return (sequences, &buf[start..]);
                        }
                        let c = buf[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&c) {
                            // Final byte — sequence complete.
                            break;
                        }
                        // Parameter/intermediate bytes (0x20-0x3f) — keep scanning.
                    }
                    sequences.push(&buf[start..i]);
                }
                b'O' => {
                    // SS3 sequence: ESC O <one byte>
                    if i + 2 >= buf.len() {
                        return (sequences, &buf[start..]);
                    }
                    i += 3; // ESC O <byte>
                    sequences.push(&buf[start..i]);
                }
                b']' => {
                    // OSC sequence: ESC ] <params> <BEL|ST>
                    // Terminated by BEL (0x07) or ST (ESC \).
                    i += 2; // skip ESC ]
                    loop {
                        if i >= buf.len() {
                            // Incomplete OSC — return as remainder.
                            return (sequences, &buf[start..]);
                        }
                        if buf[i] == 0x07 {
                            // BEL terminator.
                            i += 1;
                            break;
                        }
                        if buf[i] == 0x1b {
                            // Possible ST (ESC \).
                            if i + 1 >= buf.len() {
                                return (sequences, &buf[start..]);
                            }
                            if buf[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            // Malformed — treat ESC as start of next sequence.
                            break;
                        }
                        i += 1;
                    }
                    sequences.push(&buf[start..i]);
                }
                _ => {
                    // Alt+key or other two-byte ESC sequence.
                    i += 2;
                    sequences.push(&buf[start..i]);
                }
            }
        } else if (0xc0..0xfe).contains(&b) {
            // UTF-8 multi-byte lead byte.
            let expected = if b < 0xe0 {
                2
            } else if b < 0xf0 {
                3
            } else {
                4
            };
            if i + expected > buf.len() {
                // Incomplete UTF-8 — return as remainder.
                return (sequences, &buf[start..]);
            }
            i += expected;
            sequences.push(&buf[start..i]);
        } else {
            // Single byte: control chars, printable ASCII, DEL (0x7f).
            i += 1;
            sequences.push(&buf[start..i]);
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
}
