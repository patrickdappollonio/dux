//! The scroll style the welcome and What's new screens wear, shared by every
//! surface that should look the same when it scrolls.
//!
//! Two pieces:
//!
//! - the scroll INDICATOR: the shared one-cell direction marker from
//!   [`super::scroll_marker`] in the modal's right border column, painted in
//!   [`scroll_indicator_color`], the accent those two screens give it;
//! - the scroll VIEW: a content pane showing a window of pre-built lines,
//!   clamped so the offset can never run past the last page, with that
//!   indicator beside it.
//!
//! A list surface (a picker's rows, driven by a `ListState`) has no lines to
//! slice, so it takes the indicator alone, in ITEM units; see
//! [`super::picker_list`].

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use super::scroll_marker::render_scroll_marker;
use crate::theme::Theme;

/// The indicator's color: the accent the welcome and What's new screens paint
/// their title and marker in.
pub(crate) fn scroll_indicator_color(theme: &Theme) -> Color {
    theme.title_focused
}

/// Draw the scroll indicator for `content` inside `outer`'s border ring, if
/// there is more to see.
///
/// `offset`, `viewport` and `total` share one unit, as
/// [`render_scroll_marker`] documents: wrapped rows for a paragraph, whole
/// items for a list.
pub(crate) fn render_scroll_indicator(
    frame: &mut Frame,
    outer: Rect,
    content: Rect,
    offset: usize,
    viewport: usize,
    total: usize,
    theme: &Theme,
) {
    render_scroll_marker(
        frame,
        outer,
        content,
        offset,
        viewport,
        total,
        scroll_indicator_color(theme),
    );
}

/// What a scroll view drew, in rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScrollViewRender {
    /// The offset actually used, after clamping to the last page.
    pub(crate) offset: usize,
    /// Rows the content pane shows.
    pub(crate) viewport: usize,
    /// Rows the content has in all.
    pub(crate) total: usize,
}

/// Show `lines` from `scroll` in `content`, clamped so the last page is the
/// furthest it can go, with the indicator in `outer`'s border column.
///
/// `lines` must already be wrapped to `content`'s width: the clamp counts them,
/// and a wrapping paragraph would draw more rows than it was given.
pub(crate) fn render_scroll_view(
    frame: &mut Frame,
    outer: Rect,
    content: Rect,
    lines: Vec<Line<'static>>,
    scroll: usize,
    theme: &Theme,
) -> ScrollViewRender {
    let total = lines.len();
    let viewport = usize::from(content.height);
    let offset = scroll.min(total.saturating_sub(viewport));
    let slice: Vec<Line> = lines.into_iter().skip(offset).take(viewport).collect();
    Paragraph::new(slice).render(content, frame.buffer_mut());
    render_scroll_indicator(frame, outer, content, offset, viewport, total, theme);
    ScrollViewRender {
        offset,
        viewport,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::components::MARKER_GLYPHS;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn lines(count: usize) -> Vec<Line<'static>> {
        (0..count)
            .map(|n| Line::from(format!("line {n}")))
            .collect()
    }

    #[test]
    fn the_view_clamps_to_the_last_page_and_marks_the_way_back() {
        let theme = Theme::default_dark();
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).expect("terminal");
        let outer = Rect::new(0, 0, 20, 8);
        let content = Rect::new(1, 1, 18, 6);
        let mut drawn = None;
        terminal
            .draw(|frame| {
                drawn = Some(render_scroll_view(
                    frame,
                    outer,
                    content,
                    lines(10),
                    99,
                    &theme,
                ));
            })
            .expect("draw");
        let drawn = drawn.expect("rendered");
        assert_eq!(
            drawn,
            ScrollViewRender {
                offset: 4,
                viewport: 6,
                total: 10
            }
        );
        let buf = terminal.backend().buffer();
        let row: String = (1..8).map(|x| buf[(x, 6)].symbol().to_string()).collect();
        assert!(
            row.starts_with("line 9"),
            "last line on the last row: {row:?}"
        );
        let marker = &buf[(19, 6)];
        assert_eq!(marker.symbol(), MARKER_GLYPHS[1], "only up is left");
        assert_eq!(marker.fg, scroll_indicator_color(&theme));
    }

    #[test]
    fn content_that_fits_draws_no_indicator() {
        let theme = Theme::default_dark();
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).expect("terminal");
        terminal
            .draw(|frame| {
                render_scroll_view(
                    frame,
                    Rect::new(0, 0, 20, 8),
                    Rect::new(1, 1, 18, 6),
                    lines(3),
                    0,
                    &theme,
                );
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        assert!(
            (0..8).all(|y| !MARKER_GLYPHS.contains(&buf[(19, y)].symbol())),
            "nothing to scroll, so nothing may suggest it"
        );
    }
}
