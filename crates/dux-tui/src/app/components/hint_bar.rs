//! The one key-hint line: the `<key> what it does` row a dialog paints along
//! its bottom edge, a pane paints under its content, and the footer paints
//! above the status line.
//!
//! The shape is `key badge` + ` ` + `description` segments separated by two
//! spaces, with `<a>/<b> description` for two keys that do the same thing. A
//! dialog's line starts with one space so it clears the corner of the frame it
//! sits on. Three rules are structural here:
//!
//! * **A segment whose key has no honest label is dropped, not blanked.** Every
//!   binding is user-configurable, so a lookup can legitimately come back
//!   empty, and a dialog with a text field additionally has to skip any key the
//!   field swallows (see `keybindings::text_field_owns_key` and
//!   `RuntimeBindings::label_for_text_field_dialog`). Naming a key that types a
//!   character is worse than naming none.
//! * **A label is never hardcoded.** [`Hint::key`] and [`Hint::keys`] take
//!   labels the caller resolved through the bindings. [`Hint::fixed`] is for a
//!   key the surface handles WITHOUT a binding (a text-input context's literal
//!   Enter, Tab or Escape, a mouse gesture), which is therefore named as it is;
//!   [`Hint::plain`] is prose with no badge, for Space acting on the focused
//!   control (the accessibility tenet).
//! * **A badge sits on the surface it is painted over.** It carries no
//!   background of its own, so the same line reads right on a dialog, a pane
//!   and the footer bar in every theme.
//!
//! [`HintTone`] picks the colors: a dialog or bar speaks in the full hint
//! colors, a pane's hint row under live content in the dimmed ones.
//!
//! Pure: takes a [`Theme`], returns spans, touches no `App` state.

use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::text::{Line, Span};

// The mark of a cut and the column measure are the shared ones every other cut
// in the terminal UI uses.
use crate::app::components::ellipsis::ELLIPSIS;
use crate::app::components::wrap_lines::display_width;
use crate::theme::Theme;

/// The separator between two segments.
const SEPARATOR: &str = "  ";

/// Which colors a hint line speaks in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HintTone {
    /// A dialog's edge, a bar, the footer: the full hint colors.
    Modal,
    /// A pane's hint row under content that is not the hint's (a terminal, a
    /// diff, a file list): the dimmed hint colors, so it does not compete.
    Pane,
}

/// What a segment shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// A key badge followed by what the key does. An empty key drops the
    /// whole segment.
    Key {
        key: String,
        desc: Cow<'static, str>,
    },
    /// Two or more keys that do the same thing, `<a>/<b> desc`. Empty keys are
    /// left out, and the segment drops when none is left.
    Keys {
        keys: Vec<String>,
        desc: Cow<'static, str>,
    },
    /// Prose with no badge.
    Plain(Cow<'static, str>),
}

/// One segment of a hint line, and whether it survives a cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    segment: Segment,
    /// A pinned segment is the last to go when the line is too long for its
    /// space: the way out of the surface (close, cancel, minimize) is pinned,
    /// so a narrow window loses the conveniences and keeps the exit.
    pinned: bool,
}

impl Hint {
    fn new(segment: Segment) -> Self {
        Self {
            segment,
            pinned: false,
        }
    }

