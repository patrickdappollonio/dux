//! The one order an agent's pull requests are listed in, on every surface and
//! the command line, and the changes a user can make to it.
//!
//! Element zero of the visible list is the pull request the agent shows.

use std::cmp::Ordering;

use crate::model::{AgentPr, PrKey, PrState};

/// How an agent's pull requests are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrSortMode {
    /// Open, then draft, then merged, then closed; newest first within each.
    #[default]
    Automatic,
    /// The order the user arranged. Rows the user has not placed yet follow
    /// the placed ones, in automatic order.
    Manual,
}

impl PrSortMode {
    /// The word the session database stores.
    #[must_use]
    pub fn stored_text(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }

    /// The mode a stored word names. Anything else is automatic, the order
    /// that needs nothing the user did.
    #[must_use]
    pub fn from_stored(text: &str) -> Self {
        match text {
            "manual" => Self::Manual,
            _ => Self::Automatic,
        }
    }
}

/// An agent's pull requests in order: the ones it shows, then the ones the
/// user hid, which are listed in automatic order whatever the mode.
#[derive(Debug, PartialEq, Eq)]
pub struct OrderedPrs<'a> {
    /// The rows not hidden, in the mode's order.
    pub visible: Vec<&'a AgentPr>,
    /// The hidden rows, in automatic order.
    pub hidden: Vec<&'a AgentPr>,
}

/// Which way to step through the visible list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Toward the end, wrapping to the start.
    Next,
    /// Toward the start, wrapping to the end.
    Previous,
}

/// Where a move puts a pull request in the visible list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTo {
    /// First.
    Top,
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// At this place, counted from 1.
    Position(usize),
}

/// A change to the order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrange {
    /// Move one visible pull request.
    Move {
        /// The pull request to move.
        key: PrKey,
        /// Where to.
        to: MoveTo,
    },
    /// The whole visible list in a new order, as a drag leaves it.
    Drop {
        /// Every visible pull request, in the order wanted.
        visible_order: Vec<PrKey>,
    },
}

/// What a change did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangeOutcome {
    /// The order changed and every row now has a manual slot.
    Moved {
        /// The mode was automatic, so the caller now switches it to manual.
        switched_to_manual: bool,
        /// A manual order saved earlier was replaced by the one on screen.
        replaced_saved: bool,
    },
    /// The pull request is already where the move would put it.
    AtEdge,
    /// The change names a pull request that is not visible, or a drop does
    /// not name exactly the visible ones.
    Stale,
    /// A position outside 1 to the visible length.
    OutOfRange {
        /// How many pull requests are visible.
        visible: usize,
    },
}

fn state_rank(state: &PrState) -> u8 {
    match state {
        PrState::Open => 0,
        PrState::Draft => 1,
        PrState::Merged => 2,
        PrState::Closed => 3,
    }
}

/// The automatic order: a total order, so no two surfaces can disagree.
fn automatic_cmp(a: &AgentPr, b: &AgentPr) -> Ordering {
    state_rank(&a.state)
        .cmp(&state_rank(&b.state))
        .then_with(|| b.gh_created_at.cmp(&a.gh_created_at))
        .then_with(|| b.first_recorded_at.cmp(&a.first_recorded_at))
        .then_with(|| b.number.cmp(&a.number))
        .then_with(|| {
            a.host
                .to_ascii_lowercase()
                .cmp(&b.host.to_ascii_lowercase())
        })
        .then_with(|| {
            a.owner_repo
                .to_ascii_lowercase()
                .cmp(&b.owner_repo.to_ascii_lowercase())
        })
}

/// The manual order: placed rows by slot, then the rest in automatic order.
fn manual_cmp(a: &AgentPr, b: &AgentPr) -> Ordering {
    match (a.manual_slot, b.manual_slot) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| automatic_cmp(a, b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => automatic_cmp(a, b),
    }
}

