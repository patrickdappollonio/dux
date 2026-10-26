//! The focus order of a modal, as data.
//!
//! A ring is the declared order of every stop a modal can ever have, each
//! paired with whether it is reachable right now, and [`next_focus`] walks it.
//! Conditional stops (a checkbox hidden in some states) are why it exists:
//! hand-written focus matches multiply arms over them.
//!
//! Pure and `App`-free, like the rest of `components`: callers hand in their
//! own focus enum.

/// Which way a focus or selection move travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusStep {
    /// Toward the next stop, the way Tab moves.
    Forward,
    /// Toward the previous stop, the way Shift-Tab moves.
    Backward,
}

/// Where focus goes when the user moves it.
///
/// `stops` is the modal's full declared order, every stop it can ever present,
/// in visual order, paired with whether that stop is reachable in the current
/// state. `direction` is the direction of travel.
///
/// Semantics, in the order they are checked:
///
/// 1. No stop is enabled: focus cannot move, so `current` is returned.
/// 2. `current` is a declared stop: walk from its declared index in the
///    requested direction, wrapping, and stop at the first enabled entry. This
///    holds whether or not `current` itself is enabled, which is what gives a
///    focus stranded on a stop that just disappeared a defined way out.
/// 3. `current` is not declared at all: return the first enabled stop.
pub(crate) fn next_focus<T: Copy + PartialEq>(
    stops: &[(T, bool)],
    current: T,
    direction: FocusStep,
) -> T {
    if !stops.iter().any(|&(_, enabled)| enabled) {
        return current;
    }
    let len = stops.len();
    let Some(start) = stops.iter().position(|&(stop, _)| stop == current) else {
        return stops
            .iter()
            .find(|&&(_, enabled)| enabled)
            .map_or(current, |&(stop, _)| stop);
    };
    for step in 1..=len {
        let index = if direction == FocusStep::Forward {
            (start + step) % len
        } else {
            (start + len - (step % len)) % len
        };
        let (stop, enabled) = stops[index];
        if enabled {
            return stop;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Stop {
        A,
        B,
        C,
    }
    use Stop::{A, B, C};

    #[test]
    fn walks_forward_and_wraps() {
        let ring = [(A, true), (B, true), (C, true)];
        assert_eq!(next_focus(&ring, A, FocusStep::Forward), B);
        assert_eq!(next_focus(&ring, B, FocusStep::Forward), C);
        assert_eq!(next_focus(&ring, C, FocusStep::Forward), A);
    }

    #[test]
    fn walks_backward_and_wraps() {
        let ring = [(A, true), (B, true), (C, true)];
        assert_eq!(next_focus(&ring, A, FocusStep::Backward), C);
        assert_eq!(next_focus(&ring, C, FocusStep::Backward), B);
        assert_eq!(next_focus(&ring, B, FocusStep::Backward), A);
    }

    #[test]
    fn skips_a_disabled_stop_in_both_directions() {
        let ring = [(A, true), (B, false), (C, true)];
        assert_eq!(next_focus(&ring, A, FocusStep::Forward), C);
        assert_eq!(next_focus(&ring, C, FocusStep::Backward), A);
    }

    #[test]
    fn a_focus_stranded_on_a_vanished_stop_walks_from_that_stop() {
        let ring = [(A, true), (B, true), (C, false)];
        assert_eq!(next_focus(&ring, C, FocusStep::Forward), A);
        assert_eq!(next_focus(&ring, C, FocusStep::Backward), B);
    }

    #[test]
    fn an_undeclared_focus_lands_on_the_first_enabled_stop() {
        let ring = [(A, false), (B, true)];
        assert_eq!(next_focus(&ring[1..], A, FocusStep::Forward), B);
    }

    #[test]
    fn a_ring_with_no_enabled_stop_cannot_move() {
        let ring = [(A, false), (B, false)];
        assert_eq!(next_focus(&ring, A, FocusStep::Forward), A);
        assert_eq!(next_focus(&ring, A, FocusStep::Backward), A);
    }

    #[test]
    fn a_single_enabled_stop_is_its_own_neighbour() {
        let ring = [(A, true), (B, false)];
        assert_eq!(next_focus(&ring, A, FocusStep::Forward), A);
        assert_eq!(next_focus(&ring, A, FocusStep::Backward), A);
    }
}
