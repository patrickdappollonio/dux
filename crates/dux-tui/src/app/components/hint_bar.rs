//! The one-line hint footer a modal paints along its bottom edge.
//!
//! Every modal that shows one builds the same shape by hand: a leading space,
//! then `key badge` + ` ` + `description` segments separated by two spaces. The
//! duplication is not the only reason to lift it out. Two rules are easy to get
//! wrong per-copy and are now structural here:
//!
//! * **A segment whose key has no honest label is DROPPED, not blanked.** Every
//!   binding is user-configurable, so a lookup can legitimately come back
//!   empty, and the rename-agent footer additionally has to skip any key its
//!   text field swallows (see `keybindings::text_field_owns_key` and
//!   `RuntimeBindings::label_for_text_field_dialog`). Naming a key that types a
//!   character is worse than naming none.
//! * **A label is never hardcoded.** [`Hint::key`] takes a label the caller
//!   resolved through the bindings. [`Hint::plain`] exists for the one thing
//!   that is genuinely not a binding, Space acting on the focused control,
//!   which is hardcoded on purpose (the accessibility tenet) and so has no
//!   binding to look up.
//!
//! Pure: takes a [`Theme`], returns a [`Line`], touches no `App` state.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// One footer segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Hint {
    /// A key badge followed by what the key does. `key` must already be
    /// resolved through the bindings; an empty one drops the whole segment.
    Key { key: String, desc: &'static str },
    /// Prose with no badge, for a key that has no binding to resolve.
    Plain(&'static str),
}

impl Hint {
    /// A bound key and its description. `key` is whatever the bindings
    /// returned; pass the empty string (or use [`Hint::maybe_key`]) when there
    /// is none and the segment should vanish.
    pub(crate) fn key(key: impl Into<String>, desc: &'static str) -> Self {
        Self::Key {
            key: key.into(),
            desc,
        }
    }

    /// The `Option`-shaped form, for lookups that already return `None` when no
    /// honest label exists (`label_for_text_field_dialog`).
    pub(crate) fn maybe_key(key: Option<impl Into<String>>, desc: &'static str) -> Self {
        Self::Key {
            key: key.map(Into::into).unwrap_or_default(),
            desc,
        }
    }

    /// Prose with no key badge.
    pub(crate) fn plain(text: &'static str) -> Self {
        Self::Plain(text)
    }

    fn is_renderable(&self) -> bool {
        match self {
            Self::Key { key, .. } => !key.is_empty(),
            Self::Plain(text) => !text.is_empty(),
        }
    }
}

/// Build the footer line: a leading space, then every renderable segment
/// separated by two spaces.
///
/// Byte-for-byte the shape the hand-written footers already produce, which is
/// what lets a migrated modal be proved unchanged rather than merely reviewed.
pub(crate) fn modal_hint_line(theme: &Theme, hints: &[Hint]) -> Line<'static> {
    let desc_style = Style::default().fg(theme.hint_desc_fg);
    let mut spans = vec![Span::raw(" ")];
    let mut first = true;
    for hint in hints.iter().filter(|hint| hint.is_renderable()) {
        let separator = if first { "" } else { "  " };
        first = false;
        match hint {
            Hint::Key { key, desc } => {
                if !separator.is_empty() {
                    spans.push(Span::styled(separator.to_string(), desc_style));
                }
                spans.extend(
                    theme
                        .key_badge_default(key)
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style)),
                );
                spans.push(Span::styled(format!(" {desc}"), desc_style));
            }
            Hint::Plain(text) => {
                spans.push(Span::styled(format!("{separator}{text}"), desc_style));
            }
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn theme() -> Theme {
        Theme::default_dark()
    }

    #[test]
    fn a_key_with_no_label_drops_its_whole_segment() {
        let theme = theme();
        let with = modal_hint_line(
            &theme,
            &[
                Hint::key("Enter", "confirm"),
                Hint::key("Tab", "focus"),
                Hint::key("Esc", "cancel"),
            ],
        );
        let without = modal_hint_line(
            &theme,
            &[
                Hint::key("Enter", "confirm"),
                Hint::maybe_key(None::<String>, "focus"),
                Hint::key("Esc", "cancel"),
            ],
        );
        assert!(text_of(&with).contains("focus"));
        // Not blanked into a stray gap: the separator goes with the segment.
        assert!(!text_of(&without).contains("focus"));
        assert!(!text_of(&without).contains("   "));
    }

    #[test]
    fn segments_are_separated_by_two_spaces_after_one_leading_space() {
        let line = modal_hint_line(&theme(), &[Hint::key("a", "one"), Hint::key("b", "two")]);
        let text = text_of(&line);
        // The badge wraps the key, so assert on the joins rather than the glyphs.
        assert!(text.starts_with(' '), "leading space, got {text:?}");
        assert!(text.contains(" one  "), "two-space join, got {text:?}");
        assert!(text.ends_with(" two"), "no trailing pad, got {text:?}");
    }

    #[test]
    fn a_plain_segment_carries_no_badge() {
        let badged = modal_hint_line(&theme(), &[Hint::key("Space", "toggle")]);
        let plain = modal_hint_line(&theme(), &[Hint::plain("Space toggle")]);
        assert_eq!(text_of(&plain), " Space toggle");
        assert_ne!(text_of(&badged), text_of(&plain));
    }

    /// The rename-agent footer, rebuilt from the parts, still reads the way it
    /// reads today, including the case where the focus key is swallowed by the
    /// name field and the segment has to disappear.
    #[test]
    fn reproduces_the_rename_agent_footer_in_both_states() {
        let theme = theme();
        let with_focus = modal_hint_line(
            &theme,
            &[
                Hint::key("Enter", "confirm"),
                Hint::maybe_key(Some("Tab"), "focus"),
                Hint::plain("Space toggle"),
                Hint::key("Esc", "cancel"),
            ],
        );
        assert_eq!(
            text_of(&with_focus),
            " <Enter> confirm  <Tab> focus  Space toggle  <Esc> cancel"
        );
        let without_focus = modal_hint_line(
            &theme,
            &[
                Hint::key("Enter", "confirm"),
                Hint::maybe_key(None::<String>, "focus"),
                Hint::plain("Space toggle"),
                Hint::key("Esc", "cancel"),
            ],
        );
        assert_eq!(
            text_of(&without_focus),
            " <Enter> confirm  Space toggle  <Esc> cancel"
        );
    }
}
