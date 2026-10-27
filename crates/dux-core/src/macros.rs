//! Shared text-macro helpers.
//!
//! The byte transform ([`macro_payload_bytes`]) and the surface-filter predicate
//! ([`macro_matches_surface`]) live in core, beside the macro data they read.
//! The web's macro picker mirrors both in its own client code. The macro DATA model ([`crate::config::MacroEntry`],
//! [`crate::config::MacroSurface`]) stays in `config.rs` with the rest of the
//! serde-persisted config.

use crate::config::MacroSurface;
use crate::model::SessionSurface;

/// Build the byte payload for a macro send. Newlines are translated to
/// Alt+Enter (ESC followed by CR) so that multi-line macros are entered
/// as a single multi-line prompt rather than submitting at each newline.
/// Handles `\r\n`, `\n`, and bare `\r` uniformly.
pub fn macro_payload_bytes(text: &str) -> Vec<u8> {
    const ALT_ENTER: &[u8] = b"\x1b\r";
    let bytes = text.as_bytes();
    let mut payload = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                payload.extend_from_slice(ALT_ENTER);
                i += 2;
            }
            b'\n' | b'\r' => {
                payload.extend_from_slice(ALT_ENTER);
                i += 1;
            }
            b => {
                payload.push(b);
                i += 1;
            }
        }
    }
    payload
}

/// Whether a macro of `macro_surface` is available on a target of
/// `target_surface`: the surface gate of the TUI's macro bar.
pub fn macro_matches_surface(macro_surface: MacroSurface, target_surface: SessionSurface) -> bool {
    macro_surface.matches(target_surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_payload_translates_newlines_to_alt_enter() {
        // Plain text with no newlines passes through byte-for-byte.
        assert_eq!(macro_payload_bytes("hello").as_slice(), b"hello");

        // Bare LF (\n) becomes ESC + CR.
        assert_eq!(macro_payload_bytes("a\nb").as_slice(), b"a\x1b\rb");

        // Bare CR (\r) becomes ESC + CR.
        assert_eq!(macro_payload_bytes("a\rb").as_slice(), b"a\x1b\rb");

        // CRLF collapses to a single ESC + CR (not two).
        assert_eq!(macro_payload_bytes("a\r\nb").as_slice(), b"a\x1b\rb");

        // Multiple newlines each become their own ESC + CR.
        assert_eq!(
            macro_payload_bytes("a\nb\nc").as_slice(),
            b"a\x1b\rb\x1b\rc"
        );

        // Trailing and leading newlines are also translated.
        assert_eq!(macro_payload_bytes("\na\n").as_slice(), b"\x1b\ra\x1b\r");

        // Empty input yields empty payload.
        assert_eq!(macro_payload_bytes(""), Vec::<u8>::new());
    }

    #[test]
    fn macro_matches_surface_mirrors_entry_matches() {
        // Both is available on every target surface.
        assert!(macro_matches_surface(
            MacroSurface::Both,
            SessionSurface::Agent
        ));
        assert!(macro_matches_surface(
            MacroSurface::Both,
            SessionSurface::Terminal
        ));
        // Agent only matches an agent target.
        assert!(macro_matches_surface(
            MacroSurface::Agent,
            SessionSurface::Agent
        ));
        assert!(!macro_matches_surface(
            MacroSurface::Agent,
            SessionSurface::Terminal
        ));
        // Terminal only matches a terminal target.
        assert!(macro_matches_surface(
            MacroSurface::Terminal,
            SessionSurface::Terminal
        ));
        assert!(!macro_matches_surface(
            MacroSurface::Terminal,
            SessionSurface::Agent
        ));
    }
}