    /// A bound key and its description. `key` is whatever the bindings
    /// returned; pass the empty string (or use [`Hint::maybe_key`]) when there
    /// is none and the segment should vanish.
    pub(crate) fn key(key: impl Into<String>, desc: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Segment::Key {
            key: key.into(),
            desc: desc.into(),
        })
    }

    /// The `Option`-shaped form, for lookups that already return `None` when no
    /// honest label exists (`label_for_text_field_dialog`).
    pub(crate) fn maybe_key(
        key: Option<impl Into<String>>,
        desc: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::key(key.map(Into::into).unwrap_or_default(), desc)
    }

    /// Several bound keys that do the same thing.
    pub(crate) fn keys<K: Into<String>>(
        keys: impl IntoIterator<Item = K>,
        desc: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::new(Segment::Keys {
            keys: keys.into_iter().map(Into::into).collect(),
            desc: desc.into(),
        })
    }

    /// A key the surface handles without a binding (a text-input context's
    /// literal key, a mouse gesture), named as it is because no rebind can
    /// change it. Never for a key that has a binding.
    pub(crate) fn fixed(key: &'static str, desc: impl Into<Cow<'static, str>>) -> Self {
        Self::key(key, desc)
    }

    /// [`Hint::fixed`] for several keys that do the same thing.
    pub(crate) fn fixed_keys<const N: usize>(
        keys: [&'static str; N],
        desc: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::keys(keys, desc)
    }

    /// Prose with no key badge.
    pub(crate) fn plain(text: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Segment::Plain(text.into()))
    }

    /// Keep this segment when the line has to be cut: for the way out.
    pub(crate) fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    fn keys_shown(&self) -> Vec<&str> {
        match &self.segment {
            Segment::Key { key, .. } if key.is_empty() => Vec::new(),
            Segment::Key { key, .. } => vec![key.as_str()],
            Segment::Keys { keys, .. } => keys
                .iter()
                .map(String::as_str)
                .filter(|k| !k.is_empty())
                .collect(),
            Segment::Plain(_) => Vec::new(),
        }
    }

    fn is_renderable(&self) -> bool {
        match &self.segment {
            Segment::Key { .. } | Segment::Keys { .. } => !self.keys_shown().is_empty(),
            Segment::Plain(text) => !text.is_empty(),
        }
    }

    /// Display columns the segment takes, separator excluded.
    fn width(&self) -> usize {
        match &self.segment {
            Segment::Key { desc, .. } | Segment::Keys { desc, .. } => {
                let keys = self.keys_shown();
                // `<` and `>` around each key, a `/` between two, the space
                // before the description.
                let badges: usize = keys.iter().map(|k| display_width(k) + 2).sum();
                badges + keys.len().saturating_sub(1) + 1 + display_width(desc)
            }
            Segment::Plain(text) => display_width(text),
        }
    }

    fn push_spans(&self, theme: &Theme, tone: HintTone, spans: &mut Vec<Span<'static>>) {
        let desc_style = desc_style(theme, tone);
        match &self.segment {
            Segment::Key { desc, .. } | Segment::Keys { desc, .. } => {
                for (index, key) in self.keys_shown().into_iter().enumerate() {
                    if index > 0 {
                        spans.push(Span::styled("/", desc_style));
                    }
                    let badge = match tone {
                        HintTone::Modal => theme.key_badge_default(key),
                        HintTone::Pane => theme.dim_key_badge_default(key),
                    };
                    spans.extend(
                        badge
                            .into_iter()
                            .map(|span| Span::styled(span.content.into_owned(), span.style)),
                    );
                }
                spans.push(Span::styled(format!(" {desc}"), desc_style));
            }
            Segment::Plain(text) => spans.push(Span::styled(text.to_string(), desc_style)),
        }
    }
}

fn desc_style(theme: &Theme, tone: HintTone) -> Style {
    Style::default().fg(match tone {
        HintTone::Modal => theme.hint_desc_fg,
        HintTone::Pane => theme.hint_dim_desc_fg,
    })
}

/// A line cut to a width: its spans, and how many segments made it in.
pub(crate) struct FittedHints {
    pub(crate) spans: Vec<Span<'static>>,
    pub(crate) shown: usize,
}

/// The segments that fit in `width` display columns, in their order.
///
/// When the whole line does not fit, segments leave from the right, unpinned
/// ones first, so what goes is the conveniences in the middle and what stays is
/// the first segments and the way out. Where segments were left out the line
/// carries a `…` of its own, joined like any other segment, so a cut line says
/// so rather than reading as complete. Nothing is ever cut through the middle.
pub(crate) fn fitted_hint_spans(
    theme: &Theme,
    tone: HintTone,
    hints: &[Hint],
    width: usize,
) -> FittedHints {
    let hints: Vec<&Hint> = hints.iter().filter(|hint| hint.is_renderable()).collect();
    let mut kept = vec![true; hints.len()];
    let cost = |kept: &[bool]| {
        let dropped = kept.iter().any(|keep| !keep);
        let widths = hints
            .iter()
            .zip(kept)
            .filter(|(_, keep)| **keep)
            .map(|(hint, _)| hint.width())
            .chain(dropped.then(|| display_width(ELLIPSIS)));
        let (count, total) = widths.fold((0usize, 0usize), |(n, sum), w| (n + 1, sum + w));
        total + count.saturating_sub(1) * SEPARATOR.len()
    };
    while cost(&kept) > width {
        let victim = (0..hints.len())
            .rev()
            .find(|&index| kept[index] && !hints[index].pinned)
            .or_else(|| (0..hints.len()).rev().find(|&index| kept[index]));
        let Some(victim) = victim else {
            break;
        };
        kept[victim] = false;
    }

    let mut spans = Vec::new();
    let mut shown = 0usize;
    let mut items = 0usize;
    let mut marked = false;
    let fits = cost(&kept) <= width;
    for (hint, keep) in hints.iter().zip(&kept) {
        if !keep && (marked || !fits) {
            continue;
        }
        if items > 0 {
            spans.push(Span::styled(SEPARATOR, desc_style(theme, tone)));
        }
        items += 1;
        if *keep {
            hint.push_spans(theme, tone, &mut spans);
            shown += 1;
        } else {
            spans.push(Span::styled(ELLIPSIS, desc_style(theme, tone)));
            marked = true;
        }
    }
    FittedHints { spans, shown }
}

