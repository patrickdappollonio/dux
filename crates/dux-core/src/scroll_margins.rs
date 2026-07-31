//! Tracks the scrolling region (the DECSTBM top and bottom margins) a child
//! program has in effect.
//!
//! A program that pins a header or a status bar narrows the region so only the
//! middle of the screen scrolls. The region is terminal state rather than screen
//! content, so a repaint that rebuilds cells does not carry it, and a web client
//! that resets before applying the replay comes back with the full screen
//! scrolling again.
//!
//! The terminal engine dux drives keeps its region in a private field with no
//! accessor, so it cannot be read back from the terminal that already has it.
//! Instead this module runs a second parser over the same bytes and keeps its own
//! copy. The parser drives `Handler`, whose seventy-odd methods all have empty
//! default bodies, so an observer implements only the handful of callbacks that
//! can move the region and ignores everything else. It touches no grid and
//! allocates nothing per byte.
//!
//! Two copies of one value can drift, so the rule for this module is that it
//! mirrors the engine rather than the specification. The engine writes its region
//! at five sites, found by reading every assignment to that private field and
//! every caller that reaches one, and this module moves at all five:
//!
//! - construction, which starts at the whole screen
//! - a resize, which widens back to the whole screen at the new height, but ONLY
//!   when a dimension actually changed: the engine compares both dimensions and
//!   returns before touching its region when neither moved
//! - a full reset (RIS), which widens back to the whole screen
//! - `set_scrolling_region`, including the engine's own clamping and its refusal
//!   of an inverted pair
//! - column mode (DECCOLM, `CSI ? 3 h` and `CSI ? 3 l`), which widens back to the
//!   whole screen as a side effect. The engine does that by calling its own
//!   `set_scrolling_region` directly rather than dispatching through the parser,
//!   so no `set_scrolling_region` callback carries it and the observer has to
//!   watch the private-mode callbacks to see it at all
//!
//! Only the explicit set is a program saying something; the other four are the
//! engine acting on its own. An observer that watched the set alone would report
//! a region the program no longer has after any of them, and restoring a stale
//! region is worse than restoring none. `pty::tests` drives each of those sites
//! through both this tracker and a live terminal and asserts they agree, which is
//! what keeps the mirror honest as the dependency changes.
//!
//! Column mode is not exotic. The standard terminal description for xterm carries
//! it in its initialisation and reset strings, so a routine terminal-initialising
//! command emits it inside an agent's session with no full reset around it.
//!
//! A soft reset (DECSTR, `CSI ! p`) is deliberately not in that list: the parser
//! version in use does not decode it at all, so it reaches neither the engine nor
//! this observer and moves no region on either side. Adding it here alone would
//! create exactly the divergence the mirror exists to prevent.

use alacritty_terminal::vte::ansi::{
    Handler, NamedPrivateMode, PrivateMode, Processor, StdSyncHandler,
};

/// The scrolling region in effect, in grid rows.
///
/// `start` is the first row of the region and `end` is one past its last, the
/// same half-open form the terminal engine keeps internally so the two can be
/// compared directly without a conversion that could itself be wrong. `screen_lines`
/// travels with the pair because whether a region is "the whole screen" is only
/// answerable against a height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollRegion {
    pub start: i32,
    pub end: i32,
    pub screen_lines: i32,
}

impl ScrollRegion {
    /// The whole screen, which is what a terminal has until a program narrows it.
    pub fn full(rows: u16) -> Self {
        Self::full_lines(i32::from(rows))
    }

    /// The whole screen, from a height already measured in grid lines.
    fn full_lines(screen_lines: i32) -> Self {
        Self {
            start: 0,
            end: screen_lines,
            screen_lines,
        }
    }

    /// Whether the region still covers every row.
    pub fn is_full_screen(&self) -> bool {
        self.start == 0 && self.end == self.screen_lines
    }

    /// The DECSTBM sequence that puts a client's region where this one is.
    ///
    /// The whole screen is written as the parameterless reset rather than as an
    /// explicit `1;rows` pair, so it lands correctly even on a client whose own
    /// height has not caught up with ours yet.
    ///
    /// An empty region is written the same way. The engine clamps both margins to
    /// the screen height and so can end up with `start == end` when a program asks
    /// for a region that starts below the last row. There is no DECSTBM spelling
    /// for an empty region, and a client handed the inverted pair that would
    /// describe it rejects the sequence and keeps the whole screen, so this emits
    /// the whole screen directly rather than relying on that rejection.
    pub fn decstbm_sequence(&self) -> String {
        if self.is_full_screen() || self.start >= self.end {
            "\x1b[r".to_string()
        } else {
            format!("\x1b[{};{}r", self.start + 1, self.end)
        }
    }
}

/// The parser callbacks that move the region. Everything else keeps `Handler`'s
/// empty default body.
#[derive(Debug)]
struct RegionObserver {
    region: ScrollRegion,
}

impl RegionObserver {
    /// The region side effect of column mode (DECCOLM), which the engine runs on
    /// both polarities of the sequence.
    ///
    /// The engine ignores the column count itself (it will not switch a font) but
    /// still runs the rest of DECCOLM, and the first of those is widening the
    /// region back to the whole screen. It spells that as its own
    /// `set_scrolling_region(1, None)`, an ordinary method call rather than a
    /// parser dispatch, so nothing arrives at this observer as a set. Spell it
    /// the same way here rather than assigning the full screen directly, so the
    /// two stay identical if the engine's clamping ever changes.
    fn column_mode(&mut self, mode: PrivateMode) {
        if matches!(mode, PrivateMode::Named(NamedPrivateMode::ColumnMode)) {
            self.set_scrolling_region(1, None);
        }
    }
}

