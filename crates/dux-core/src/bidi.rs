//! Removing the Unicode controls that reorder how text is displayed from text
//! dux did not write.
//!
//! A pull request title is written by whoever opened the pull request. The
//! embedding, override and isolate controls (U+202A to U+202E, U+2066 to
//! U+2069) and the directional marks (U+200E, U+200F, U+061C) make a terminal
//! or a browser draw characters in an order other than the one they are stored
//! in, so a crafted title can make the words around it in a dialog read as
//! something they do not say. dux strips them where such text enters, on both
//! surfaces: here for the terminal UI and everything the wire carries, and in
//! the browser's `lib/bidi.ts` where pull request data enters its store.

/// Whether `c` is a bidirectional formatting control or directional mark.
pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
    )
}

/// `text` with every bidirectional control removed.
pub fn strip_bidi_controls(text: &str) -> String {
    text.chars().filter(|c| !is_bidi_control(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedding_override_isolate_and_mark_is_removed() {
        let controls = [
            '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}',
            '\u{2068}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}',
        ];
        for control in controls {
            assert!(is_bidi_control(control), "{control:?}");
            assert_eq!(strip_bidi_controls(&format!("a{control}b")), "ab");
        }
    }

    #[test]
    fn an_override_crafted_title_reads_in_stored_order() {
        // Displayed, the override would draw "Fix login" followed by "txt.exe"
        // reversed; stripped, the title is exactly its stored characters.
        assert_eq!(
            strip_bidi_controls("Fix login \u{202E}exe.txt\u{202C}"),
            "Fix login exe.txt"
        );
    }

    #[test]
    fn right_to_left_text_itself_is_kept() {
        assert_eq!(strip_bidi_controls("תיקון באג"), "תיקון באג");
        assert_eq!(strip_bidi_controls("إصلاح"), "إصلاح");
        assert!(
            !is_bidi_control('\u{200D}'),
            "the zero-width joiner shapes emoji"
        );
    }
}
