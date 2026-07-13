//! Terminal-window focus tracking for attention "viewed" gating.
//!
//! The TUI stamps the focused agent tab as "viewed" once per tick, which clears
//! its needs-attention flag and suppresses new ones for a short engaged window.
//! That is correct only while the user is actually looking at the terminal. When
//! the terminal window sits in another workspace we must stop stamping so a new
//! attention request can rise, and on return we hold a short grace so the user
//! has time to see which agent(s) wanted them before the indicator clears.
//!
//! This module is a pure state machine over the host's DEC mode 1004 focus
//! reports (`ESC [ I` / `ESC [ O`). It is deliberately free of I/O so the exact
//! grace semantics can be unit tested with explicit `Instant`/`Duration`
//! arithmetic, honoring the "wall-clock, not tick counts" tenet.

use std::time::{Duration, Instant};

/// Terminal-window focus as reported by the host via DEC mode 1004.
///
/// Fail-open: until the first focus event of this run is observed we assume
/// focused, so terminals (or tmux setups without `focus-events on`) that never
/// report focus behave exactly as before this feature existed.
#[derive(Debug, Clone)]
pub struct TerminalFocus {
    /// Assumed `true` until proven otherwise by a `FocusLost` event.
    focused: bool,
    /// Fail-open guard: stays `false` until the first focus event is observed.
    ever_saw_focus_event: bool,
    /// Set on a genuine unfocused->focused transition; starts the grace window.
    regained_at: Option<Instant>,
}

impl Default for TerminalFocus {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalFocus {
    /// A fresh state that fails open (assumes focused, no grace pending).
    pub fn new() -> Self {
        Self {
            focused: true,
            ever_saw_focus_event: false,
            regained_at: None,
        }
    }

    /// Record a `FocusGained` report.
    ///
    /// A genuine unfocused->focused transition starts the grace window from
    /// `now`. The first-ever event being a `FocusGained` (some terminals report
    /// the current state the moment mode 1004 is enabled) simply confirms the
    /// fail-open assumption with no grace, so startup does not blip. A duplicate
    /// `FocusGained` while already focused is a no-op and never restarts a
    /// running grace.
    pub fn on_focus_gained(&mut self, now: Instant) {
        // `!self.focused` alone already implies `ever_saw_focus_event`: the
        // only setter of `focused = false` is `on_focus_lost`, which always
        // sets `ever_saw_focus_event = true` in the same call. The explicit
        // `ever_saw_focus_event &&` guard is kept only to document that
        // invariant; the assertion below catches a future setter of
        // `focused = false` that forgets to also set `ever_saw_focus_event`.
        debug_assert!(self.focused || self.ever_saw_focus_event);
        if self.ever_saw_focus_event && !self.focused {
            self.regained_at = Some(now);
        }
        self.focused = true;
        self.ever_saw_focus_event = true;
    }

    /// Record a `FocusLost` report: we are unfocused and any pending grace is
    /// discarded (a later regain starts a fresh grace).
    pub fn on_focus_lost(&mut self) {
        self.focused = false;
        self.ever_saw_focus_event = true;
        self.regained_at = None;
    }

    /// Whether the per-tick viewed stamp should fire right now.
    ///
    /// - Before any focus event is seen: `true` (fail open).
    /// - While unfocused: `false`.
    /// - Within `grace` of a genuine refocus: `false`; at or after the boundary
    ///   (`elapsed >= grace`): `true`. A `grace` of zero disables the delay, so
    ///   a refocus resumes stamping immediately.
    ///
    /// Pure `&self`: an expired `regained_at` is ignored, never mutated.
    pub fn should_stamp_viewed(&self, now: Instant, grace: Duration) -> bool {
        if !self.ever_saw_focus_event {
            return true;
        }
        if !self.focused {
            return false;
        }
        if let Some(regained_at) = self.regained_at
            && now.duration_since(regained_at) < grace
        {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_open_before_any_focus_event_stamps() {
        let focus = TerminalFocus::new();
        let now = Instant::now();
        assert!(focus.should_stamp_viewed(now, Duration::from_secs(3)));
        assert!(focus.should_stamp_viewed(now, Duration::ZERO));
    }

    #[test]
    fn focus_lost_stops_stamping() {
        let mut focus = TerminalFocus::new();
        focus.on_focus_lost();
        assert!(!focus.should_stamp_viewed(Instant::now(), Duration::from_secs(3)));
    }

    #[test]
    fn first_focus_gained_without_prior_loss_stamps_immediately() {
        let mut focus = TerminalFocus::new();
        let now = Instant::now();
        focus.on_focus_gained(now);
        // No grace was started because this only confirmed the fail-open state.
        assert!(focus.should_stamp_viewed(now, Duration::from_secs(3)));
    }

    #[test]
    fn refocus_within_grace_suppresses_stamping() {
        let mut focus = TerminalFocus::new();
        let grace = Duration::from_secs(3);
        focus.on_focus_lost();
        let t = Instant::now();
        focus.on_focus_gained(t);
        assert!(!focus.should_stamp_viewed(t + grace - Duration::from_millis(1), grace));
    }

    #[test]
    fn refocus_at_grace_boundary_resumes_stamping() {
        let mut focus = TerminalFocus::new();
        let grace = Duration::from_secs(3);
        focus.on_focus_lost();
        let t = Instant::now();
        focus.on_focus_gained(t);
        assert!(focus.should_stamp_viewed(t + grace, grace));
    }

    #[test]
    fn refocus_after_grace_stamps() {
        let mut focus = TerminalFocus::new();
        let grace = Duration::from_secs(3);
        focus.on_focus_lost();
        let t = Instant::now();
        focus.on_focus_gained(t);
        assert!(focus.should_stamp_viewed(t + Duration::from_secs(30), grace));
    }

    #[test]
    fn zero_grace_stamps_immediately_on_refocus() {
        let mut focus = TerminalFocus::new();
        focus.on_focus_lost();
        let t = Instant::now();
        focus.on_focus_gained(t);
        assert!(focus.should_stamp_viewed(t, Duration::ZERO));
    }

    #[test]
    fn focus_lost_during_grace_suppresses_and_clears_grace() {
        let mut focus = TerminalFocus::new();
        let grace = Duration::from_secs(3);
        focus.on_focus_lost();
        let t0 = Instant::now();
        focus.on_focus_gained(t0);
        // Lost again mid-grace: unfocused, so no stamping and grace cleared.
        focus.on_focus_lost();
        assert!(!focus.should_stamp_viewed(t0 + Duration::from_millis(1), grace));

        // A later regain starts a fresh grace from the new instant.
        let t1 = t0 + Duration::from_secs(10);
        focus.on_focus_gained(t1);
        assert!(!focus.should_stamp_viewed(t1 + grace - Duration::from_millis(1), grace));
        assert!(focus.should_stamp_viewed(t1 + grace, grace));
    }

    #[test]
    fn duplicate_focus_gained_does_not_restart_grace() {
        let mut focus = TerminalFocus::new();
        let grace = Duration::from_secs(3);
        focus.on_focus_lost();
        let t = Instant::now();
        focus.on_focus_gained(t);
        // A second FocusGained while already focused must not move the grace
        // origin forward.
        focus.on_focus_gained(t + Duration::from_secs(2));
        assert!(focus.should_stamp_viewed(t + grace, grace));
    }
}