/// Every row, hidden ones included, in `mode`'s order.
fn sequence(rows: &[AgentPr], mode: PrSortMode) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..rows.len()).collect();
    match mode {
        PrSortMode::Automatic => indices.sort_by(|&a, &b| automatic_cmp(&rows[a], &rows[b])),
        PrSortMode::Manual => indices.sort_by(|&a, &b| manual_cmp(&rows[a], &rows[b])),
    }
    indices
}

/// An agent's rows in order.
#[must_use]
pub fn order(rows: &[AgentPr], mode: PrSortMode) -> OrderedPrs<'_> {
    let visible = sequence(rows, mode)
        .into_iter()
        .map(|i| &rows[i])
        .filter(|row| !row.hidden)
        .collect();
    let hidden = sequence(rows, PrSortMode::Automatic)
        .into_iter()
        .map(|i| &rows[i])
        .filter(|row| row.hidden)
        .collect();
    OrderedPrs { visible, hidden }
}

/// The pull request the agent shows: the first visible one.
#[must_use]
pub fn shown(rows: &[AgentPr], mode: PrSortMode) -> Option<&AgentPr> {
    order(rows, mode).visible.into_iter().next()
}

/// The place one step from `from` in a list of `len`, wrapping at both ends.
#[must_use]
pub fn step(len: usize, from: usize, dir: Step) -> usize {
    if len == 0 {
        return 0;
    }
    let from = from % len;
    match dir {
        Step::Next => (from + 1) % len,
        Step::Previous => (from + len - 1) % len,
    }
}

/// The visible order a move asks for, or why there is none.
fn moved_visible(visible: &[PrKey], key: &PrKey, to: MoveTo) -> Result<Vec<PrKey>, ArrangeOutcome> {
    let Some(from) = visible.iter().position(|k| k == key) else {
        return Err(ArrangeOutcome::Stale);
    };
    let target = match to {
        MoveTo::Top => 0,
        MoveTo::Up => from.checked_sub(1).ok_or(ArrangeOutcome::AtEdge)?,
        MoveTo::Down => from + 1,
        MoveTo::Position(n) => {
            if n == 0 || n > visible.len() {
                return Err(ArrangeOutcome::OutOfRange {
                    visible: visible.len(),
                });
            }
            n - 1
        }
    };
    if target == from || target >= visible.len() {
        return Err(ArrangeOutcome::AtEdge);
    }
    let mut next = visible.to_vec();
    let moved = next.remove(from);
    next.insert(target, moved);
    Ok(next)
}

/// Whether `wanted` names exactly the keys in `visible`, each once.
fn same_set(visible: &[PrKey], wanted: &[PrKey]) -> bool {
    wanted.len() == visible.len()
        && visible.iter().all(|k| wanted.contains(k))
        && wanted
            .iter()
            .enumerate()
            .all(|(i, k)| !wanted[..i].contains(k))
}

/// The one row whose move turns `before` into `after`, when one does. A swap
/// of two neighbours is a move of either; the earlier one is taken.
pub(crate) fn single_moved(before: &[PrKey], after: &[PrKey]) -> Option<PrKey> {
    before.iter().find_map(|candidate| {
        let rest = |list: &[PrKey]| -> Vec<PrKey> {
            list.iter().filter(|k| *k != candidate).cloned().collect()
        };
        (rest(before) == rest(after)).then(|| candidate.clone())
    })
}

/// Put `key` into `full` (every row's key, hidden ones included) so that it
/// sits immediately before the visible row after it in `visible`, or
/// immediately after the one before it when it is last. Hidden rows keep their
/// places that way, so a row shown again comes back where it was.
fn place(full: &mut Vec<PrKey>, visible: &[PrKey], key: &PrKey) {
    let Some(at) = visible.iter().position(|k| k == key) else {
        return;
    };
    full.retain(|k| k != key);
    let index = if let Some(next) = visible.get(at + 1) {
        full.iter().position(|k| k == next).unwrap_or(full.len())
    } else if let Some(prev) = at.checked_sub(1).and_then(|i| visible.get(i)) {
        full.iter()
            .position(|k| k == prev)
            .map_or(full.len(), |i| i + 1)
    } else {
        0
    };
    full.insert(index, key.clone());
}

