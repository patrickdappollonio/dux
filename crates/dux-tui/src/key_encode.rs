//! Encoding of key events into the bytes a child expects.
//!
//! The one source of truth for the classic key-to-bytes table, the bytes a
//! classic xterm sends, used two ways:
//!
//! - `key_event_to_pty_bytes` and `encode_for_child` turn a key dux hands to a
//!   child into its classic bytes. `encode_for_child` also knows the classic
//!   forms a classic host never sends to dux but a decoded extended key can
//!   name (Ctrl with digits, modified F-keys, shifted letters), sends
//!   Shift-Enter as the soft newline, and drops releases.
//! - `keybindings::key_combination_to_bytes` is a thin adapter over
//!   `encode_key` for building the interactive-mode byte patterns from crokey
//!   `KeyCombination`s. It carries no table of its own.
//!
//! Arrows and Home/End honor the child's DECCKM state, passed in by the caller:
//! with application cursor keys on (DECSET 1) the unmodified forms take SS3
//! (`ESC O A` and friends), and otherwise, as for every modified form, the CSI
//! encoding applies, exactly as xterm behaves. DECKPAM (application keypad,
//! `ESC =`) governs the numeric keypad and is out of scope here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// The child's DECCKM state: which form the unmodified arrows and Home/End take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorKeys {
    /// The default (DECRST 1): the CSI form, `ESC [ A`.
    Normal,
    /// Application cursor keys (DECSET 1): the SS3 form, `ESC O A`.
    Application,
}

impl CursorKeys {
    /// The cursor-key mode the child running in `client` has set right now.
    pub(crate) fn of(client: &dux_core::pty::PtyClient) -> Self {
        if client.has_app_cursor() {
            Self::Application
        } else {
            Self::Normal
        }
    }
}

/// Convert a crossterm key event into the byte sequence a legacy terminal
/// would send for it. Returns `None` for release events and for combinations
/// the legacy protocol cannot represent (callers drop those silently).
/// `cursor_keys` is the child's DECCKM state (DECSET 1), read from the
/// receiving PTY at encode time; it switches unmodified arrows and Home/End
/// to the SS3 application form.
pub(crate) fn key_event_to_pty_bytes(key: &KeyEvent, cursor_keys: CursorKeys) -> Option<Vec<u8>> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    encode_key(key.code, key.modifiers, cursor_keys)
}

/// The xterm-style modifier parameter for CSI sequences:
/// 1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0).
fn csi_modifier_param(mods: KeyModifiers) -> u8 {
    let mut m = 1u8;
    if mods.contains(KeyModifiers::SHIFT) {
        m += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m += 4;
    }
    m
}

/// A cursor key (arrow, Home, End): `CSI 1;m X` when modified, otherwise the
/// plain `CSI X` form, or SS3 (`ESC O X`) when the child enabled DECCKM.
fn cursor_key(final_byte: u8, mods: KeyModifiers, cursor_keys: CursorKeys) -> Vec<u8> {
    let m = csi_modifier_param(mods);
    if m == 1 {
        if cursor_keys == CursorKeys::Application {
            vec![0x1b, b'O', final_byte]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    } else {
        vec![0x1b, b'[', b'1', b';', b'0' + m, final_byte]
    }
}

/// `CSI n;m ~` for modified Insert/Delete/PgUp/PgDn, or the plain `CSI n ~`
/// form when no modifier is held.
fn csi_tilde(number: u8, mods: KeyModifiers) -> Vec<u8> {
    let m = csi_modifier_param(mods);
    if m == 1 {
        vec![0x1b, b'[', b'0' + number, b'~']
    } else {
        vec![0x1b, b'[', b'0' + number, b';', b'0' + m, b'~']
    }
}

/// The control byte for Ctrl+char, when one exists: letters map to
/// 0x01..0x1a, plus the classic symbol forms (Ctrl+Space is NUL, Ctrl+\ is
/// FS, and so on).
fn ctrl_byte(c: char) -> Option<u8> {
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        return Some(u8::try_from(lower).expect("an ASCII letter fits in a byte") - b'a' + 1);
    }
    match c {
        ' ' => Some(0x00),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        _ => None,
    }
}

