//! The one-cell scroll-direction marker, shared by every scrollable surface.
//!
//! The semantics — whether a surface can scroll, and which way there is more to
//! see — live in [`dux_core::scroll_hint`]. This module owns only the
//! presentation: the glyph table, the theme color, and where the cell goes.
//!
//! The marker goes in the surface's right BORDER column, never in the content
//! pane. That is deliberate, and the first-load modal learned it the hard way: a
//! word too long to break makes a paragraph fill its pane's full width, so a
//! marker drawn in the pane's own last column silently eats a character of real
//! content, on the very row the reader is heading toward. A border column is
//! chrome by construction and nothing can collide with it.
//!
//! # The caller chooses the unit, and must choose the right one
//!
//! `offset`/`viewport`/`total` are passed straight to
//! [`dux_core::scroll_hint::scroll_hint`], so the caller's unit is the one that
//! matters: **wrapped visual rows** for a paragraph surface (pass the count
//! AFTER wrapping, not the number of logical lines), **whole items** for a list
//! surface driven by a `ListState`. A marker fed mismatched units lies about
//! whether there is more to read, which is worse than no marker at all.

use dux_core::scroll_hint::{ScrollHint, scroll_hint};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Which way there is more to see. Up and down arrows rather than a scrollbar:
/// one cell of chrome, legible in any terminal, and it never has to be sized.
pub(crate) const MARKER_GLYPHS: [&str; 3] = ["↓", "↑", "↕"];

/// The glyph for a classification, or `None` when the surface cannot scroll (a
/// surface that cannot scroll must not suggest that it can).
pub(crate) fn scroll_marker_glyph(hint: ScrollHint) -> Option<&'static str> {
    match hint {
        ScrollHint::NotScrollable => None,
        ScrollHint::MoreBelow => Some(MARKER_GLYPHS[0]),
        ScrollHint::AtBottom => Some(MARKER_GLYPHS[1]),
        ScrollHint::BothWays => Some(MARKER_GLYPHS[2]),
    }
}

/// Where the marker goes: the surface's right BORDER column, on the content
/// pane's last row.
///
/// `content` must be inside `area`'s border ring (derive it from
/// `Block::inner`), which is what puts the border column to the right of it.
/// Sitting on the content pane's last row keeps the marker next to the edge the
/// reader is scrolling toward, and clear of anything laid out below the content
/// (a hint bar with its own top border, for instance).
pub(crate) fn scroll_marker_rect(area: Rect, content: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(1);
    let y = content.y + content.height.saturating_sub(1);
    Rect::new(x, y, 1, 1)
}

/// Draw the marker for `content` inside `area`, if there is anything to
/// indicate.
///
/// `offset`, `viewport`, and `total` must share one unit — see the module docs.
/// Nothing is drawn when the content fits, when the pane has no room, or when
/// the cell would fall outside the frame.
pub(crate) fn render_scroll_marker(
    frame: &mut Frame,
    area: Rect,
    content: Rect,
    offset: usize,
    viewport: usize,
    total: usize,
    color: Color,
) {
    if content.width == 0 || content.height == 0 {
        return;
    }
    let Some(glyph) = scroll_marker_glyph(scroll_hint(offset, viewport, total)) else {
        return;
    };
    let cell = scroll_marker_rect(area, content);
    // A pane laid out at the screen edge can put the border column off-frame;
    // an empty intersection renders nothing rather than painting a wrapped cell.
    let cell = cell.intersection(frame.area());
    if cell.is_empty() {
        return;
    }
    Paragraph::new(Line::from(Span::styled(glyph, Style::default().fg(color))))
        .render(cell, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyph_says_which_way_there_is_more() {
        assert_eq!(scroll_marker_glyph(ScrollHint::NotScrollable), None);
        assert_eq!(scroll_marker_glyph(ScrollHint::MoreBelow), Some("↓"));
        assert_eq!(scroll_marker_glyph(ScrollHint::AtBottom), Some("↑"));
        assert_eq!(scroll_marker_glyph(ScrollHint::BothWays), Some("↕"));
    }

    #[test]
    fn the_marker_cell_is_the_border_column_beside_the_content() {
        // Every glyph the table can produce must be findable by a test that
        // scans the border column, so keep the table and the classifier in step.
        for hint in [
            ScrollHint::MoreBelow,
            ScrollHint::AtBottom,
            ScrollHint::BothWays,
        ] {
            let glyph = scroll_marker_glyph(hint).expect("scrollable");
            assert!(MARKER_GLYPHS.contains(&glyph));
        }

        let area = Rect::new(4, 2, 30, 12);
        // A content pane derived from a bordered block, with a hint strip below.
        let content = Rect::new(area.x + 1, area.y + 1, area.width - 2, 8);
        let cell = scroll_marker_rect(area, content);
        assert_eq!(cell.x, area.x + area.width - 1, "right border column");
        assert!(
            cell.x >= content.x + content.width,
            "the marker must never land inside the content pane"
        );
        assert!(
            cell.x < area.x + area.width,
            "and never outside the surface"
        );
        assert_eq!(cell.y, content.y + content.height - 1, "content's last row");
    }

    #[test]
    fn degenerate_geometry_does_not_wrap_around() {
        // Zero-sized panes are laid out on tiny terminals; the arithmetic must
        // saturate rather than underflow into the far corner of the screen.
        let cell = scroll_marker_rect(Rect::new(0, 0, 0, 0), Rect::new(0, 0, 0, 0));
        assert_eq!((cell.x, cell.y), (0, 0));
    }
}
