//! Pre-wrapping styled lines so a scroll clamp can be exact.
//!
//! A `Paragraph` rendered with `Wrap { trim: false }` produces MORE rows than it
//! was given lines, and it does not say how many. A surface that clamps its
//! scroll offset with `lines.len()` therefore stops short of its own bottom the
//! moment anything wraps — the help overlay did exactly that, on any terminal
//! narrow enough to wrap a keybinding row.
//!
//! The fix is not to predict ratatui's wrapping: a prediction has to match its
//! algorithm exactly or the clamp is still wrong, just differently. Instead the
//! caller wraps the content HERE, renders the result WITHOUT `Wrap`, and clamps
//! with the returned length — which is then the row count by construction,
//! because every returned line renders on exactly one row.
//!
//! [`wrap_styled_lines`] deliberately reproduces `Wrap { trim: false }`:
//!
//! - Greedy word wrapping at whitespace, in display columns.
//! - The leading whitespace of a line is kept (that is what `trim: false`
//!   means); continuation rows are NOT re-indented, because ratatui does not
//!   re-indent them either.
//! - Whitespace at a break point is dropped, as ratatui's `WordWrapper` drops
//!   the whitespace that fits in the row it just ended. Invisible either way,
//!   except under a background color, where it would be trailing padding.
//! - A word too long to fit a row is hard-broken at the row edge.
//! - Every span keeps its own style; a wrapped row can carry several.
//!
//! This is NOT [`crate::diff::wrap_diff_lines`], which is diff-specific: it
//! re-emits the line-number gutter on every continuation row and indents past
//! it. Prose must not be indented that way.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Display width of `s`, via ratatui's own unicode-width measurement, so the
/// wrapper agrees with the renderer about how wide a CJK glyph or an emoji is.
/// `Span::raw` borrows, so there is no allocation.
fn display_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Display width of one character.
fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    display_width(c.encode_utf8(&mut buf))
}

/// Wrap `lines` to `width` display columns, preserving per-span styling.
///
/// The returned lines are each at most `width` columns wide, so rendering them
/// WITHOUT `Wrap` puts exactly one on each row: `result.len()` is the rendered
/// height, which is the whole point.
///
/// A `width` of 0 yields nothing, matching a `Paragraph` given no room.
pub(crate) fn wrap_styled_lines(lines: &[Line<'_>], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for line in lines {
        wrap_one(line, width, &mut out);
    }
    out
}

/// Append the wrapped rows of a single line.
fn wrap_one(line: &Line<'_>, width: usize, out: &mut Vec<Line<'static>>) {
    // Already fits: emit it verbatim, spans and all. This keeps the common case
    // byte-identical to what the un-wrapped paragraph drew, including styled
    // trailing padding (the help banners are a background-colored run of
    // spaces).
    if line.width() <= width {
        out.push(owned_line(line));
        return;
    }

    let mut wrapper = LineWrapper::new(width);
    let mut word: Vec<(char, Style)> = Vec::new();
    let mut word_width = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    wrapper.push_word(std::mem::take(&mut word), word_width);
                    word_width = 0;
                }
                wrapper.push_whitespace(ch, span.style);
            } else {
                word_width += char_width(ch);
                word.push((ch, span.style));
            }
        }
    }
    if !word.is_empty() {
        wrapper.push_word(word, word_width);
    }
    out.extend(wrapper.finish().into_iter().map(|cells| {
        // Line-level style and alignment belong to every row the line became.
        let mut row = cells_to_line(cells).style(line.style);
        row.alignment = line.alignment;
        row
    }));
}

/// Greedy word-wrap state machine for one input line.
struct LineWrapper {
    width: usize,
    rows: Vec<Vec<(char, Style)>>,
    /// The row being filled.
    row: Vec<(char, Style)>,
    row_width: usize,
    /// Whitespace seen since the last word, held back until we know whether it
    /// lands inside a row or at a break.
    space: Vec<(char, Style)>,
    space_width: usize,
}

