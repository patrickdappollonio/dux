//! Shared text-macro helpers used by both surfaces.
//!
//! The byte transform ([`macro_payload_bytes`]) and the surface-filter predicate
//! ([`macro_matches_surface`]) live here — in core, not in the TUI — so a macro
//! sent from the web (via a `WireCommand`) and a macro sent from the TUI produce
//! BYTE-IDENTICAL input to the same PTY, with parity guaranteed by construction
//! rather than by keeping two copies in sync. The macro DATA model
//! ([`crate::config::MacroEntry`], [`crate::config::MacroSurface`]) stays in
//! `config.rs` next to the rest of the serde-persisted config.

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
/// `target_surface`. The pure core of the TUI's `filtered_macros` surface gate
/// (`entry.surface.matches(session_surface)`), extracted so the web's
/// `RunMacro` enforces the SAME surface restriction the TUI's macro bar does.
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
        assert!(macro_payload_bytes("").is_empty());
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