/// The pure key-to-bytes table over `(code, modifiers)`. Returns `None` for
/// combinations the legacy terminal protocol has no bytes for.
pub(crate) fn encode_key(
    code: KeyCode,
    mods: KeyModifiers,
    cursor_keys: CursorKeys,
) -> Option<Vec<u8>> {
    let has_ctrl = mods.contains(KeyModifiers::CONTROL);
    let has_alt = mods.contains(KeyModifiers::ALT);
    let unmodified = csi_modifier_param(mods) == 1;

    match code {
        KeyCode::Char(c) if has_ctrl && has_alt => {
            // Ctrl+Alt+letter: ESC prefix, then the control byte.
            ctrl_byte(c).map(|b| vec![0x1b, b])
        }
        KeyCode::Char(c) if has_ctrl => {
            // Ctrl+letter (and Ctrl+Space, Ctrl+\, ...). Shift changes
            // nothing here: the control byte has no case.
            ctrl_byte(c).map(|b| vec![b])
        }
        KeyCode::Char(c) if has_alt => {
            // Alt+char: ESC then the char's UTF-8. This relies on crossterm's
            // classic decoding, which has already folded Shift into the char
            // (it delivers capitals as capitals); a decoded key with Shift and
            // a lower-case letter is upper-cased first by
            // `encode_for_classic_child`.
            let mut buf = vec![0x1b];
            let mut char_buf = [0u8; 4];
            buf.extend_from_slice(c.encode_utf8(&mut char_buf).as_bytes());
            Some(buf)
        }
        KeyCode::Char(c) => {
            // Plain or Shift+char: the char's own UTF-8. crossterm delivers
            // capitals as Char('P') with SHIFT set, so Shift must not make
            // the char unencodable or capitals become untypeable.
            let mut buf = [0u8; 4];
            Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Enter if has_alt => {
            // Alt+Enter: ESC CR, the newline-without-submit keystroke.
            Some(vec![0x1b, 0x0d])
        }
        KeyCode::Enter => {
            // CR: a classic terminal sends it for Enter with any modifier but
            // Alt. Shift-Enter becomes a line feed in `encode_for_child`, not
            // here, because this table also builds the fullscreen byte
            // patterns, where a `0x0a` pattern would fire on Ctrl-j.
            Some(vec![0x0d])
        }
        KeyCode::Tab if !mods.contains(KeyModifiers::SHIFT) => Some(vec![0x09]),
        // crossterm reports Shift-Tab as BackTab on a classic host and, for a
        // Kitty `CSI 9;2u`, as BackTab with Shift; terminput-crossterm does the
        // same. Tab with Shift is kept only as a defensive second spelling.
        KeyCode::Tab | KeyCode::BackTab => Some(vec![0x1b, b'[', b'Z']),
        KeyCode::Backspace if has_ctrl && has_alt => Some(vec![0x1b, 0x08]),
        KeyCode::Backspace if has_ctrl => Some(vec![0x08]),
        KeyCode::Backspace if has_alt => Some(vec![0x1b, 0x7f]),
        KeyCode::Backspace => Some(vec![0x7f]),
        // DECCKM applies to the cursor keys (arrows, Home, End) and only to
        // their UNMODIFIED forms: xterm keeps the CSI 1;m encoding for a
        // modified cursor key even in application mode, and so do we.
        KeyCode::Up => Some(cursor_key(b'A', mods, cursor_keys)),
        KeyCode::Down => Some(cursor_key(b'B', mods, cursor_keys)),
        KeyCode::Right => Some(cursor_key(b'C', mods, cursor_keys)),
        KeyCode::Left => Some(cursor_key(b'D', mods, cursor_keys)),
        KeyCode::Home => Some(cursor_key(b'H', mods, cursor_keys)),
        KeyCode::End => Some(cursor_key(b'F', mods, cursor_keys)),
        KeyCode::Insert => Some(csi_tilde(2, mods)),
        KeyCode::Delete => Some(csi_tilde(3, mods)),
        KeyCode::PageUp => Some(csi_tilde(5, mods)),
        KeyCode::PageDown => Some(csi_tilde(6, mods)),
        KeyCode::F(n) if unmodified => match n {
            1 => Some(vec![0x1b, b'O', b'P']),
            2 => Some(vec![0x1b, b'O', b'Q']),
            3 => Some(vec![0x1b, b'O', b'R']),
            4 => Some(vec![0x1b, b'O', b'S']),
            5 => Some(vec![0x1b, b'[', b'1', b'5', b'~']),
            6 => Some(vec![0x1b, b'[', b'1', b'7', b'~']),
            7 => Some(vec![0x1b, b'[', b'1', b'8', b'~']),
            8 => Some(vec![0x1b, b'[', b'1', b'9', b'~']),
            9 => Some(vec![0x1b, b'[', b'2', b'0', b'~']),
            10 => Some(vec![0x1b, b'[', b'2', b'1', b'~']),
            11 => Some(vec![0x1b, b'[', b'2', b'3', b'~']),
            12 => Some(vec![0x1b, b'[', b'2', b'4', b'~']),
            _ => None,
        },
        _ => None,
    }
}

/// Ctrl-`[` held with Ctrl alone is Esc: classic terminals send it as Esc, and
/// people press it as Escape everywhere.
pub(crate) fn fold_ctrl_bracket(key: KeyEvent) -> KeyEvent {
    if key.code == KeyCode::Char('[') && key.modifiers == KeyModifiers::CONTROL {
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            ..key
        }
    } else {
        key
    }
}