/// A dialog's hint line in `width` columns: one leading space, then the
/// segments that fit.
pub(crate) fn modal_hint_line(theme: &Theme, hints: &[Hint], width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(
        fitted_hint_spans(
            theme,
            HintTone::Modal,
            hints,
            usize::from(width).saturating_sub(1),
        )
        .spans,
    );
    Line::from(spans)
}

/// A hint row of its own in `width` columns (under a pane's content, or on
/// a dialog's own hint row rather than its frame): the segments that fit,
/// flush left, in `tone`.
pub(crate) fn hint_line(
    theme: &Theme,
    tone: HintTone,
    hints: &[Hint],
    width: u16,
) -> Line<'static> {
    Line::from(fitted_hint_spans(theme, tone, hints, usize::from(width)).spans)
}

/// [`hint_line`] in the pane tone.
pub(crate) fn pane_hint_line(theme: &Theme, hints: &[Hint], width: u16) -> Line<'static> {
    hint_line(theme, HintTone::Pane, hints, width)
}

/// A hint row that opens with a sentence of its own (how far the view is
/// scrolled back, why keys are not reaching the child), set off from the
/// segments by the same separator that joins them. The sentence is kept whole;
/// the segments fit in what is left of `width`.
pub(crate) fn hint_line_after(
    theme: &Theme,
    tone: HintTone,
    lead: Span<'static>,
    hints: &[Hint],
    width: u16,
) -> Line<'static> {
    let room = usize::from(width)
        .saturating_sub(display_width(&lead.content))
        .saturating_sub(SEPARATOR.len());
    let rest = fitted_hint_spans(theme, tone, hints, room).spans;
    let mut spans = vec![lead];
    if !rest.is_empty() {
        spans.push(Span::styled(SEPARATOR, desc_style(theme, tone)));
        spans.extend(rest);
    }
    Line::from(spans)
}