impl Handler for RegionObserver {
    /// Column mode is the only private mode that moves the region, and it moves
    /// it on the way in and on the way out alike.
    fn set_private_mode(&mut self, mode: PrivateMode) {
        self.column_mode(mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        self.column_mode(mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // Mirrors the engine's own handler, clamping included: an omitted bottom
        // means the last row, an inverted pair is refused outright, and both
        // margins are clamped to the screen height (to the height itself, not to
        // the last row, which is how a region can come out empty).
        let bottom = bottom.unwrap_or(self.region.screen_lines as usize);
        if top >= bottom {
            return;
        }
        let screen_lines = self.region.screen_lines;
        self.region.start = (top as i32 - 1).min(screen_lines);
        self.region.end = (bottom as i32).min(screen_lines);
    }

    /// A full reset (RIS) widens the region back to the whole screen. Nothing
    /// downstream says so, which is why this has to be mirrored: without it the
    /// tracker keeps reporting the margins the program had before the reset, and
    /// a repaint would hand a client a region the program has already given up.
    fn reset_state(&mut self) {
        self.region = ScrollRegion::full_lines(self.region.screen_lines);
    }
}

/// Runs a parser over the child's output and keeps the scrolling region it
/// implies.
pub struct ScrollRegionTracker {
    parser: Processor<StdSyncHandler>,
    observer: RegionObserver,
    /// Both dimensions, because the engine decides whether a resize touches its
    /// region by comparing both of them. Only the height reaches the region
    /// itself; the width is carried solely to answer "did anything change".
    rows: u16,
    cols: u16,
}

impl ScrollRegionTracker {
    /// A tracker for a terminal of this size that has not been written to.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Processor::new(),
            observer: RegionObserver {
                region: ScrollRegion::full(rows),
            },
            rows,
            cols,
        }
    }

    /// Feed the same bytes the terminal is being fed.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.observer, bytes);
    }

    /// Follow the terminal through a resize, which widens the region back to the
    /// whole screen at the new height.
    ///
    /// A resize to the size already in effect is not a resize: the engine
    /// compares both dimensions up front and returns before it reaches its
    /// region, so this returns too. That guard is the whole reason this takes a
    /// width it otherwise has no use for, and it is load bearing rather than an
    /// optimisation. A browser client sends its size on every reconnect, every
    /// tab focus, every visibility change and every input claim, and nearly all
    /// of those carry the size already in effect; widening on them would leave
    /// this reporting the whole screen while the child still had its margins, and
    /// the next reconnect would then assert the whole screen over a layout the
    /// program still has. A width-only change IS a real resize and does widen the
    /// region, which is why comparing heights alone would not do.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.rows == rows && self.cols == cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.observer.region = ScrollRegion::full(rows);
    }

    /// The region the child currently has.
    pub fn region(&self) -> ScrollRegion {
        self.observer.region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_tracker_has_the_whole_screen() {
        let tracker = ScrollRegionTracker::new(24, 80);
        assert_eq!(tracker.region(), ScrollRegion::full(24));
        assert!(tracker.region().is_full_screen());
    }

    #[test]
    fn the_whole_screen_is_written_as_the_parameterless_reset() {
        assert_eq!(ScrollRegion::full(24).decstbm_sequence(), "\x1b[r");
    }

    #[test]
    fn a_narrowed_region_is_written_as_one_based_inclusive_margins() {
        let mut tracker = ScrollRegionTracker::new(24, 80);
        tracker.advance(b"\x1b[3;20r");
        assert_eq!(tracker.region().decstbm_sequence(), "\x1b[3;20r");
    }

    #[test]
    fn column_mode_widens_the_region_on_both_polarities() {
        for sequence in [&b"\x1b[?3h"[..], &b"\x1b[?3l"[..]] {
            let mut tracker = ScrollRegionTracker::new(24, 80);
            tracker.advance(b"\x1b[3;20r");
            assert_eq!((tracker.region().start, tracker.region().end), (2, 20));
            tracker.advance(sequence);
            assert!(
                tracker.region().is_full_screen(),
                "column mode must widen back to the whole screen, got {:?} for {sequence:?}",
                tracker.region()
            );
        }
    }

    #[test]
    fn another_private_mode_leaves_the_region_alone() {
        // Only column mode moves the region. Bracketed paste is a private mode a
        // program sets routinely, and a handler that widened on any private mode
        // at all would throw the layout away every time one arrived.
        let mut tracker = ScrollRegionTracker::new(24, 80);
        tracker.advance(b"\x1b[3;20r");
        tracker.advance(b"\x1b[?2004h");
        tracker.advance(b"\x1b[?2004l");
        assert_eq!((tracker.region().start, tracker.region().end), (2, 20));
    }

    #[test]
    fn a_resize_widens_only_when_a_dimension_actually_changed() {
        let mut tracker = ScrollRegionTracker::new(24, 80);
        tracker.advance(b"\x1b[3;20r");

        tracker.resize(24, 80);
        assert_eq!(
            (tracker.region().start, tracker.region().end),
            (2, 20),
            "a resize to the size already in effect must move nothing"
        );

        // Width alone is still a resize.
        tracker.resize(24, 100);
        assert!(tracker.region().is_full_screen());

        // And so is height alone, at the new height.
        tracker.advance(b"\x1b[3;20r");
        tracker.resize(10, 100);
        assert_eq!(tracker.region(), ScrollRegion::full(10));
    }

    #[test]
    fn an_empty_region_is_written_as_the_whole_screen() {
        let region = ScrollRegion {
            start: 24,
            end: 24,
            screen_lines: 24,
        };
        assert!(!region.is_full_screen());
        assert_eq!(region.decstbm_sequence(), "\x1b[r");
    }
}
