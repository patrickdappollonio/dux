//! Translation between crossterm's `KeyEvent` and the crate-owned
//! [`WireKeyEvent`](crate::messages::WireKeyEvent).
//!
//! `WireKeyEvent` is what travels on the wire so the browser client (which
//! cannot depend on crossterm) can construct values directly from DOM
//! `KeyboardEvent`s. On the host, every arriving `RemoteMessage::InputKey`
//! passes through [`to_crossterm`] before reaching the TUI's input dispatch;
//! every outgoing key event from the native client passes through
//! [`from_crossterm`].
//!
//! The translation is total both directions for the subset of key codes
//! the protocol handles. Media keys and named modifier-only keys (crossterm
//! `KeyCode::Media`, `KeyCode::Modifier`) are not carried on the wire — the
//! host side maps them to `WireKeyCode::Null` on egress; the client side
//! does the same on ingress. Dux's input dispatch does not distinguish
//! these cases today.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use crate::messages::{WireKeyCode, WireKeyEvent, WireKeyKind};

/// Convert a crossterm `KeyEvent` into the wire representation.
pub fn from_crossterm(e: KeyEvent) -> WireKeyEvent {
    WireKeyEvent {
        code: encode_key_code(e.code),
        modifiers: e.modifiers.bits(),
        kind: encode_kind(e.kind),
    }
}

/// Convert a wire key event back into a crossterm `KeyEvent`.
///
/// Modifier bits outside the range crossterm recognizes are silently
/// dropped, matching how `KeyModifiers::from_bits_truncate` behaves.
pub fn to_crossterm(e: &WireKeyEvent) -> KeyEvent {
    KeyEvent {
        code: decode_key_code(&e.code),
        modifiers: KeyModifiers::from_bits_truncate(e.modifiers),
        kind: decode_kind(e.kind),
        state: KeyEventState::empty(),
    }
}

fn encode_key_code(code: KeyCode) -> WireKeyCode {
    match code {
        KeyCode::Backspace => WireKeyCode::Backspace,
        KeyCode::Enter => WireKeyCode::Enter,
        KeyCode::Left => WireKeyCode::Left,
        KeyCode::Right => WireKeyCode::Right,
        KeyCode::Up => WireKeyCode::Up,
        KeyCode::Down => WireKeyCode::Down,
        KeyCode::Home => WireKeyCode::Home,
        KeyCode::End => WireKeyCode::End,
        KeyCode::PageUp => WireKeyCode::PageUp,
        KeyCode::PageDown => WireKeyCode::PageDown,
        KeyCode::Tab => WireKeyCode::Tab,
        KeyCode::BackTab => WireKeyCode::BackTab,
        KeyCode::Delete => WireKeyCode::Delete,
        KeyCode::Insert => WireKeyCode::Insert,
        KeyCode::Esc => WireKeyCode::Esc,
        KeyCode::CapsLock => WireKeyCode::CapsLock,
        KeyCode::ScrollLock => WireKeyCode::ScrollLock,
        KeyCode::NumLock => WireKeyCode::NumLock,
        KeyCode::PrintScreen => WireKeyCode::PrintScreen,
        KeyCode::Pause => WireKeyCode::Pause,
        KeyCode::Menu => WireKeyCode::Menu,
        KeyCode::KeypadBegin => WireKeyCode::KeypadBegin,
        KeyCode::F(n) => WireKeyCode::F(n),
        KeyCode::Char(c) => WireKeyCode::Char(c),
        KeyCode::Null => WireKeyCode::Null,
        // Media / Modifier variants are not used by dux's input dispatch;
        // collapse to Null so the downstream handler treats them as no-ops.
        KeyCode::Media(_) | KeyCode::Modifier(_) => WireKeyCode::Null,
    }
}

fn decode_key_code(code: &WireKeyCode) -> KeyCode {
    match code {
        WireKeyCode::Backspace => KeyCode::Backspace,
        WireKeyCode::Enter => KeyCode::Enter,
        WireKeyCode::Left => KeyCode::Left,
        WireKeyCode::Right => KeyCode::Right,
        WireKeyCode::Up => KeyCode::Up,
        WireKeyCode::Down => KeyCode::Down,
        WireKeyCode::Home => KeyCode::Home,
        WireKeyCode::End => KeyCode::End,
        WireKeyCode::PageUp => KeyCode::PageUp,
        WireKeyCode::PageDown => KeyCode::PageDown,
        WireKeyCode::Tab => KeyCode::Tab,
        WireKeyCode::BackTab => KeyCode::BackTab,
        WireKeyCode::Delete => KeyCode::Delete,
        WireKeyCode::Insert => KeyCode::Insert,
        WireKeyCode::Esc => KeyCode::Esc,
        WireKeyCode::CapsLock => KeyCode::CapsLock,
        WireKeyCode::ScrollLock => KeyCode::ScrollLock,
        WireKeyCode::NumLock => KeyCode::NumLock,
        WireKeyCode::PrintScreen => KeyCode::PrintScreen,
        WireKeyCode::Pause => KeyCode::Pause,
        WireKeyCode::Menu => KeyCode::Menu,
        WireKeyCode::KeypadBegin => KeyCode::KeypadBegin,
        WireKeyCode::F(n) => KeyCode::F(*n),
        WireKeyCode::Char(c) => KeyCode::Char(*c),
        WireKeyCode::Null => KeyCode::Null,
    }
}

fn encode_kind(k: KeyEventKind) -> WireKeyKind {
    match k {
        KeyEventKind::Press => WireKeyKind::Press,
        KeyEventKind::Repeat => WireKeyKind::Repeat,
        KeyEventKind::Release => WireKeyKind::Release,
    }
}

fn decode_kind(k: WireKeyKind) -> KeyEventKind {
    match k {
        WireKeyKind::Press => KeyEventKind::Press,
        WireKeyKind::Repeat => KeyEventKind::Repeat,
        WireKeyKind::Release => KeyEventKind::Release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_char_with_ctrl_modifier() {
        let orig = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let wire = from_crossterm(orig);
        let back = to_crossterm(&wire);
        assert_eq!(orig.code, back.code);
        assert_eq!(orig.modifiers, back.modifiers);
    }

    #[test]
    fn roundtrip_function_key_preserves_index() {
        let orig = KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE);
        let back = to_crossterm(&from_crossterm(orig));
        assert_eq!(back.code, KeyCode::F(9));
    }

    #[test]
    fn roundtrip_press_kind() {
        let mut orig = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        orig.kind = KeyEventKind::Press;
        let back = to_crossterm(&from_crossterm(orig));
        assert_eq!(back.kind, KeyEventKind::Press);
    }

    #[test]
    fn media_keycode_collapses_to_null() {
        let orig = KeyEvent::new(
            KeyCode::Media(crossterm::event::MediaKeyCode::PlayPause),
            KeyModifiers::empty(),
        );
        let wire = from_crossterm(orig);
        assert_eq!(wire.code, WireKeyCode::Null);
    }
}