/// The key dux acts on for `key` as it arrived, or `None` for a release, on
/// which no dux action fires (crokey drops the kind, so it would fire the
/// action a second time). A repeat counts as a press, as a classic terminal
/// sends it, and with extended keys on Ctrl-`[` is Esc. The input debugger's
/// bindings column asks this too, so it shows what fires.
pub(crate) fn acted_on_key(
    key: KeyEvent,
    extended_keys: crate::keybindings::ExtendedKeys,
) -> Option<KeyEvent> {
    let key = match key.kind {
        KeyEventKind::Release => return None,
        KeyEventKind::Repeat => KeyEvent {
            kind: KeyEventKind::Press,
            ..key
        },
        KeyEventKind::Press => key,
    };
    Some(if extended_keys == crate::keybindings::ExtendedKeys::On {
        fold_ctrl_bracket(key)
    } else {
        key
    })
}

/// The modifiers the classic encoding has no bytes for.
const NON_CLASSIC_MODIFIERS: KeyModifiers = KeyModifiers::SUPER
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

/// Encode `key` for a child. Every child receives classic keys, the bytes a
/// classic xterm sends, except Shift-Enter: Shift alone with Enter is a line
/// feed (`0x0a`, the Ctrl-j byte), the soft newline the web sends. A repeat
/// counts as a press, as a classic terminal sends it. `None` for a release,
/// which a classic terminal never sends, and for a chord the classic encoding
/// cannot express.
pub(crate) fn encode_for_child(key: &KeyEvent, cursor_keys: CursorKeys) -> Option<Vec<u8>> {
    let key = fold_ctrl_bracket(*key);
    match key.kind {
        KeyEventKind::Release => None,
        KeyEventKind::Press | KeyEventKind::Repeat => {
            // Lock states live in `key.state`, not in the modifiers, so Caps
            // Lock or Num Lock leave this a Shift-Enter.
            if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::SHIFT {
                return Some(vec![0x0a]);
            }
            encode_for_classic_child(key.code, key.modifiers, cursor_keys)
        }
    }
}

/// A terminput key as crossterm's. terminput-crossterm's own conversion drops
/// Hyper and Meta, which would turn a Hyper chord into a plain key, so they
/// are carried across here.
pub(crate) fn from_terminput(key: terminput::KeyEvent) -> KeyEvent {
    let mut converted = terminput_crossterm::to_crossterm_key(key);
    for (terminput_bit, crossterm_bit) in [
        (terminput::KeyModifiers::HYPER, KeyModifiers::HYPER),
        (terminput::KeyModifiers::META, KeyModifiers::META),
    ] {
        if key.modifiers.contains(terminput_bit) {
            converted.modifiers |= crossterm_bit;
        }
    }
    converted
}