/// Apply a change to an agent's order. Every row (hidden ones included) gets
/// a dense manual slot. In automatic mode the order on screen is the
/// starting point and replaces any manual order saved earlier; the caller
/// then switches the mode to manual, as the outcome says.
pub fn arrange(rows: &mut [AgentPr], mode: PrSortMode, change: Arrange) -> ArrangeOutcome {
    let visible: Vec<PrKey> = order(rows, mode)
        .visible
        .iter()
        .map(|row| row.key())
        .collect();
    let (wanted, moved) = match change {
        Arrange::Move { key, to } => match moved_visible(&visible, &key, to) {
            Ok(next) => (next, Some(key)),
            Err(outcome) => return outcome,
        },
        Arrange::Drop { visible_order } => {
            if !same_set(&visible, &visible_order) {
                return ArrangeOutcome::Stale;
            }
            if visible_order == visible {
                return ArrangeOutcome::AtEdge;
            }
            let moved = single_moved(&visible, &visible_order);
            (visible_order, moved)
        }
    };
    let mut full: Vec<PrKey> = sequence(rows, mode)
        .into_iter()
        .map(|i| rows[i].key())
        .collect();
    if let Some(moved) = moved {
        place(&mut full, &wanted, &moved);
    } else {
        // Hidden rows keep their places; the visible places take the wanted
        // order.
        let mut next = wanted.iter();
        for slot in &mut full {
            if visible.contains(slot)
                && let Some(key) = next.next()
            {
                slot.clone_from(key);
            }
        }
    }
    let replaced_saved =
        mode == PrSortMode::Automatic && rows.iter().any(|row| row.manual_slot.is_some());
    for row in rows.iter_mut() {
        let slot = full.iter().position(|k| row.is(k)).unwrap_or(full.len());
        row.manual_slot = Some(u32::try_from(slot).unwrap_or(u32::MAX));
    }
    ArrangeOutcome::Moved {
        switched_to_manual: mode == PrSortMode::Automatic,
        replaced_saved,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap()
    }

    fn row(number: u64, state: PrState, created_day: Option<u32>) -> AgentPr {
        AgentPr {
            host: "github.com".to_string(),
            owner_repo: "ada/widget".to_string(),
            number,
            state,
            title: format!("PR {number}"),
            url: String::new(),
            head_branch: None,
            found: true,
            attached: false,
            hidden: false,
            manual_slot: None,
            gh_created_at: created_day.map(at),
            gh_updated_at: None,
            gh_closed_at: None,
            gh_merged_at: None,
            first_recorded_at: at(1),
            last_refreshed_at: None,
        }
    }

    fn numbers(list: &[&AgentPr]) -> Vec<u64> {
        list.iter().map(|row| row.number).collect()
    }

    fn visible(rows: &[AgentPr], mode: PrSortMode) -> Vec<u64> {
        numbers(&order(rows, mode).visible)
    }

    fn key(number: u64) -> PrKey {
        PrKey::new("github.com", "ada/widget", number)
    }

    #[test]
    fn automatic_order_ranks_the_states_and_puts_the_newest_first_with_ties_broken() {
        let mut rows = vec![
            row(1, PrState::Closed, Some(9)),
            row(2, PrState::Merged, Some(3)),
            row(3, PrState::Draft, Some(2)),
            row(4, PrState::Open, Some(2)),
            row(5, PrState::Open, Some(5)),
            row(6, PrState::Merged, Some(8)),
            // No GitHub time: after the ones that have one, by recording time.
            row(7, PrState::Open, None),
            // The same times as 7: the higher number first.
            row(8, PrState::Open, None),
        ];
        rows[6].first_recorded_at = at(4);
        rows[7].first_recorded_at = at(4);
        let mut other_repo = row(8, PrState::Open, None);
        other_repo.owner_repo = "Ada/Aardvark".to_string();
        other_repo.first_recorded_at = at(4);
        rows.push(other_repo);
        let ordered = order(&rows, PrSortMode::Automatic);
        assert_eq!(numbers(&ordered.visible), vec![5, 4, 8, 8, 7, 3, 6, 2, 1]);
        assert_eq!(
            ordered.visible[2].owner_repo, "Ada/Aardvark",
            "equal in every other way, the repository decides"
        );
        assert_eq!(
            shown(&rows, PrSortMode::Automatic).map(|r| r.number),
            Some(5)
        );
    }

    #[test]
    fn manual_order_puts_placed_rows_first_and_the_rest_in_automatic_order() {
        let mut rows = vec![
            row(1, PrState::Open, Some(1)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Merged, Some(3)),
            row(4, PrState::Open, Some(4)),
        ];
        rows[2].manual_slot = Some(0);
        rows[0].manual_slot = Some(1);
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![3, 1, 4, 2]);
        assert_eq!(visible(&rows, PrSortMode::Automatic), vec![4, 2, 1, 3]);
    }

    #[test]
    fn hidden_rows_are_listed_apart_in_automatic_order() {
        let mut rows = vec![
            row(1, PrState::Closed, Some(1)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Open, Some(3)),
        ];
        rows[0].hidden = true;
        rows[1].hidden = true;
        rows[0].manual_slot = Some(0);
        rows[1].manual_slot = Some(1);
        let ordered = order(&rows, PrSortMode::Manual);
        assert_eq!(numbers(&ordered.visible), vec![3]);
        assert_eq!(numbers(&ordered.hidden), vec![2, 1]);
    }

    #[test]
    fn a_hidden_row_shown_again_comes_back_to_its_own_place() {
        let mut rows = vec![
            row(1, PrState::Open, Some(3)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Open, Some(1)),
        ];
        for (slot, row) in rows.iter_mut().enumerate() {
            row.manual_slot = Some(u32::try_from(slot).unwrap());
        }
        rows[1].hidden = true;
        assert_eq!(
            arrange(
                &mut rows,
                PrSortMode::Manual,
                Arrange::Move {
                    key: key(3),
                    to: MoveTo::Up,
                },
            ),
            ArrangeOutcome::Moved {
                switched_to_manual: false,
                replaced_saved: false,
            }
        );
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![3, 1]);
        rows[1].hidden = false;
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![3, 1, 2]);
    }

    #[test]
    fn a_move_made_in_automatic_order_seeds_a_new_manual_order_from_the_screen() {
        let mut rows = vec![
            row(1, PrState::Open, Some(1)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Open, Some(3)),
        ];
        // A saved manual order the user is not looking at: 1, 2, 3.
        for (slot, row) in rows.iter_mut().enumerate() {
            row.manual_slot = Some(u32::try_from(slot).unwrap());
        }
        assert_eq!(visible(&rows, PrSortMode::Automatic), vec![3, 2, 1]);
        let outcome = arrange(
            &mut rows,
            PrSortMode::Automatic,
            Arrange::Move {
                key: key(1),
                to: MoveTo::Top,
            },
        );
        assert_eq!(
            outcome,
            ArrangeOutcome::Moved {
                switched_to_manual: true,
                replaced_saved: true,
            }
        );
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![1, 3, 2]);
    }

    #[test]
    fn switching_to_automatic_and_back_without_a_move_keeps_the_saved_order() {
        let mut rows = vec![
            row(1, PrState::Open, Some(1)),
            row(2, PrState::Open, Some(2)),
        ];
        rows[0].manual_slot = Some(0);
        rows[1].manual_slot = Some(1);
        let before = rows.clone();
        assert_eq!(visible(&rows, PrSortMode::Automatic), vec![2, 1]);
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![1, 2]);
        assert_eq!(rows, before);
    }

    #[test]
    fn a_move_to_a_position_counts_from_one_and_refuses_outside_the_list() {
        let mut rows = vec![
            row(1, PrState::Open, Some(3)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Open, Some(1)),
        ];
        let to = |n| Arrange::Move {
            key: key(1),
            to: MoveTo::Position(n),
        };
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, to(4)),
            ArrangeOutcome::OutOfRange { visible: 3 }
        );
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, to(0)),
            ArrangeOutcome::OutOfRange { visible: 3 }
        );
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, to(1)),
            ArrangeOutcome::AtEdge
        );
        assert!(rows.iter().all(|row| row.manual_slot.is_none()));
        assert!(matches!(
            arrange(&mut rows, PrSortMode::Automatic, to(2)),
            ArrangeOutcome::Moved { .. }
        ));
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![2, 1, 3]);
    }

    #[test]
    fn a_move_past_either_end_changes_nothing() {
        let mut rows = vec![
            row(1, PrState::Open, Some(2)),
            row(2, PrState::Open, Some(1)),
        ];
        let mv = |number, to| Arrange::Move {
            key: key(number),
            to,
        };
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, mv(1, MoveTo::Up)),
            ArrangeOutcome::AtEdge
        );
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, mv(2, MoveTo::Down)),
            ArrangeOutcome::AtEdge
        );
        assert_eq!(
            arrange(&mut rows, PrSortMode::Automatic, mv(9, MoveTo::Top)),
            ArrangeOutcome::Stale,
            "a pull request the agent does not have"
        );
    }

    #[test]
    fn a_drop_takes_the_order_it_names_and_refuses_anything_but_the_visible_set() {
        let mut rows = vec![
            row(1, PrState::Open, Some(3)),
            row(2, PrState::Open, Some(2)),
            row(3, PrState::Open, Some(1)),
        ];
        rows[2].hidden = true;
        let drop = |keys: Vec<u64>| Arrange::Drop {
            visible_order: keys.into_iter().map(key).collect(),
        };
        for stale in [vec![2], vec![2, 1, 3], vec![2, 2], vec![2, 4]] {
            assert_eq!(
                arrange(&mut rows, PrSortMode::Automatic, drop(stale.clone())),
                ArrangeOutcome::Stale,
                "{stale:?}"
            );
        }
        assert!(matches!(
            arrange(&mut rows, PrSortMode::Automatic, drop(vec![2, 1])),
            ArrangeOutcome::Moved { .. }
        ));
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![2, 1]);

        // A reversal is more than one move.
        rows[2].hidden = false;
        assert!(matches!(
            arrange(&mut rows, PrSortMode::Manual, drop(vec![3, 1, 2])),
            ArrangeOutcome::Moved { .. }
        ));
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![3, 1, 2]);
        assert!(matches!(
            arrange(&mut rows, PrSortMode::Manual, drop(vec![2, 1, 3])),
            ArrangeOutcome::Moved { .. }
        ));
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![2, 1, 3]);

        // Two separate swaps with a hidden row between them: the hidden row
        // keeps its place in the sequence.
        let mut rows = vec![
            row(1, PrState::Open, Some(5)),
            row(2, PrState::Open, Some(4)),
            row(9, PrState::Open, Some(3)),
            row(3, PrState::Open, Some(2)),
            row(4, PrState::Open, Some(1)),
        ];
        rows[2].hidden = true;
        assert!(matches!(
            arrange(&mut rows, PrSortMode::Automatic, drop(vec![2, 1, 4, 3])),
            ArrangeOutcome::Moved { .. }
        ));
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![2, 1, 4, 3]);
        rows[2].hidden = false;
        assert_eq!(visible(&rows, PrSortMode::Manual), vec![2, 1, 9, 4, 3]);
    }

    #[test]
    fn step_wraps_both_ways() {
        assert_eq!(step(3, 2, Step::Next), 0);
        assert_eq!(step(3, 0, Step::Previous), 2);
        assert_eq!(step(3, 1, Step::Next), 2);
        assert_eq!(step(0, 0, Step::Next), 0);
    }
}
