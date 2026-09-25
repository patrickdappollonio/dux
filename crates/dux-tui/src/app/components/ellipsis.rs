//! Cutting user-visible text to a column budget, and marking the cut.
//!
//! Every function here walks the text in extended grapheme clusters, the unit
//! ratatui draws in, and measures each with the wrapper's own
//! [`cluster_width`], so a CJK glyph or an emoji sequence counts as the columns
//! it is drawn in and the result never overflows the budget it was cut for. A
//! byte count or a character count against a column budget is exactly the bug
//! these exist to prevent: a byte slice panics inside a multi-byte character,
//! and a character count lets a row of wide glyphs come back twice as wide as
//! its column, or an emoji presentation selector one column wider than
//! measured.
//!
//! Walking clusters also means a combining mark stays on its letter and an
//! emoji sequence (a ZWJ family, a flag, a keycap, a skin tone) is kept or
//! dropped whole, never cut into a dangling joiner.
//!
//! The cut mark is always the single-cell `…`. When a wide glyph does not fit the
//! last cell before the mark, the mark follows the kept text directly and the
//! result is one column narrower than the budget, rather than splitting the
//! glyph.

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use super::wrap_lines::{cluster_width, display_width};

/// The mark every cut shows, one cell wide.
pub(crate) const ELLIPSIS: &str = "\u{2026}";

/// `text` split into extended grapheme clusters, each paired with its width.
fn clusters(text: &str) -> Vec<(&str, usize)> {
    text.graphemes(true)
        .map(|cluster| (cluster, cluster_width(cluster)))
        .collect()
}

/// The longest prefix of `text` at most `max` columns wide, with no mark.
pub(crate) fn truncate_to_width(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for (cluster, width) in clusters(text) {
        if used + width > max {
            break;
        }
        out.push_str(cluster);
        used += width;
    }
    out
}

/// The longest suffix of `text` at most `max` columns wide, with no mark.
fn tail_within_width(text: &str, max: usize) -> String {
    let all = clusters(text);
    let mut used = 0usize;
    let mut first = all.len();
    for (index, (_, width)) in all.iter().enumerate().rev() {
        if used + width > max {
            break;
        }
        used += width;
        first = index;
    }
    all[first..].iter().map(|(cluster, _)| *cluster).collect()
}

/// `text` cut at its END to at most `max` columns, marking the cut with `…`.
/// Unchanged when it already fits; a budget of zero yields nothing.
pub(crate) fn ellipsize_end(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = truncate_to_width(text, max - 1);
    out.push_str(ELLIPSIS);
    out
}

/// `text` cut at its START to at most `max` columns, keeping the tail behind a
/// leading `…`. For paths, whose leaf is the informative part.
pub(crate) fn ellipsize_start(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    format!("{ELLIPSIS}{}", tail_within_width(text, max - 1))
}

/// `text` cut in its MIDDLE to at most `max` columns, keeping both ends around
/// one `…`. The head gets the smaller half; any column a wide glyph leaves
/// unused at the head is given to the tail.
pub(crate) fn ellipsize_middle(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1;
    let head = truncate_to_width(text, budget / 2);
    let tail = tail_within_width(text, budget - display_width(&head));
    format!("{head}{ELLIPSIS}{tail}")
}

/// `text` followed by enough spaces to fill `width` columns, for a column in a
/// row of columns. Padding with `format!("{:width$}")` counts characters, which
/// leaves a wide-glyph name short and pushes every column after it out of line.
/// Text already at least `width` wide is returned unchanged.
pub(crate) fn pad_to_width(text: &str, width: usize) -> String {
    let used = display_width(text);
    let mut out = String::with_capacity(text.len() + width.saturating_sub(used));
    out.push_str(text);
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    out
}

/// `text` cut to `width` columns and padded to exactly `width`: one cell of a
/// table row whose next column must start at a fixed place.
pub(crate) fn fit_to_width(text: &str, width: usize) -> String {
    pad_to_width(&ellipsize_end(text, width), width)
}

