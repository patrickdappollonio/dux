//! Encoding of key events into the raw bytes a legacy terminal would send.
//!
//! This module is the ONE source of truth for the key-to-bytes table. It is
//! used two ways:
//!
//! - `key_event_to_pty_bytes` turns a crossterm `KeyEvent` into PTY bytes, so
//!   the minimized center pane can forward typed keys to the agent.
//! - `keybindings::key_combination_to_bytes` is a thin adapter over
//!   `encode_key` for building the interactive-mode byte patterns from crokey
//!   `KeyCombination`s. It carries no table of its own.
//!
//! Arrows and Home/End honor the child's DECCKM state, passed in by the
//! caller: with application cursor keys on (DECSET 1), the UNMODIFIED forms
//! take SS3 (`ESC O A` and friends); otherwise, and for every modified form,
//! the CSI encoding applies, exactly as xterm behaves. DECKPAM (application
//! keypad, `ESC =`) is a different mode governing the numeric keypad and is
//! deliberately out of scope here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Convert a crossterm key event into the byte sequence a legacy terminal
/// would send for it. Returns `None` for release events and for combinations
/// the legacy protocol cannot represent (callers drop those silently).
/// `app_cursor` is the child's DECCKM state (DECSET 1), read from the
/// receiving PTY at encode time; it switches unmodified arrows and Home/End
/// to the SS3 application form.
pub(crate) fn key_event_to_pty_bytes(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    encode_key(key.code, key.modifiers, app_cursor)
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
fn cursor_key(final_byte: u8, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    let m = csi_modifier_param(mods);
    if m == 1 {
        if app_cursor {
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
        return Some(lower as u8 - b'a' + 1);
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
pub(crate) fn encode_key(code: KeyCode, mods: KeyModifiers, app_cursor: bool) -> Option<Vec<u8>> {
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
            // Alt+char: ESC then the char's UTF-8 (Shift is already folded
            // into the char itself, crossterm delivers capitals as capitals).
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
            // Plain CR, and deliberately also for Shift+Enter: the legacy
            // terminal protocol sends the same CR byte for both, so there is
            // no distinct Shift+Enter to encode (see the Shift-Enter tenet in
            // CLAUDE.md; Ctrl-j is the supported soft-newline key).
            Some(vec![0x0d])
        }
        KeyCode::Tab if !mods.contains(KeyModifiers::SHIFT) => Some(vec![0x09]),
        KeyCode::BackTab => Some(vec![0x1b, b'[', b'Z']),
        KeyCode::Backspace if has_ctrl && has_alt => Some(vec![0x1b, 0x08]),
        KeyCode::Backspace if has_ctrl => Some(vec![0x08]),
        KeyCode::Backspace if has_alt => Some(vec![0x1b, 0x7f]),
        KeyCode::Backspace => Some(vec![0x7f]),
        // DECCKM applies to the cursor keys (arrows, Home, End) and only to
        // their UNMODIFIED forms: xterm keeps the CSI 1;m encoding for a
        // modified cursor key even in application mode, and so do we.
        KeyCode::Up => Some(cursor_key(b'A', mods, app_cursor)),
        KeyCode::Down => Some(cursor_key(b'B', mods, app_cursor)),
        KeyCode::Right => Some(cursor_key(b'C', mods, app_cursor)),
        KeyCode::Left => Some(cursor_key(b'D', mods, app_cursor)),
        KeyCode::Home => Some(cursor_key(b'H', mods, app_cursor)),
        KeyCode::End => Some(cursor_key(b'F', mods, app_cursor)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn capitals_encode_as_their_utf8() {
        // crossterm delivers a capital as Char('P') with SHIFT set; returning
        // None here would make capitals untypeable in the minimized pane.
        let ev = press(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(b"P".to_vec()));
    }

    #[test]
    fn ctrl_arrow_takes_the_modified_csi_form() {
        let ev = press(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ev, false),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
        );
    }

    #[test]
    fn shift_arrow_takes_the_modified_csi_form() {
        let ev = press(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ev, false),
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
            key_event_to_pty_bytes(&ev, false),
            Some(vec![0x1b, b'[', b'1', b';', b'8', b'B'])
        );
    }

    #[test]
    fn modified_delete_and_insert_take_the_tilde_form() {
        let del = press(KeyCode::Delete, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&del, false),
            Some(vec![0x1b, b'[', b'3', b';', b'5', b'~'])
        );
        let ins = press(KeyCode::Insert, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&ins, false),
            Some(vec![0x1b, b'[', b'2', b';', b'2', b'~'])
        );
        let pgup = press(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&pgup, false),
            Some(vec![0x1b, b'[', b'5', b'~'])
        );
    }

    #[test]
    fn alt_enter_is_esc_cr() {
        let ev = press(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x1b, 0x0d]));
    }

    #[test]
    fn shift_enter_is_plain_cr() {
        // Deliberate: the legacy protocol cannot distinguish Shift+Enter from
        // Enter, so both encode as the same CR byte.
        let ev = press(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x0d]));
    }

    #[test]
    fn ctrl_space_is_nul() {
        let ev = press(KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x00]));
    }

    #[test]
    fn ctrl_backspace_and_alt_backspace() {
        let ctrl = press(KeyCode::Backspace, KeyModifiers::CONTROL);
        assert_eq!(key_event_to_pty_bytes(&ctrl, false), Some(vec![0x08]));
        let alt = press(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(key_event_to_pty_bytes(&alt, false), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn ctrl_alt_letter_is_esc_then_ctrl_byte() {
        let ev = press(
            KeyCode::Char('b'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x1b, 0x02]));
    }

    #[test]
    fn ctrl_j_is_line_feed() {
        let ev = press(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x0a]));
    }

    #[test]
    fn ctrl_c_is_etx() {
        let ev = press(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(vec![0x03]));
    }

    #[test]
    fn f13_is_unencodable() {
        let ev = press(KeyCode::F(13), KeyModifiers::NONE);
        assert_eq!(key_event_to_pty_bytes(&ev, false), None);
    }

    #[test]
    fn release_kind_returns_none() {
        let mut ev = press(KeyCode::Char('a'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert_eq!(key_event_to_pty_bytes(&ev, false), None);
    }

    #[test]
    fn repeat_kind_encodes() {
        let mut ev = press(KeyCode::Char('a'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Repeat;
        assert_eq!(key_event_to_pty_bytes(&ev, false), Some(b"a".to_vec()));
    }

    #[test]
    fn modified_home_and_end() {
        let home = press(KeyCode::Home, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&home, false),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'H'])
        );
        let end = press(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&end, false),
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
                key_event_to_pty_bytes(&ev, true),
                Some(vec![0x1b, b'O', letter])
            );
        }
    }

    #[test]
    fn app_cursor_switches_unmodified_home_and_end_to_ss3() {
        let home = press(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&home, true),
            Some(vec![0x1b, b'O', b'H'])
        );
        let end = press(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&end, true),
            Some(vec![0x1b, b'O', b'F'])
        );
    }

    #[test]
    fn app_cursor_leaves_modified_cursor_keys_on_csi() {
        // xterm keeps the CSI 1;m form for modified cursor keys even in
        // application cursor mode; only the unmodified forms switch to SS3.
        let ctrl_right = press(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            key_event_to_pty_bytes(&ctrl_right, true),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
        );
        let shift_home = press(KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_pty_bytes(&shift_home, true),
            Some(vec![0x1b, b'[', b'1', b';', b'2', b'H'])
        );
    }

    #[test]
    fn app_cursor_off_keeps_unmodified_arrows_on_csi() {
        let up = press(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&up, false),
            Some(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn app_cursor_does_not_touch_tilde_keys_or_f_keys() {
        // DECCKM covers the cursor keys only; PgUp/PgDn/Delete/Insert and the
        // F-keys encode identically in both states.
        let pgup = press(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&pgup, true),
            key_event_to_pty_bytes(&pgup, false)
        );
        let del = press(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&del, true),
            key_event_to_pty_bytes(&del, false)
        );
        let f5 = press(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(
            key_event_to_pty_bytes(&f5, true),
            key_event_to_pty_bytes(&f5, false)
        );
    }
}
