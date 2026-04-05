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
}