impl LineWrapper {
    fn new(width: usize) -> Self {
        Self {
            width,
            rows: Vec::new(),
            row: Vec::new(),
            row_width: 0,
            space: Vec::new(),
            space_width: 0,
        }
    }

    fn push_whitespace(&mut self, ch: char, style: Style) {
        self.space_width += char_width(ch);
        self.space.push((ch, style));
    }

    /// Move the held-back whitespace into the current row.
    fn take_space(&mut self) {
        self.row.append(&mut self.space);
        self.row_width += self.space_width;
        self.space_width = 0;
    }

    /// End the current row. The whitespace that caused the break is discarded,
    /// exactly as ratatui discards it, and continuation rows therefore start at
    /// column 0 rather than re-indented.
    fn break_row(&mut self) {
        self.rows.push(std::mem::take(&mut self.row));
        self.row_width = 0;
        self.space.clear();
        self.space_width = 0;
    }

    fn push_word(&mut self, word: Vec<(char, Style)>, word_width: usize) {
        // It fits after the pending whitespace: keep filling this row.
        if self.row_width + self.space_width + word_width <= self.width {
            self.take_space();
            self.row_width += word_width;
            self.row.extend(word);
            return;
        }
        // The row has content and the word does not fit in what is left, so the
        // row ends here. This holds even for a word too long for ANY row:
        // ratatui breaks BEFORE such a word and hard-breaks it on the fresh row,
        // rather than filling the rest of this one with its first few
        // characters.
        if self.row_width > 0 {
            self.break_row();
        }
        // On a fresh row, a word that fits simply starts there.
        if self.row_width + self.space_width + word_width <= self.width {
            self.take_space();
            self.row_width += word_width;
            self.row.extend(word);
            return;
        }
        // What is left is a word wider than the room it can ever get: hard-break
        // it at the row edge, keeping the line's leading indent if that fits.
        // ratatui does the same — there is no break point to prefer.
        if self.row_width + self.space_width <= self.width {
            self.take_space();
        } else {
            self.space.clear();
            self.space_width = 0;
        }
        for (ch, style) in word {
            let cw = char_width(ch);
            // `row_width > 0` keeps a single character wider than the whole row
            // from looping forever; it overflows one row instead, as ratatui's
            // renderer would clip it.
            if self.row_width + cw > self.width && self.row_width > 0 {
                self.break_row();
            }
            self.row_width += cw;
            self.row.push((ch, style));
        }
    }

    fn finish(mut self) -> Vec<Vec<(char, Style)>> {
        // Trailing whitespace only survives if it fits: past the row edge it is
        // invisible, and keeping it would make the row wider than `width` and
        // break the one-line-per-row guarantee the caller clamps with.
        if self.space_width > 0 && self.row_width + self.space_width <= self.width {
            self.take_space();
        }
        if !self.row.is_empty() || self.rows.is_empty() {
            self.rows.push(std::mem::take(&mut self.row));
        }
        self.rows
    }
}

/// Clone a line into an owned one, span for span.
fn owned_line(line: &Line<'_>) -> Line<'static> {
    let mut out = Line::from(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), span.style))
            .collect::<Vec<_>>(),
    )
    .style(line.style);
    // `None` means "inherit the paragraph's alignment", which is not the same as
    // `Some(Left)`, so copy the option rather than defaulting it.
    out.alignment = line.alignment;
    out
}