/// [`hint_line_after`] in the pane tone.
pub(crate) fn pane_hint_line_after(
    theme: &Theme,
    lead: Span<'static>,
    hints: &[Hint],
    width: u16,
) -> Line<'static> {
    hint_line_after(theme, HintTone::Pane, lead, hints, width)
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
            200,
        );
        let without = modal_hint_line(
            &theme,
            &[
                Hint::key("Enter", "confirm"),
                Hint::maybe_key(None::<String>, "focus"),
                Hint::key("Esc", "cancel"),
            ],
            200,
        );
        assert!(text_of(&with).contains("focus"));
        // Not blanked into a stray gap: the separator goes with the segment.
        assert!(!text_of(&without).contains("focus"));
        assert!(!text_of(&without).contains("   "));
    }

    #[test]
    fn segments_are_separated_by_two_spaces_after_one_leading_space() {
        let line = modal_hint_line(
            &theme(),
            &[Hint::key("a", "one"), Hint::key("b", "two")],
            200,
        );
        let text = text_of(&line);
        // The badge wraps the key, so assert on the joins rather than the glyphs.
        assert!(text.starts_with(' '), "leading space, got {text:?}");
        assert!(text.contains(" one  "), "two-space join, got {text:?}");
        assert!(text.ends_with(" two"), "no trailing pad, got {text:?}");
    }

    #[test]
    fn a_plain_segment_carries_no_badge() {
        let badged = modal_hint_line(&theme(), &[Hint::key("Space", "toggle")], 200);
        let plain = modal_hint_line(&theme(), &[Hint::plain("Space toggle")], 200);
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
            200,
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
            200,
        );
        assert_eq!(
            text_of(&without_focus),
            " <Enter> confirm  Space toggle  <Esc> cancel"
        );
    }

    /// No span of a hint line names a background, so every cell takes the
    /// background of the surface under it, in either tone.
    #[test]
    fn no_span_carries_a_background_of_its_own() {
        let theme = theme();
        let hints = [
            Hint::key("Enter", "confirm"),
            Hint::keys(["PgUp", "PgDn"], "scroll"),
            Hint::plain("Space toggle"),
        ];
        for tone in [HintTone::Modal, HintTone::Pane] {
            for span in fitted_hint_spans(&theme, tone, &hints, 200).spans {
                assert_eq!(span.style.bg, None, "{tone:?}: {span:?}");
            }
        }
    }

    /// The pane tone is the dimmed hint colors, the modal tone the full ones.
    #[test]
    fn the_tone_picks_the_badge_and_description_colors() {
        let theme = theme();
        let hints = [Hint::key("Esc", "close")];
        let modal = fitted_hint_spans(&theme, HintTone::Modal, &hints, 200).spans;
        let pane = fitted_hint_spans(&theme, HintTone::Pane, &hints, 200).spans;
        assert_eq!(modal[0].style.fg, Some(theme.hint_bracket_fg));
        assert_eq!(modal[1].style.fg, Some(theme.hint_key_fg));
        assert_eq!(modal[3].style.fg, Some(theme.hint_desc_fg));
        assert_eq!(pane[0].style.fg, Some(theme.hint_dim_bracket_fg));
        assert_eq!(pane[1].style.fg, Some(theme.hint_dim_key_fg));
        assert_eq!(pane[3].style.fg, Some(theme.hint_dim_desc_fg));
    }

    /// Two keys that do one thing share one description, joined by a slash, and
    /// an unbound one of them leaves no stray slash behind.
    #[test]
    fn keys_that_do_one_thing_share_a_description() {
        let theme = theme();
        let both = modal_hint_line(&theme, &[Hint::keys(["Tab", "S-Tab"], "actions")], 200);
        assert_eq!(text_of(&both), " <Tab>/<S-Tab> actions");
        let one = modal_hint_line(&theme, &[Hint::keys(["", "S-Tab"], "actions")], 200);
        assert_eq!(text_of(&one), " <S-Tab> actions");
        let none = modal_hint_line(
            &theme,
            &[Hint::keys(["", ""], "actions"), Hint::key("Esc", "close")],
            200,
        );
        assert_eq!(text_of(&none), " <Esc> close");
    }

    /// A pane line with a sentence of its own sets it off with the separator,
    /// and a sentence with no segments after it trails nothing.
    #[test]
    fn a_lead_sentence_is_set_off_by_the_separator() {
        let theme = theme();
        let line = pane_hint_line_after(
            &theme,
            Span::raw("Scrolled back 3 lines."),
            &[Hint::key("PgDn", "down")],
            200,
        );
        assert_eq!(text_of(&line), "Scrolled back 3 lines.  <PgDn> down");
        let alone = pane_hint_line_after(&theme, Span::raw("Exited."), &[Hint::key("", "x")], 200);
        assert_eq!(text_of(&alone), "Exited.");
    }

    /// Fitting keeps whole segments only, marks the cut with an ellipsis, and
    /// measures display columns, so a wide description cannot overflow.
    #[test]
    fn fitting_keeps_whole_segments_and_marks_the_cut() {
        let theme = theme();
        let hints = [
            Hint::key("a", "one"),
            Hint::key("b", "\u{5e45}\u{5e45}"),
            Hint::key("c", "three"),
        ];
        let full = fitted_hint_spans(&theme, HintTone::Modal, &hints, 80);
        let full_text: String = full.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(full_text, "<a> one  <b> \u{5e45}\u{5e45}  <c> three");
        assert_eq!(full.shown, 3);

        // `<a> one` is 7 columns, `  <b> 幅幅` another 10 and the mark of the
        // cut another 3: 20 fit exactly, 19 do not, even though the wide
        // segment is only 8 characters.
        let cut = fitted_hint_spans(&theme, HintTone::Modal, &hints, 20);
        let cut_text: String = cut.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(cut_text, "<a> one  <b> \u{5e45}\u{5e45}  \u{2026}");
        assert_eq!(cut.shown, 2);
        let cut = fitted_hint_spans(&theme, HintTone::Modal, &hints, 19);
        let cut_text: String = cut.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(cut_text, "<a> one  \u{2026}");
        assert_eq!(cut.shown, 1);

        let nothing = fitted_hint_spans(&theme, HintTone::Modal, &hints, 0);
        assert!(nothing.spans.is_empty());
        assert_eq!(nothing.shown, 0);
    }

    /// A pinned segment outlives the segments before it: the unpinned ones go
    /// from the right first, and the mark of the cut sits where they were.
    #[test]
    fn a_pinned_segment_is_the_last_to_go() {
        let theme = theme();
        let hints = [
            Hint::key("a", "one"),
            Hint::key("b", "two"),
            Hint::key("c", "three"),
            Hint::key("Esc", "close").pinned(),
        ];
        let line = modal_hint_line(&theme, &hints, 33);
        assert_eq!(text_of(&line), " <a> one  <b> two  \u{2026}  <Esc> close");
        let line = modal_hint_line(&theme, &hints, 32);
        assert_eq!(text_of(&line), " <a> one  \u{2026}  <Esc> close");
        let line = modal_hint_line(&theme, &hints, 16);
        assert_eq!(text_of(&line), " \u{2026}  <Esc> close");
        // Only when even the pinned segment cannot fit does it go too.
        let line = modal_hint_line(&theme, &hints, 8);
        assert_eq!(text_of(&line), " \u{2026}");
    }
}
