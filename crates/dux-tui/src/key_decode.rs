//! Decoding of one complete input sequence into a key.
//!
//! dux's raw-input splitter decides where a sequence ends; terminput only reads
//! the one sequence it is handed, because it does not report how many bytes it
//! consumed.

use crossterm::event::KeyEvent;

/// Whether `seq` is a key in the Kitty keyboard protocol's `CSI … u` form (or
/// tmux's `csi-u` extended form): a CSI ending in `u` whose parameters do not
/// start with `?`, `>`, `<` or `=`, which are the protocol's own requests and
/// replies rather than keys.
pub(crate) fn is_extended_key_sequence(seq: &[u8]) -> bool {
    let Some(body) = seq.strip_prefix(b"\x1b[") else {
        return false;
    };
    let Some((&b'u', params)) = body.split_last() else {
        return false;
    };
    !matches!(params.first(), Some(b'?' | b'>' | b'<' | b'='))
}

/// The key `seq` encodes, when it is one complete key sequence. `None` for
/// anything that is not a key (a mouse report, a paste marker) or not complete.
pub(crate) fn decode_key(seq: &[u8]) -> Option<KeyEvent> {
    match terminput::Event::parse_from(seq) {
        Ok(Some(terminput::Event::Key(key))) => Some(crate::key_encode::from_terminput(key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    #[test]
    fn only_a_csi_u_key_is_an_extended_key_sequence() {
        assert!(is_extended_key_sequence(b"\x1b[13;2u"));
        assert!(is_extended_key_sequence(b"\x1b[97;5:3u"));
        assert!(is_extended_key_sequence(b"\x1b[27u"));
        for not_a_key in [
            &b"\x1b[?1u"[..],
            b"\x1b[>1u",
            b"\x1b[<1u",
            b"\x1b[=1;1u",
            b"\x1b[A",
            b"\x1b[27;2;13~",
            b"u",
            b"\x1bu",
        ] {
            assert!(!is_extended_key_sequence(not_a_key), "{not_a_key:?}");
        }
    }

    #[test]
    fn decoding_reads_the_key_its_modifiers_and_its_kind() {
        let shift_enter = decode_key(b"\x1b[13;2u").expect("a key");
        assert_eq!(shift_enter.code, KeyCode::Enter);
        assert_eq!(shift_enter.modifiers, KeyModifiers::SHIFT);
        assert_eq!(shift_enter.kind, KeyEventKind::Press);

        let release = decode_key(b"\x1b[97;5:3u").expect("a key");
        assert_eq!(release.code, KeyCode::Char('a'));
        assert_eq!(release.modifiers, KeyModifiers::CONTROL);
        assert_eq!(release.kind, KeyEventKind::Release);

        let hyper = decode_key(b"\x1b[120;17u").expect("a key");
        assert_eq!(
            hyper.modifiers,
            KeyModifiers::HYPER,
            "Hyper survives the conversion"
        );
        let meta = decode_key(b"\x1b[120;33u").expect("a key");
        assert_eq!(
            meta.modifiers,
            KeyModifiers::META,
            "Meta survives the conversion"
        );

        assert_eq!(decode_key(b"\x1b[13;2"), None, "an incomplete sequence");
        assert_eq!(decode_key(b"\x1b[<0;1;1M"), None, "a mouse report");
    }
}
