//! Pre-wrapping styled lines so a scroll clamp can be exact.
//!
//! A `Paragraph` rendered with `Wrap { trim: false }` produces more rows than
//! it was given lines and does not say how many, so a surface clamping its
//! scroll offset with `lines.len()` stops short of its own bottom whenever
//! anything wraps. Callers wrap here, render the result without `Wrap`, and
//! clamp with the returned length, which is then the row count by construction.
//!
//! [`wrap_styled_lines`] reproduces `Wrap { trim: false }`:
//!
//! - Greedy word wrapping at whitespace, in display columns.
//! - The leading whitespace of a line is kept; continuation rows are not
//!   re-indented, because ratatui does not re-indent them either.
//! - Whitespace at a break point is dropped, as ratatui's `WordWrapper` drops
//!   the whitespace that fits in the row it just ended.
//! - A word too long to fit a row is hard-broken at the row edge.
//! - Every span keeps its own style; a wrapped row can carry several.
//! - A name chip (a span carrying the chip marker, see
//!   [`crate::theme::is_name_chip`]) is one unbreakable unit, spaces inside it
//!   and its two padding cells included, so a multi-word name moves to the next
//!   row whole rather than splitting or losing a pad. This is the one rule
//!   ratatui's own wrapper has no way to express. A chip is known by its marker
//!   and never by its colors, which are two ordinary theme colors a caret or a
//!   highlight may share.
//!
//! Not [`crate::diff::wrap_diff_lines`], which is diff-specific: it re-emits
//! the line-number gutter on every continuation row and indents past it.

use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::is_name_chip;

/// Display width of `s`: the sum of its extended grapheme clusters, each
/// measured by [`cluster_width`], which is how ratatui's buffer draws text. So
/// the wrapper and every cut agree with the renderer about how wide a CJK glyph
/// or an emoji sequence is.
///
/// Shared with the dialog bodies' own indenter, which must decide "does this
/// line fit" by the measure the widget will render it at.
pub(crate) fn display_width(s: &str) -> usize {
    s.graphemes(true).map(cluster_width).sum()
}

/// Display width of one extended grapheme cluster, the unit ratatui draws in.
///
/// Measured whole rather than character by character: an emoji presentation
/// selector turns a one-column `⚠` into a two-column glyph, and a ZWJ family is
/// three emoji drawn as one, so a per-character sum is wrong in both
/// directions. A lone control character is measured as before, because the
/// buffer's own measure refuses one.
pub(crate) fn cluster_width(cluster: &str) -> usize {
    if cluster.len() == 1 && cluster.as_bytes()[0].is_ascii_control() {
        return Span::raw(cluster).width();
    }
    usize::from(cluster.cell_width())
}

/// Display width of one character, in terminal cells.
///
/// Shared with the pane card's truncation so both cut by the measure the
/// wrapper wraps by: two functions that disagree about how wide a CJK glyph is
/// produce a line that overflows the box it was measured for.
pub(crate) fn char_display_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    display_width(c.encode_utf8(&mut buf))
}

/// Wrap `lines` to `width` display columns, preserving per-span styling.
///
/// Each returned line is at most `width` columns wide, so rendering them
/// without `Wrap` puts exactly one on each row and `result.len()` is the
/// rendered height. A `width` of 0 yields nothing, matching a `Paragraph`
/// given no room.
///
/// Every span carrying the name chip's marker is kept whole.
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

/// Whether a row may break at `ch`. Whitespace does, except the no-break
/// space, which glues its neighbours exactly as ratatui's own wrapper treats
/// it.
fn breaks_a_row(ch: char) -> bool {
    ch.is_whitespace() && ch != NO_BREAK_SPACE
}

/// U+00A0, the space a row never breaks at.
pub(crate) const NO_BREAK_SPACE: char = '\u{a0}';