/// Truncate a line of styled spans to `max_w` columns, appending a single `…`
/// when anything is dropped. Each surviving span keeps its own style, and the
/// mark takes the style of the span it cut into, so it matches the text it
/// replaced. Returns the spans unchanged when they already fit.
pub(crate) fn ellipsize_spans(spans: Vec<Span<'static>>, max_w: u16) -> Vec<Span<'static>> {
    let total: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    if total <= usize::from(max_w) {
        return spans;
    }
    mark_cut_row(spans, max_w)
}

/// The last row a cut kept, marked as cut: its spans are kept up to one column
/// short of `width` and the `…` follows them, so the mark lands after the text
/// when the row has room and replaces its end when it is full. For a block of
/// rows cut to fit, where the text that did not fit is on rows that are gone
/// rather than on this one. A width of zero yields nothing.
pub(crate) fn mark_cut_row(spans: Vec<Span<'static>>, width: u16) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let budget = usize::from(width) - 1;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut mark_style = Style::default();
    for span in spans {
        mark_style = span.style;
        let w = display_width(span.content.as_ref());
        if used + w <= budget {
            used += w;
            out.push(span);
        } else {
            let head = truncate_to_width(span.content.as_ref(), budget - used);
            if !head.is_empty() {
                out.push(Span::styled(head, span.style));
            }
            break;
        }
    }
    out.push(Span::styled(ELLIPSIS, mark_style));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    /// The columns `text` takes when ratatui draws it, which is the truth every
    /// cut here must stay within.
    fn drawn_width(text: &str) -> usize {
        let mut buf = Buffer::empty(Rect::new(0, 0, 200, 1));
        usize::from(buf.set_stringn(0, 0, text, 200, Style::default()).0)
    }

    /// Emoji sequences that are one glyph two columns wide as drawn, though
    /// they are several characters: a presentation selector, a keycap, a ZWJ
    /// family, a regional-indicator flag and a skin tone.
    const SEQUENCES: [&str; 5] = [
        "\u{26a0}\u{fe0f}",
        "1\u{fe0f}\u{20e3}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        "\u{1f1ef}\u{1f1f5}",
        "\u{1f44d}\u{1f3fd}",
    ];

    #[test]
    fn a_presentation_selector_is_measured_as_drawn() {
        let text = "\u{26a0}\u{fe0f} Fix the build \u{26a0}\u{fe0f}";
        let cut = ellipsize_end(text, 8);
        assert_eq!(cut, "\u{26a0}\u{fe0f} Fix …");
        assert!(
            drawn_width(&cut) <= 8,
            "{cut:?} draws {}",
            drawn_width(&cut)
        );
    }

    #[test]
    fn emoji_sequences_are_one_glyph_and_never_split() {
        for glyph in SEQUENCES {
            assert_eq!(display_width(glyph), 2, "{glyph:?}");
            let text = glyph.repeat(5);
            assert_eq!(
                ellipsize_end(&text, 5),
                format!("{glyph}{glyph}…"),
                "{glyph:?}"
            );
            assert_eq!(
                ellipsize_start(&text, 5),
                format!("…{glyph}{glyph}"),
                "{glyph:?}"
            );
            assert_eq!(
                ellipsize_middle(&text, 5),
                format!("{glyph}…{glyph}"),
                "{glyph:?}"
            );
            for max in 0..=12 {
                for cut in [
                    ellipsize_end(&text, max),
                    ellipsize_start(&text, max),
                    ellipsize_middle(&text, max),
                    fit_to_width(&text, max),
                ] {
                    assert!(
                        drawn_width(&cut) <= max,
                        "{glyph:?} at {max}: {cut:?} draws {}",
                        drawn_width(&cut)
                    );
                }
            }
        }
    }

    /// A combining acute accent, zero columns wide.
    const ACUTE: char = '\u{0301}';

    #[test]
    fn ascii_fits_unchanged_and_is_cut_with_one_mark() {
        assert_eq!(ellipsize_end("hello", 5), "hello");
        assert_eq!(ellipsize_end("hello world", 6), "hello…");
        assert_eq!(display_width(&ellipsize_end("hello world", 6)), 6);
    }

    /// Box-drawing and block glyphs are several bytes but one column each, as a
    /// provider's banner copied into a status often is.
    #[test]
    fn multi_byte_single_column_glyphs_are_cut_by_columns() {
        let cut = ellipsize_end("Copied: \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}end", 10);
        assert_eq!(cut, "Copied: \u{2500}…");
        let cut = ellipsize_end("\u{2588}\u{2588}\u{259b}\u{2598} Opus 4.6 (1M context)", 12);
        assert_eq!(display_width(&cut), 12);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn cjk_is_cut_by_columns_never_overflowing() {
        // Six glyphs, twelve columns.
        let text = "日本語の名前";
        assert_eq!(ellipsize_end(text, 12), text);
        // Seven columns: three glyphs (six) plus the mark.
        assert_eq!(ellipsize_end(text, 7), "日本語…");
        // Six columns: the third glyph would need columns five and six, which
        // leaves no cell for the mark, so it goes and the result is five wide.
        assert_eq!(ellipsize_end(text, 6), "日本…");
        for max in 0..=13 {
            assert!(display_width(&ellipsize_end(text, max)) <= max, "max {max}");
        }
    }

    #[test]
    fn emoji_count_two_columns() {
        let text = "🦆🦆🦆🦆";
        assert_eq!(ellipsize_end(text, 5), "🦆🦆…");
        assert_eq!(ellipsize_end(text, 4), "🦆…");
        assert_eq!(ellipsize_start(text, 5), "…🦆🦆");
        assert_eq!(ellipsize_middle(text, 5), "🦆…🦆");
    }

    #[test]
    fn combining_marks_stay_on_their_letter() {
        let text: String = ['e', ACUTE, 'e', ACUTE, 'e', ACUTE, 'e', ACUTE]
            .iter()
            .collect();
        // Four columns of text; three columns keep two letters and the mark.
        let cut = ellipsize_end(&text, 3);
        assert_eq!(cut, format!("e{ACUTE}e{ACUTE}…"));
        // The kept tail never starts with a dangling accent.
        let tail = ellipsize_start(&text, 3);
        assert_eq!(tail, format!("…e{ACUTE}e{ACUTE}"));
        let middle = ellipsize_middle(&text, 3);
        assert_eq!(middle, format!("e{ACUTE}…e{ACUTE}"));
    }

    #[test]
    fn width_zero_yields_nothing_and_width_one_only_the_mark() {
        for f in [ellipsize_end, ellipsize_start, ellipsize_middle] {
            assert_eq!(f("日本語", 0), "");
            assert_eq!(f("hello", 0), "");
            assert_eq!(f("hello", 1), "…");
            assert_eq!(f("日本語", 1), "…");
            assert_eq!(f("", 0), "");
            assert_eq!(f("", 1), "");
            assert_eq!(f("a", 1), "a");
        }
        assert_eq!(truncate_to_width("日本", 1), "");
        assert_eq!(truncate_to_width("日本", 0), "");
    }

    #[test]
    fn start_keeps_the_tail() {
        assert_eq!(ellipsize_start("proj/app", 12), "proj/app");
        let out = ellipsize_start("/home/patrick/code/proj", 10);
        assert_eq!(out, "…code/proj");
        assert_eq!(ellipsize_start("/home/日本語/プロジェクト", 9), "…ジェクト");
    }

    #[test]
    fn middle_keeps_both_ends_and_gives_the_head_the_smaller_half() {
        assert_eq!(
            ellipsize_middle("src/components/app.rs", 12),
            "src/c…app.rs"
        );
        assert_eq!(ellipsize_middle("日本語のとても長い名前", 9), "日本…名前");
        // The head's three columns hold one glyph; the column it could not use
        // goes to the tail, which then keeps five letters rather than four.
        assert_eq!(ellipsize_middle("日本語abcdef", 8), "日…bcdef");
        for max in 0..=24 {
            let out = ellipsize_middle("日本語のとても長い名前", max);
            assert!(display_width(&out) <= max, "max {max}: {out}");
        }
    }

    #[test]
    fn padding_counts_columns() {
        assert_eq!(pad_to_width("日本", 6), "日本  ");
        assert_eq!(pad_to_width("abc", 2), "abc");
        assert_eq!(display_width(&fit_to_width("日本語の名前", 7)), 7);
        assert_eq!(fit_to_width("日本語の名前", 6), "日本… ");
        assert_eq!(fit_to_width("ab", 4), "ab  ");
    }

    #[test]
    fn spans_keep_styles_and_mark_in_the_cut_span_style() {
        let red = Style::default().fg(Color::Red);
        let blue = Style::default().fg(Color::Blue);
        let spans = vec![Span::styled("abc", red), Span::styled("日本語", blue)];
        let out = ellipsize_spans(spans.clone(), 20);
        assert_eq!(out, spans);
        let out = ellipsize_spans(spans.clone(), 6);
        let text: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abc日…");
        assert_eq!(out.last().unwrap().style, blue);
        assert!(ellipsize_spans(spans, 0).is_empty());
    }

    #[test]
    fn a_cut_row_is_marked_after_short_text_and_at_the_end_of_a_full_one() {
        let row = |text: &str| vec![Span::raw(text.to_string())];
        let text = |spans: Vec<Span<'static>>| -> String {
            spans.iter().map(|s| s.content.as_ref()).collect()
        };
        assert_eq!(text(mark_cut_row(row("short"), 10)), "short…");
        assert_eq!(text(mark_cut_row(row("日本語の名前"), 10)), "日本語の…");
        assert_eq!(text(mark_cut_row(row("日本語の名前"), 12)), "日本語の名…");
        assert_eq!(text(mark_cut_row(row("abc"), 1)), "…");
        assert!(mark_cut_row(row("abc"), 0).is_empty());
    }
}