/// The bytes a classic xterm sends for `(code, mods)`. Covers the classic forms [`encode_key`] lacks because a
/// classic host never sends them to dux: Ctrl with digits and with `@`, `[`,
/// `/` and `?`, upper-case for a shifted letter, and modified F1 to F12.
/// `None` for a key held with Super, Hyper or Meta: the classic encoding has
/// no bytes for them, and a bare letter would type a character nobody typed.
pub(crate) fn encode_for_classic_child(
    code: KeyCode,
    mods: KeyModifiers,
    cursor_keys: CursorKeys,
) -> Option<Vec<u8>> {
    if mods.intersects(NON_CLASSIC_MODIFIERS) {
        return None;
    }
    let has_ctrl = mods.contains(KeyModifiers::CONTROL);
    let has_alt = mods.contains(KeyModifiers::ALT);
    let has_shift = mods.contains(KeyModifiers::SHIFT);
    let alt_prefixed = |byte: u8| {
        if has_alt {
            vec![0x1b, byte]
        } else {
            vec![byte]
        }
    };
    match code {
        // xterm sends Alt with Esc, Tab or Shift-Tab as ESC and then the key's
        // own bytes; `encode_key` has no Alt form for them.
        KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab if has_alt => {
            let mut out = vec![0x1b];
            out.extend(encode_key(code, mods - KeyModifiers::ALT, cursor_keys)?);
            return Some(out);
        }
        KeyCode::Char(c) if has_ctrl => {
            if let Some(byte) = classic_ctrl_extra(c) {
                return Some(alt_prefixed(byte));
            }
        }
        KeyCode::Char(c) if has_shift && c.is_ascii_lowercase() => {
            let upper =
                u8::try_from(c.to_ascii_uppercase()).expect("an ASCII letter fits in a byte");
            return Some(alt_prefixed(upper));
        }
        KeyCode::F(n @ 1..=4) if csi_modifier_param(mods) > 1 => {
            let final_byte = b'P' + (n - 1);
            return Some(vec![
                0x1b,
                b'[',
                b'1',
                b';',
                b'0' + csi_modifier_param(mods),
                final_byte,
            ]);
        }
        KeyCode::F(n @ 5..=12) if csi_modifier_param(mods) > 1 => {
            let number: &[u8] = match n {
                5 => b"15",
                6 => b"17",
                7 => b"18",
                8 => b"19",
                9 => b"20",
                10 => b"21",
                11 => b"23",
                _ => b"24",
            };
            let mut out = b"\x1b[".to_vec();
            out.extend_from_slice(number);
            out.extend_from_slice(&[b';', b'0' + csi_modifier_param(mods), b'~']);
            return Some(out);
        }
        _ => {}
    }
    encode_key(code, mods, cursor_keys)
}

