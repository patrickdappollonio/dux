//! Whether a scrollable surface has more to show, and in which direction.
//!
//! Pure and shared: this is the semantics behind every "there is more below"
//! affordance. Surfaces own the presentation (a glyph, a scrollbar, a badge) and
//! the geometry; the decision of WHICH state applies lives here so the surfaces
//! cannot drift, and so the degenerate cases (nothing to show, no room to show
//! it in, an offset left over from a taller viewport) are settled once.
//!
//! # Units are the caller's choice, and mixing them lies to the user
//!
//! [`scroll_hint`] is unit-agnostic: `offset`, `viewport`, and `total` only have
//! to agree with each other. Two units are in play in a terminal UI and they are
//! NOT interchangeable:
//!
//! - **Wrapped visual lines** — paragraph surfaces (the help overlay, the diff
//!   view, the first-load modal). `total` must be the count AFTER wrapping, not
//!   the number of logical lines: a surface that wraps its text has more rows
//!   than lines, and passing the pre-wrap count reports "at the bottom" while
//!   rows are still hidden.
//! - **Whole items** — list surfaces (the command palette). A `ListState`
//!   offset counts items and never clips a partially visible top item, so
//!   `offset`/`viewport`/`total` are all item counts.
//!
//! Passing a viewport measured in rows next to a total measured in items (or a
//! pre-wrap line count) makes the marker lie, which is worse than having no
//! marker at all.

/// The furthest offset that still fills the viewport: everything below is
/// already on screen at this offset.
///
/// The `total.saturating_sub(viewport)` every surface used to hand-roll, in one
/// place. Saturating, so a viewport taller than the content clamps to 0 instead
/// of underflowing.
#[must_use]
pub fn max_scroll_offset(viewport: usize, total: usize) -> usize {
    total.saturating_sub(viewport)
}

/// Which scroll affordance a surface should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollHint {
    /// Everything fits (or there is no viewport to fit it in). Nothing to
    /// indicate: a surface that cannot scroll must not suggest that it can.
    NotScrollable,
    /// At the top, with content below.
    MoreBelow,
    /// Somewhere in the middle: content above AND below.
    BothWays,
    /// Scrolled to the end, with content above only.
    AtBottom,
}

impl ScrollHint {
    /// Whether the surface can be scrolled at all.
    #[must_use]
    pub fn is_scrollable(self) -> bool {
        !matches!(self, Self::NotScrollable)
    }
}

/// Classify a scrollable surface.
///
/// `offset` is the first visible row/item, `viewport` how many fit on screen,
/// and `total` how many exist — all in the SAME unit (see the module docs).
///
/// An `offset` past the last reachable position is treated as
/// [`ScrollHint::AtBottom`] rather than as an error: renderers clamp the offset
/// they actually draw with, and a stale offset (the terminal just grew, so the
/// viewport now reaches further) must agree with what was drawn.
#[must_use]
pub fn scroll_hint(offset: usize, viewport: usize, total: usize) -> ScrollHint {
    // No room to render anything, so there is nowhere to put a hint and nothing
    // it could describe.
    if viewport == 0 || total <= viewport {
        return ScrollHint::NotScrollable;
    }
    let max = max_scroll_offset(viewport, total);
    if offset == 0 {
        ScrollHint::MoreBelow
    } else if offset >= max {
        ScrollHint::AtBottom
    } else {
        ScrollHint::BothWays
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_offset_is_the_overflow_and_saturates() {
        assert_eq!(max_scroll_offset(10, 30), 20);
        assert_eq!(max_scroll_offset(10, 10), 0);
        assert_eq!(max_scroll_offset(10, 3), 0);
        assert_eq!(max_scroll_offset(0, 0), 0);
        assert_eq!(max_scroll_offset(usize::MAX, 1), 0);
    }

    #[test]
    fn nothing_to_scroll_when_the_content_fits() {
        // Exactly full, and short of full: both are unscrollable, and the
        // offset is irrelevant because a clamped offset can only be 0.
        for total in 0..=5usize {
            assert_eq!(
                scroll_hint(0, 5, total),
                ScrollHint::NotScrollable,
                "{total} lines in a 5-row viewport must not offer scrolling"
            );
        }
    }

    #[test]
    fn no_viewport_means_no_hint() {
        // A zero-height pane cannot show a marker, and "more below" would be
        // meaningless: nothing is visible to be below.
        assert_eq!(scroll_hint(0, 0, 0), ScrollHint::NotScrollable);
        assert_eq!(scroll_hint(0, 0, 100), ScrollHint::NotScrollable);
        assert_eq!(scroll_hint(50, 0, 100), ScrollHint::NotScrollable);
    }

    #[test]
    fn empty_content_is_never_scrollable() {
        assert_eq!(scroll_hint(0, 1, 0), ScrollHint::NotScrollable);
        assert_eq!(scroll_hint(7, 20, 0), ScrollHint::NotScrollable);
    }

    #[test]
    fn one_row_of_overflow_walks_top_to_bottom() {
        // 6 lines in a 5-row viewport: max offset 1, so there is no middle.
        assert_eq!(scroll_hint(0, 5, 6), ScrollHint::MoreBelow);
        assert_eq!(scroll_hint(1, 5, 6), ScrollHint::AtBottom);
    }

    #[test]
    fn every_offset_of_a_scrollable_surface_is_classified() {
        // 10 lines, 4-row viewport, max offset 6.
        let expected = [
            ScrollHint::MoreBelow,
            ScrollHint::BothWays,
            ScrollHint::BothWays,
            ScrollHint::BothWays,
            ScrollHint::BothWays,
            ScrollHint::BothWays,
            ScrollHint::AtBottom,
        ];
        for (offset, want) in expected.iter().enumerate() {
            assert_eq!(
                scroll_hint(offset, 4, 10),
                *want,
                "offset {offset} of 10 lines in a 4-row viewport"
            );
        }
    }

    #[test]
    fn an_offset_past_the_end_reads_as_the_bottom() {
        // A stale offset (the pane just grew) must agree with what the renderer
        // clamped to and drew, which is the bottom.
        assert_eq!(scroll_hint(7, 4, 10), ScrollHint::AtBottom);
        assert_eq!(scroll_hint(1_000, 4, 10), ScrollHint::AtBottom);
        assert_eq!(scroll_hint(usize::MAX, 4, 10), ScrollHint::AtBottom);
    }

    #[test]
    fn is_scrollable_agrees_with_the_classification() {
        assert!(!ScrollHint::NotScrollable.is_scrollable());
        for hint in [
            ScrollHint::MoreBelow,
            ScrollHint::BothWays,
            ScrollHint::AtBottom,
        ] {
            assert!(hint.is_scrollable(), "{hint:?} means content is off-screen");
        }
    }

    #[test]
    fn single_row_viewport_is_still_classified() {
        assert_eq!(scroll_hint(0, 1, 3), ScrollHint::MoreBelow);
        assert_eq!(scroll_hint(1, 1, 3), ScrollHint::BothWays);
        assert_eq!(scroll_hint(2, 1, 3), ScrollHint::AtBottom);
    }
}
