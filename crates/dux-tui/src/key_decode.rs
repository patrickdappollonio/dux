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

/// Whether `seq` is a functional key in its legacy shape (`CSI 1 ; m X` or
/// `CSI n ; m ~`) carrying a field a classic xterm never sends: a modifier
/// with a lock, Super, Hyper or Meta bit, an event type, or a value out of
/// range. A host in the Kitty protocol reports arrows, Home, End, the page keys
/// and F1 to F12 this way, with Caps Lock or Num Lock in the modifier, so such a
/// key is read by meaning rather than passed on byte for byte. A classic
/// modifier (`CSI 1 ; 5 A`) is not matched here: those bytes are what a classic
/// terminal sends and still reach a program unchanged. Three fields (tmux's
/// xterm format, `CSI 27 ; m ; code ~`) are left alone as well.
pub(crate) fn is_extended_legacy_key_sequence(seq: &[u8]) -> bool {
    let Some(body) = seq.strip_prefix(b"\x1b[") else {
        return false;
    };
    let Some((final_byte, params)) = body.split_last() else {
        return false;
    };
    if !matches!(
        final_byte,
        b'A' | b'B' | b'C' | b'D' | b'E' | b'F' | b'H' | b'P' | b'Q' | b'R' | b'S' | b'~'
    ) {
        return false;
    }
    let mut fields = params.split(|&byte| byte == b';');
    let (Some(number), Some(modifier), None) = (fields.next(), fields.next(), fields.next()) else {
        return false;
    };
    if number.is_empty() || !number.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let mut parts = modifier.split(|&byte| byte == b':');
    let (Some(mask), event_type) = (parts.next(), parts.next()) else {
        return false;
    };
    if mask.is_empty() || !mask.iter().all(u8::is_ascii_digit) || parts.next().is_some() {
        return false;
    }
    if event_type.is_some_and(|kind| kind.is_empty() || !kind.iter().all(u8::is_ascii_digit)) {
        return false;
    }
    let classic_mask = std::str::from_utf8(mask)
        .ok()
        .and_then(|mask| mask.parse::<u16>().ok())
        .is_some_and(|mask| (1..=8).contains(&mask));
    event_type.is_some() || !classic_mask
}

/// The key `seq` encodes, when it is one complete key sequence. `None` for
/// anything that is not a key (a mouse report, a paste marker) or not complete.
///
/// A modifier or event-type field out of the protocol's range is refused here
/// rather than handed to terminput, which reads such a field as absent: a
/// mangled Ctrl chord would otherwise come back as a bare letter and be typed.
pub(crate) fn decode_key(seq: &[u8]) -> Option<KeyEvent> {
    if !modifier_field_in_range(seq) {
        return None;
    }
    match terminput::Event::parse_from(seq) {
        Ok(Some(terminput::Event::Key(key))) => Some(crate::key_encode::from_terminput(key)),
        _ => None,
    }
}

/// Whether the modifier field of a CSI key (its second `;` field), when there
/// is one, holds a mask from 1 to 255 and, after a `:`, an event type from 1 to
/// 3. Anything that is not a CSI passes: only a CSI key carries these fields.
fn modifier_field_in_range(seq: &[u8]) -> bool {
    let Some(body) = seq.strip_prefix(b"\x1b[") else {
        return true;
    };
    let Some((_, params)) = body.split_last() else {
        return true;
    };
    let Some(modifier) = params.split(|&byte| byte == b';').nth(1) else {
        return true;
    };
    let number = |field: &[u8]| -> Option<u16> {
        if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(field).ok()?.parse().ok()
    };
    let mut parts = modifier.split(|&byte| byte == b':');
    let mask_ok = parts.next().is_some_and(|mask| {
        mask.is_empty() || number(mask).is_some_and(|m| (1..=255).contains(&m))
    });
    let kind_ok = parts
        .next()
        .is_none_or(|kind| number(kind).is_some_and(|k| (1..=3).contains(&k)));
    mask_ok && kind_ok && parts.next().is_none()
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
    fn only_a_legacy_key_with_a_field_a_classic_xterm_never_sends_is_extended() {
        for extended in [
            &b"\x1b[1;65A"[..],
            b"\x1b[1;69A",
            b"\x1b[3;129~",
            b"\x1b[1;9B",
            b"\x1b[1;1:3A",
            b"\x1b[15;65~",
            b"\x1b[1;999A",
        ] {
            assert!(is_extended_legacy_key_sequence(extended), "{extended:?}");
        }
        for classic in [
            &b"\x1b[A"[..],
            b"\x1b[1;5A",
            b"\x1b[3;5~",
            b"\x1b[5~",
            b"\x1b[1;8P",
            b"\x1b[27;2;13~",
            b"\x1b[13;65u",
            b"\x1b[?1;65A",
            b"\x1b[1;A",
        ] {
            assert!(!is_extended_legacy_key_sequence(classic), "{classic:?}");
        }
    }

    #[test]
    fn a_legacy_key_with_lock_bits_decodes_to_its_meaning_with_the_locks_left_out() {
        let up = decode_key(b"\x1b[1;65A").expect("a key");
        assert_eq!(up.code, KeyCode::Up);
        assert_eq!(up.modifiers, KeyModifiers::NONE);
        let ctrl_up = decode_key(b"\x1b[1;69A").expect("a key");
        assert_eq!(ctrl_up.code, KeyCode::Up);
        assert_eq!(ctrl_up.modifiers, KeyModifiers::CONTROL);
        let delete = decode_key(b"\x1b[3;133~").expect("a key");
        assert_eq!(delete.code, KeyCode::Delete);
        assert_eq!(delete.modifiers, KeyModifiers::CONTROL);
        assert_eq!(decode_key(b"\x1b[1;999A"), None, "an over-range modifier");
    }

    #[test]
    fn an_extended_shape_that_is_not_a_key_does_not_decode() {
        for bad in [
            &b"\x1b[55296u"[..],
            b"\x1b[;5u",
            b"\x1b[99;256u",
            b"\x1b[99;5:4u",
            b"\x1b[99;x5u",
        ] {
            assert!(is_extended_key_sequence(bad), "{bad:?}");
            assert_eq!(decode_key(bad), None, "{bad:?}");
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