/// Rebuild a row of styled characters into spans, merging runs that share a
/// style so the output is no more fragmented than it has to be.
fn cells_to_line(cells: Vec<(char, Style)>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut current_style: Option<Style> = None;
    for (ch, style) in cells {
        match current_style {
            Some(prev) if prev == style => current.push(ch),
            Some(prev) => {
                spans.push(Span::styled(std::mem::take(&mut current), prev));
                current.push(ch);
                current_style = Some(style);
            }
            None => {
                current.push(ch);
                current_style = Some(style);
            }
        }
    }
    if let Some(style) = current_style {
        spans.push(Span::styled(current, style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use ratatui::widgets::{Paragraph, Widget, Wrap};

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn a_line_that_fits_is_untouched() {
        let lines = vec![Line::from("short enough")];
        let wrapped = wrap_styled_lines(&lines, 20);
        assert_eq!(texts(&wrapped), vec!["short enough".to_string()]);
    }

    #[test]
    fn empty_and_blank_lines_survive_as_rows() {
        // A blank line is a row: dropping it would shift every offset below it.
        let lines = vec![Line::from(""), Line::from("x"), Line::from("")];
        assert_eq!(wrap_styled_lines(&lines, 10).len(), 3);
    }

    #[test]
    fn wrapping_breaks_at_whitespace_and_drops_the_break_space() {
        let lines = vec![Line::from("aaa bbb ccc")];
        let wrapped = wrap_styled_lines(&lines, 7);
        assert_eq!(texts(&wrapped), vec!["aaa bbb".to_string(), "ccc".into()]);
    }

    #[test]
    fn leading_indent_is_kept_on_the_first_row_only() {
        // This is what `trim: false` does, and what the help overlay's two-space
        // keybinding indent relies on. Continuation rows are NOT re-indented.
        let lines = vec![Line::from("  alpha beta gamma")];
        let wrapped = wrap_styled_lines(&lines, 12);
        assert_eq!(
            texts(&wrapped),
            vec!["  alpha beta".to_string(), "gamma".into()]
        );
    }

    #[test]
    fn an_overlong_word_is_hard_broken_at_the_row_edge() {
        let lines = vec![Line::from("abcdefghij")];
        let wrapped = wrap_styled_lines(&lines, 4);
        assert_eq!(
            texts(&wrapped),
            vec!["abcd".to_string(), "efgh".into(), "ij".into()]
        );
    }

    #[test]
    fn every_wrapped_row_fits_the_width() {
        // The guarantee the caller's clamp depends on: one row per line, so no
        // line may exceed the width.
        let lines = vec![
            Line::from("  <Ctrl-g>      Exit typed-path mode in the project browser"),
            Line::from("wordy ".repeat(30)),
            Line::from("supercalifragilisticexpialidocious-".repeat(4)),
        ];
        for width in [1usize, 3, 7, 12, 40, 55] {
            for line in wrap_styled_lines(&lines, width) {
                assert!(
                    line.width() <= width,
                    "row {:?} is {} wide, over the {width}-column limit",
                    line.to_string(),
                    line.width()
                );
            }
        }
    }

    #[test]
    fn styles_survive_the_wrap() {
        let key = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let desc = Style::default().fg(Color::Gray);
        let lines = vec![Line::from(vec![
            Span::styled("<Ctrl-g>", key),
            Span::styled(" exit the typed path mode", desc),
        ])];
        let wrapped = wrap_styled_lines(&lines, 14);
        assert_eq!(
            texts(&wrapped),
            vec![
                "<Ctrl-g> exit".to_string(),
                "the typed path".into(),
                "mode".into()
            ]
        );
        // The badge keeps its own style, and the description keeps its own on
        // both rows.
        assert_eq!(wrapped[0].spans[0].style, key);
        assert_eq!(wrapped[0].spans[0].content, "<Ctrl-g>");
        assert_eq!(wrapped[0].spans[1].style, desc);
        assert_eq!(wrapped[1].spans[0].style, desc);
    }

    #[test]
    fn wide_characters_are_measured_in_columns_not_chars() {
        // Four CJK glyphs are eight columns wide, so a six-column row holds
        // three of them.
        let lines = vec![Line::from("日本語です")];
        let wrapped = wrap_styled_lines(&lines, 6);
        assert_eq!(texts(&wrapped), vec!["日本語".to_string(), "です".into()]);
        for line in &wrapped {
            assert!(line.width() <= 6);
        }
    }

    #[test]
    fn zero_width_yields_nothing() {
        // A paragraph with no room renders no rows, so the count must be 0 too.
        assert!(wrap_styled_lines(&[Line::from("anything")], 0).is_empty());
    }

    /// The appearance guarantee, MEASURED rather than argued: pre-wrapping and
    /// rendering without `Wrap` must paint the same cells that `Wrap { trim:
    /// false }` painted, for the shapes the help overlay actually contains.
    #[test]
    fn pre_wrapping_paints_the_same_cells_as_ratatuis_own_wrap() {
        let key = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let banner = Style::default().fg(Color::Black).bg(Color::Cyan);
        let body = Style::default().fg(Color::Gray);
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "dux has two front ends over one workspace: a terminal",
                body,
            )),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("<Enter/Right/l>", key),
                Span::raw(" "),
                Span::styled("Open or navigate into the selected entry", body),
            ]),
            Line::from(vec![
                Span::styled("Your config file is self-documented: ", body),
                Span::styled("/home/somebody/.config/dux/config.toml", key),
            ]),
            Line::from(Span::styled(
                "  ○  Disabled — enable via command palette (toggle-github-integration)",
                body,
            )),
        ];

        for width in [20u16, 31, 40, 55, 72] {
            let height = 60u16;
            // The help banners are built to the content width (a title plus
            // background-colored padding), so they never wrap. Rebuild it per
            // width, exactly as `push_banner` does.
            lines.push(Line::from(Span::styled(
                format!(
                    " About dux{}",
                    " ".repeat(width as usize - "About dux".len() - 3)
                ),
                banner,
            )));
            let mut ratatui_side = Terminal::new(TestBackend::new(width, height)).expect("term");
            ratatui_side
                .draw(|frame| {
                    Paragraph::new(lines.clone())
                        .wrap(Wrap { trim: false })
                        .render(frame.area(), frame.buffer_mut());
                })
                .expect("draw");

            let wrapped = wrap_styled_lines(&lines, width as usize);
            let mut ours = Terminal::new(TestBackend::new(width, height)).expect("term");
            ours.draw(|frame| {
                Paragraph::new(wrapped.clone()).render(frame.area(), frame.buffer_mut());
            })
            .expect("draw");

            let want = ratatui_side.backend().buffer().clone();
            let got = ours.backend().buffer().clone();
            for y in 0..height {
                let want_row: String = (0..width).map(|x| want[(x, y)].symbol()).collect();
                let got_row: String = (0..width).map(|x| got[(x, y)].symbol()).collect();
                assert_eq!(
                    want_row.trim_end(),
                    got_row.trim_end(),
                    "row {y} at width {width} differs\n  ratatui: {want_row:?}\n  ours:    {got_row:?}"
                );
            }
        }
    }

    /// The one place this wrapper deliberately differs from `Wrap { trim: false
    /// }`, recorded so it is a decision rather than a surprise.
    ///
    /// ratatui breaks on any symbol once the row is full, whitespace included,
    /// so a line whose ONLY overflow is trailing whitespace becomes two rows for
    /// it: the text, then a row holding the leftover spaces. We emit one row and
    /// drop whitespace that cannot fit. Nothing visible changes (trailing spaces
    /// past the row edge paint nothing) and the row count stays honest, which is
    /// what the clamp reads.
    #[test]
    fn trailing_whitespace_overflow_does_not_earn_a_second_row() {
        let line = Line::from("text                                  ");
        assert_eq!(wrap_styled_lines(std::slice::from_ref(&line), 10).len(), 1);

        let mut ratatui_side = Terminal::new(TestBackend::new(10, 4)).expect("term");
        ratatui_side
            .draw(|frame| {
                Paragraph::new(vec![line])
                    .wrap(Wrap { trim: false })
                    .render(frame.area(), frame.buffer_mut());
            })
            .expect("draw");
        // Proof that ratatui really does spend an extra row on it: the second row
        // exists and is blank.
        let buf = ratatui_side.backend().buffer().clone();
        let second: String = (0..10).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(second.trim_end(), "", "ratatui's extra row is blank");
    }
}