/// Append the wrapped rows of a single line.
fn wrap_one(line: &Line<'_>, width: usize, out: &mut Vec<Line<'static>>) {
    // Already fits: emit it verbatim, spans and all, so the common case stays
    // byte-identical to the un-wrapped paragraph, styled trailing padding included.
    let line_width: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
    if line_width <= width {
        out.push(owned_line(line));
        return;
    }

    let mut wrapper = LineWrapper::new(width);
    let mut word: Vec<Cell> = Vec::new();
    let mut word_width = 0usize;
    for span in &line.spans {
        // A chip joins the word it touches, pads and inner spaces alike, so
        // the only break points around it are the whitespace outside it.
        let whole = is_name_chip(span.style);
        for cluster in span.content.graphemes(true) {
            let cell = Cell {
                text: cluster.to_string(),
                width: cluster_width(cluster),
                style: span.style,
            };
            if !whole && cluster.chars().count() == 1 && cluster.chars().all(breaks_a_row) {
                if !word.is_empty() {
                    wrapper.push_word(std::mem::take(&mut word), word_width);
                    word_width = 0;
                }
                wrapper.push_whitespace(cell);
            } else {
                word_width += cell.width;
                word.push(cell);
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

/// One drawn glyph: an extended grapheme cluster, its width and its style.
struct Cell {
    text: String,
    width: usize,
    style: Style,
}

/// Greedy word-wrap state machine for one input line.
struct LineWrapper {
    width: usize,
    rows: Vec<Vec<Cell>>,
    /// The row being filled.
    row: Vec<Cell>,
    row_width: usize,
    /// Whitespace seen since the last word, held back until we know whether it
    /// lands inside a row or at a break.
    space: Vec<Cell>,
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

    fn push_whitespace(&mut self, cell: Cell) {
        self.space_width += cell.width;
        self.space.push(cell);
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

    fn push_word(&mut self, word: Vec<Cell>, word_width: usize) {
        // It fits after the pending whitespace: keep filling this row.
        if self.row_width + self.space_width + word_width <= self.width {
            self.take_space();
            self.row_width += word_width;
            self.row.extend(word);
            return;
        }
        // The row ends here even for a word too long for any row: ratatui breaks
        // before such a word and hard-breaks it on the fresh row.
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
        // ratatui does the same, because there is no break point to prefer.
        if self.row_width + self.space_width <= self.width {
            self.take_space();
        } else {
            self.space.clear();
            self.space_width = 0;
        }
        for cell in word {
            // `row_width > 0` keeps a single glyph wider than the whole row
            // from looping forever; it overflows one row instead, as ratatui's
            // renderer would clip it.
            if self.row_width + cell.width > self.width && self.row_width > 0 {
                self.break_row();
            }
            self.row_width += cell.width;
            self.row.push(cell);
        }
    }

    fn finish(mut self) -> Vec<Vec<Cell>> {
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

/// Rebuild a row of styled glyphs into spans, merging runs that share a
/// style so the output is no more fragmented than it has to be.
fn cells_to_line(cells: Vec<Cell>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut current_style: Option<Style> = None;
    for Cell { text, style, .. } in cells {
        match current_style {
            Some(prev) if prev == style => current.push_str(&text),
            Some(prev) => {
                spans.push(Span::styled(std::mem::take(&mut current), prev));
                current.push_str(&text);
                current_style = Some(style);
            }
            None => {
                current.push_str(&text);
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

    /// The columns a row takes when ratatui draws it.
    fn drawn_width(line: &Line<'_>) -> usize {
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 200, 1));
        let text = line.to_string();
        usize::from(buf.set_stringn(0, 0, &text, 200, Style::default()).0)
    }

    /// A presentation selector makes `⚠` two columns as drawn, so a row holding
    /// one is one column wider than its characters say; the wrapper measures
    /// what is drawn, so nothing is pushed past the edge and clipped.
    #[test]
    fn an_emoji_sequence_is_measured_as_drawn_and_kept_whole() {
        let line = Line::from(" \u{25cf} \u{26a0}\u{fe0f} aaaaaaaaaaaaaaa");
        let rows = wrap_styled_lines(&[line], 20);
        assert_eq!(
            texts(&rows),
            vec![" \u{25cf} \u{26a0}\u{fe0f}", "aaaaaaaaaaaaaaa"]
        );
        for row in &rows {
            assert!(drawn_width(row) <= 20, "{row:?}");
        }
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let rows = wrap_styled_lines(&[Line::from(family.repeat(3))], 5);
        assert_eq!(texts(&rows), vec![family.repeat(2), family.to_string()]);
        assert_eq!(display_width(family), 2);
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
                "  ○  Disabled: enable via command palette (toggle-github-integration)",
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

    /// A no-break space glues what is on either side of it, as it does in
    /// ratatui's own wrapper.
    #[test]
    fn a_no_break_space_never_breaks_a_row() {
        let chip = Style::default().bg(Color::Blue);
        let line = Line::from(vec![
            Span::raw("ask the agent "),
            Span::styled("\u{a0}feat/login\u{a0}", chip),
            Span::raw(" to stop"),
        ]);
        let wrapped = wrap_styled_lines(std::slice::from_ref(&line), 20);
        assert_eq!(
            texts(&wrapped),
            vec!["ask the agent", "\u{a0}feat/login\u{a0} to stop"]
        );

        let mut ratatui_side = Terminal::new(TestBackend::new(20, 4)).expect("term");
        ratatui_side
            .draw(|frame| {
                Paragraph::new(vec![line])
                    .wrap(Wrap { trim: false })
                    .render(frame.area(), frame.buffer_mut());
            })
            .expect("draw");
        let buf = ratatui_side.backend().buffer().clone();
        for (y, want) in texts(&wrapped).iter().enumerate() {
            let got: String = (0..20).map(|x| buf[(x, y as u16)].symbol()).collect();
            assert_eq!(got.trim_end(), want, "row {y} differs from ratatui's wrap");
        }
    }

    /// A chip is one unit: a name with spaces inside it, padded by ordinary
    /// spaces, moves to the next row whole and keeps both pads, instead of
    /// breaking at a pad or between its words.
    #[test]
    fn a_chip_is_never_broken_inside_or_at_its_padding() {
        let theme = crate::theme::Theme::default_dark();
        let line = Line::from(vec![
            Span::raw("ask the agent "),
            Span::styled(" My Cool Project ", theme.name_style()),
            Span::raw(" to stop"),
        ]);
        let wrapped = wrap_styled_lines(std::slice::from_ref(&line), 20);
        assert_eq!(
            texts(&wrapped),
            vec!["ask the agent", " My Cool Project  to", "stop"]
        );
        assert_eq!(wrapped[1].spans[0].content, " My Cool Project ");
        assert_eq!(wrapped[1].spans[0].style, theme.name_style());
    }

    /// The chip colors are two ordinary theme colors (the body's, swapped), and
    /// a text-input caret is drawn in exactly that pair in themes whose caret
    /// tokens resolve to them. Only the chip's marker makes a span a chip, so a
    /// span that merely shares its colors wraps like any other text.
    #[test]
    fn a_span_in_the_chip_colors_without_the_marker_wraps_like_any_text() {
        let theme = crate::theme::Theme::default_dark();
        let look_alike = Style::default().fg(theme.overlay_bg).bg(theme.text_fg);
        let line = Line::from(vec![
            Span::raw("ask the agent "),
            Span::styled(" My Cool Project ", look_alike),
            Span::raw(" to stop"),
        ]);
        let wrapped = wrap_styled_lines(std::slice::from_ref(&line), 20);
        assert_eq!(
            texts(&wrapped),
            vec!["ask the agent  My", "Cool Project  to", "stop"]
        );
    }

    /// A chip a caller restyled (dimmed, recolored) is still a chip, and still
    /// one unit.
    #[test]
    fn a_restyled_chip_is_still_kept_whole() {
        let theme = crate::theme::Theme::default_dark();
        let dimmed = theme
            .name_style()
            .patch(Style::default().fg(ratatui::style::Color::DarkGray));
        let line = Line::from(vec![
            Span::raw("ask the agent "),
            Span::styled(" My Cool Project ", dimmed),
            Span::raw(" to stop"),
        ]);
        let wrapped = wrap_styled_lines(std::slice::from_ref(&line), 20);
        assert_eq!(
            texts(&wrapped),
            vec!["ask the agent", " My Cool Project  to", "stop"]
        );
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