/// xterm's control bytes for Ctrl with a digit or with `@`, `[`, `/`, `?`,
/// `-`, `` ` `` or `~`, which [`ctrl_byte`] leaves out.
fn classic_ctrl_extra(c: char) -> Option<u8> {
    match c {
        '2' | '@' | '`' => Some(0x00),
        '3' | '[' => Some(0x1b),
        '4' => Some(0x1c),
        '5' => Some(0x1d),
        '6' | '~' => Some(0x1e),
        '7' | '/' | '-' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        '1' | '9' | '0' => u8::try_from(c).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn capitals_encode_as_their_utf8() {
        // crossterm delivers a capital as Char('P') with SHIFT set; returning
        // None here would make capitals untypeable in the minimized pane.
        let ev = press(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(b"P".to_vec())
        );
    }

    /// Crossterm spells Shift-Tab `BackTab`, with or without Shift held; `Tab`
    /// with Shift is the defensive second spelling. All are the same keystroke
    /// and all must send the CSI backtab sequence.
    #[test]
    fn both_spellings_of_shift_tab_encode_to_csi_backtab() {
        let csi_backtab = Some(vec![0x1b, b'[', b'Z']);
        assert_eq!(
            key_event_to_pty_bytes(
                &press(KeyCode::BackTab, KeyModifiers::SHIFT),
                CursorKeys::Normal
            ),
            csi_backtab
        );
        assert_eq!(
            key_event_to_pty_bytes(
                &press(KeyCode::Tab, KeyModifiers::SHIFT),
                CursorKeys::Normal
            ),
            csi_backtab
        );
        assert_eq!(
            key_event_to_pty_bytes(&press(KeyCode::Tab, KeyModifiers::NONE), CursorKeys::Normal),
            Some(vec![0x09]),
            "plain Tab is still the tab byte"
        );
    }

    #[test]
    fn ctrl_arrow_takes_the_modified_csi_form() {
        let ev = press(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
        );
    }

    #[test]
    fn shift_arrow_takes_the_modified_csi_form() {
        let ev = press(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'1', b';', b'2', b'A'])
        );
    }

    #[test]
    fn ctrl_shift_alt_arrow_sums_the_modifier_param() {
        let ev = press(
            KeyCode::Down,
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
        );
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'1', b';', b'8', b'B'])
        );
    }

    #[test]
    fn modified_delete_and_insert_take_the_tilde_form() {
        let del = press(KeyCode::Delete, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&del, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'3', b';', b'5', b'~'])
        );
        let ins = press(KeyCode::Insert, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ins, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'2', b';', b'2', b'~'])
        );
        let pgup = press(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&pgup, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'5', b'~'])
        );
    }

    #[test]
    fn alt_enter_is_esc_cr() {
        let ev = press(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x1b, 0x0d])
        );
    }

    #[test]
    fn shift_enter_is_plain_cr() {
        // The classic table, and the fullscreen byte patterns built from it,
        // keep CR for Shift-Enter: a `0x0a` pattern would make a `shift-enter`
        // binding fire on Ctrl-j. Only `encode_for_child` sends the line feed.
        let ev = press(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x0d])
        );
        assert_eq!(
            encode_key(KeyCode::Enter, KeyModifiers::SHIFT, CursorKeys::Normal),
            Some(vec![0x0d])
        );
    }

    #[test]
    fn ctrl_space_is_nul() {
        let ev = press(KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x00])
        );
    }

    #[test]
    fn ctrl_backspace_and_alt_backspace() {
        let ctrl = press(KeyCode::Backspace, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ctrl, CursorKeys::Normal),
            Some(vec![0x08])
        );
        let alt = press(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(
            key_event_to_pty_bytes(&alt, CursorKeys::Normal),
            Some(vec![0x1b, 0x7f])
        );
    }

    #[test]
    fn ctrl_alt_letter_is_esc_then_ctrl_byte() {
        let ev = press(
            KeyCode::Char('b'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x1b, 0x02])
        );
    }

    #[test]
    fn ctrl_j_is_line_feed() {
        let ev = press(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x0a])
        );
    }

    #[test]
    fn ctrl_c_is_etx() {
        let ev = press(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(vec![0x03])
        );
        assert_eq!(encode_for_child(&ev, CursorKeys::Normal), Some(vec![0x03]));
        // The fullscreen byte patterns keep today's table: a pattern for Ctrl-2
        // would fire on Ctrl-Space's byte, so it has none.
        assert_eq!(
            encode_key(
                KeyCode::Char('2'),
                KeyModifiers::CONTROL,
                CursorKeys::Normal
            ),
            None
        );
        assert_eq!(
            encode_key(
                KeyCode::Char('/'),
                KeyModifiers::CONTROL,
                CursorKeys::Normal
            ),
            None
        );
        assert_eq!(
            encode_key(KeyCode::F(5), KeyModifiers::CONTROL, CursorKeys::Normal),
            None
        );
    }

    #[test]
    fn f13_is_unencodable() {
        let ev = press(KeyCode::F(13), KeyModifiers::NONE);
        assert_eq!(key_event_to_pty_bytes(&ev, CursorKeys::Normal), None);
    }

    #[test]
    fn release_kind_returns_none() {
        let mut ev = press(KeyCode::Char('a'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert_eq!(key_event_to_pty_bytes(&ev, CursorKeys::Normal), None);
    }

    #[test]
    fn repeat_kind_encodes() {
        let mut ev = press(KeyCode::Char('a'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Repeat;
        assert_eq!(
            key_event_to_pty_bytes(&ev, CursorKeys::Normal),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn modified_home_and_end() {
        let home = press(KeyCode::Home, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&home, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'H'])
        );
        let end = press(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&end, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'F'])
        );
    }

    #[test]
    fn app_cursor_switches_unmodified_arrows_to_ss3() {
        // DECCKM on: unmodified arrows take the SS3 application form.
        for (code, letter) in [
            (KeyCode::Up, b'A'),
            (KeyCode::Down, b'B'),
            (KeyCode::Right, b'C'),
            (KeyCode::Left, b'D'),
        ] {
            let ev = press(code, KeyModifiers::NONE);
            assert_eq!(
                key_event_to_pty_bytes(&ev, CursorKeys::Application),
                Some(vec![0x1b, b'O', letter])
            );
        }
    }

    #[test]
    fn app_cursor_switches_unmodified_home_and_end_to_ss3() {
        let home = press(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&home, CursorKeys::Application),
            Some(vec![0x1b, b'O', b'H'])
        );
        let end = press(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&end, CursorKeys::Application),
            Some(vec![0x1b, b'O', b'F'])
        );
    }

    #[test]
    fn app_cursor_leaves_modified_cursor_keys_on_csi() {
        // xterm keeps the CSI 1;m form for modified cursor keys even in
        // application cursor mode; only the unmodified forms switch to SS3.
        let ctrl_right = press(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ctrl_right, CursorKeys::Application),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
        );
        let shift_home = press(KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&shift_home, CursorKeys::Application),
            Some(vec![0x1b, b'[', b'1', b';', b'2', b'H'])
        );
    }

    #[test]
    fn app_cursor_off_keeps_unmodified_arrows_on_csi() {
        let up = press(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&up, CursorKeys::Normal),
            Some(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn app_cursor_does_not_touch_tilde_keys_or_f_keys() {
        // DECCKM covers the cursor keys only; PgUp/PgDn/Delete/Insert and the
        // F-keys encode identically in both states.
        let pgup = press(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&pgup, CursorKeys::Application),
            key_event_to_pty_bytes(&pgup, CursorKeys::Normal)
        );
        let del = press(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&del, CursorKeys::Application),
            key_event_to_pty_bytes(&del, CursorKeys::Normal)
        );
        let f5 = press(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&f5, CursorKeys::Application),
            key_event_to_pty_bytes(&f5, CursorKeys::Normal)
        );
    }

    fn with(code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, mods, kind)
    }

    /// Every row is a byte literal a classic xterm sends for that key, with
    /// Shift-Enter as the soft newline.
    #[test]
    fn encode_for_child_gives_a_classic_child_every_xterm_classic_form() {
        use KeyEventKind::{Press, Release, Repeat};
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let shift = KeyModifiers::SHIFT;
        let none = KeyModifiers::NONE;
        let caps_lock_shift_enter = KeyEvent::new_with_kind_and_state(
            KeyCode::Enter,
            shift,
            Press,
            KeyEventState::CAPS_LOCK,
        );
        let rows: Vec<(KeyEvent, Option<&[u8]>)> = vec![
            (with(KeyCode::Char(' '), ctrl, Press), Some(&[0x00])),
            (with(KeyCode::Char('2'), ctrl, Press), Some(&[0x00])),
            (with(KeyCode::Char('@'), ctrl, Press), Some(&[0x00])),
            (with(KeyCode::Char('3'), ctrl, Press), Some(&[0x1b])),
            (with(KeyCode::Char('['), ctrl, Press), Some(&[0x1b])),
            (with(KeyCode::Char('\\'), ctrl, Press), Some(&[0x1c])),
            (with(KeyCode::Char('4'), ctrl, Press), Some(&[0x1c])),
            (with(KeyCode::Char(']'), ctrl, Press), Some(&[0x1d])),
            (with(KeyCode::Char('5'), ctrl, Press), Some(&[0x1d])),
            (with(KeyCode::Char('^'), ctrl, Press), Some(&[0x1e])),
            (with(KeyCode::Char('6'), ctrl, Press), Some(&[0x1e])),
            (with(KeyCode::Char('_'), ctrl, Press), Some(&[0x1f])),
            (with(KeyCode::Char('/'), ctrl, Press), Some(&[0x1f])),
            (with(KeyCode::Char('7'), ctrl, Press), Some(&[0x1f])),
            (with(KeyCode::Char('8'), ctrl, Press), Some(&[0x7f])),
            (with(KeyCode::Char('?'), ctrl, Press), Some(&[0x7f])),
            (with(KeyCode::Char('1'), ctrl, Press), Some(b"1")),
            (with(KeyCode::Char('9'), ctrl, Press), Some(b"9")),
            (with(KeyCode::Char('0'), ctrl, Press), Some(b"0")),
            (with(KeyCode::Char('l'), ctrl, Press), Some(&[0x0c])),
            (with(KeyCode::Char('l'), ctrl | shift, Press), Some(&[0x0c])),
            (with(KeyCode::Char('L'), ctrl | shift, Press), Some(&[0x0c])),
            (
                with(KeyCode::Char('2'), ctrl | alt, Press),
                Some(&[0x1b, 0x00]),
            ),
            (
                with(KeyCode::Char('a'), ctrl | alt | shift, Press),
                Some(&[0x1b, 0x01]),
            ),
            (with(KeyCode::Char('a'), alt | shift, Press), Some(b"\x1bA")),
            (with(KeyCode::Char('a'), shift, Press), Some(b"A")),
            (with(KeyCode::Char('x'), alt, Press), Some(b"\x1bx")),
            (with(KeyCode::Esc, none, Press), Some(&[0x1b])),
            (with(KeyCode::Enter, none, Press), Some(&[0x0d])),
            (with(KeyCode::Enter, shift, Press), Some(&[0x0a])),
            (with(KeyCode::Enter, shift, Repeat), Some(&[0x0a])),
            (caps_lock_shift_enter, Some(&[0x0a])),
            (with(KeyCode::Enter, ctrl, Press), Some(&[0x0d])),
            (with(KeyCode::Enter, ctrl | shift, Press), Some(&[0x0d])),
            (with(KeyCode::Enter, alt, Press), Some(&[0x1b, 0x0d])),
            (
                with(KeyCode::Enter, alt | shift, Press),
                Some(&[0x1b, 0x0d]),
            ),
            (with(KeyCode::Enter, ctrl | alt, Press), Some(&[0x1b, 0x0d])),
            (with(KeyCode::BackTab, none, Press), Some(b"\x1b[Z")),
            (with(KeyCode::BackTab, shift, Press), Some(b"\x1b[Z")),
            (with(KeyCode::Esc, alt, Press), Some(&[0x1b, 0x1b])),
            (with(KeyCode::Tab, alt, Press), Some(&[0x1b, 0x09])),
            (
                with(KeyCode::BackTab, shift | alt, Press),
                Some(b"\x1b\x1b[Z"),
            ),
            (with(KeyCode::Tab, shift | alt, Press), Some(b"\x1b\x1b[Z")),
            (with(KeyCode::Char('-'), ctrl, Press), Some(&[0x1f])),
            (with(KeyCode::Char('`'), ctrl, Press), Some(&[0x00])),
            (with(KeyCode::Char('~'), ctrl, Press), Some(&[0x1e])),
            (
                with(KeyCode::Char('-'), ctrl | alt, Press),
                Some(&[0x1b, 0x1f]),
            ),
            (with(KeyCode::F(1), shift, Press), Some(b"\x1b[1;2P")),
            (with(KeyCode::F(4), ctrl, Press), Some(b"\x1b[1;5S")),
            (with(KeyCode::F(5), ctrl, Press), Some(b"\x1b[15;5~")),
            (with(KeyCode::F(12), alt, Press), Some(b"\x1b[24;3~")),
            (with(KeyCode::Up, ctrl, Press), Some(b"\x1b[1;5A")),
            (with(KeyCode::Home, shift, Press), Some(b"\x1b[1;2H")),
            (with(KeyCode::Delete, ctrl | alt, Press), Some(b"\x1b[3;7~")),
            (with(KeyCode::PageUp, shift, Press), Some(b"\x1b[5;2~")),
            (with(KeyCode::Char('x'), KeyModifiers::SUPER, Press), None),
            (with(KeyCode::Char('x'), KeyModifiers::HYPER, Press), None),
            (with(KeyCode::Char('x'), KeyModifiers::META, Press), None),
            (with(KeyCode::Char('a'), none, Release), None),
            (with(KeyCode::Char('a'), none, Repeat), Some(b"a")),
        ];
        for (key, expected) in rows {
            assert_eq!(
                encode_for_child(&key, CursorKeys::Normal).as_deref(),
                expected,
                "{key:?}"
            );
        }
    }

    /// A classic host sends a lone `0x08` and `0x1c` to `0x1f` for Ctrl-h and
    /// Ctrl with `\`, `]`, `^` and `_`; crossterm reads the last four as Ctrl
    /// with the digits `4` to `7`. Each reaches a child as the byte it was.
    #[test]
    fn the_lone_control_bytes_crossterm_reads_reach_a_child_unchanged() {
        let ctrl = KeyModifiers::CONTROL;
        let rows: [(KeyEvent, u8); 5] = [
            (KeyEvent::new(KeyCode::Char('h'), ctrl), 0x08),
            (KeyEvent::new(KeyCode::Char('4'), ctrl), 0x1c),
            (KeyEvent::new(KeyCode::Char('5'), ctrl), 0x1d),
            (KeyEvent::new(KeyCode::Char('6'), ctrl), 0x1e),
            (KeyEvent::new(KeyCode::Char('7'), ctrl), 0x1f),
        ];
        for (key, byte) in rows {
            assert_eq!(
                encode_for_child(&key, CursorKeys::Normal),
                Some(vec![byte]),
                "{key:?}"
            );
        }
    }
}
