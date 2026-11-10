//! Manual reordering of the agents list's rows, agents and project and
//! standalone terminals together, driven by the `move-agent-*` /
//! `move-terminal-*` palette commands and the mouse drag (the TUI's equivalent of
//! the web's drag-to-reorder). The pure `move_in_order` helper does the list
//! math; the `impl App` handlers apply it to the live order, switch the sort to
//! manual, persist the agents' order, place each terminal after its agent, and
//! keep the selection on the moved row.

use super::{AgentSortMode, App, LeftRow};
use dux_core::engine::Command;
use dux_core::flat_list::FlatRow;

/// Direction for a manual reorder move in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveDir {
    Up,
    Down,
    Top,
    Bottom,
}

/// Move the item at `idx` within `order` in the given direction, returning the
/// new order. An out-of-range `idx`, or a move that is already at the relevant
/// edge, returns the order unchanged.
pub(crate) fn move_in_order<T: Clone>(order: &[T], idx: usize, dir: MoveDir) -> Vec<T> {
    let mut out = order.to_vec();
    if idx >= out.len() {
        return out;
    }
    match dir {
        MoveDir::Up => {
            if idx > 0 {
                out.swap(idx, idx - 1);
            }
        }
        MoveDir::Down => {
            if idx + 1 < out.len() {
                out.swap(idx, idx + 1);
            }
        }
        MoveDir::Top => {
            let item = out.remove(idx);
            out.insert(0, item);
        }
        MoveDir::Bottom => {
            let item = out.remove(idx);
            out.push(item);
        }
    }
    out
}

/// Move `active` to the slot `over` currently occupies, returning the new order.
/// The cross-language twin of the web's `moveItem` (`lib/reorder.ts`), down to
/// the no-op cases: identical ids, or an id that is not in `order`, return the
/// order unchanged. `over`'s slot is its index in the ORIGINAL order, so
/// dropping downward lands the moved item where the target was standing.
pub(crate) fn move_to_target<T: Clone + PartialEq>(order: &[T], active: &T, over: &T) -> Vec<T> {
    let mut out = order.to_vec();
    if active == over {
        return out;
    }
    let (Some(from), Some(to)) = (
        order.iter().position(|item| item == active),
        order.iter().position(|item| item == over),
    ) else {
        return out;
    };
    let item = out.remove(from);
    out.insert(to, item);
    out
}

/// Which gap a drop would land in, as an index into the CURRENT row order: the
/// gap sits above the row with that index, so `0` is above the first row and
/// `len` is below the last.
///
/// A drop puts the dragged row where the hovered row is standing
/// ([`move_to_target`]), which means it ends up on the FAR side of that row:
/// below it when the row is travelling down the list, above it when travelling
/// up. The insertion marker is painted at this gap, so what the line promises is
/// what the release does; the two are pinned together by a test.
pub(crate) fn drop_marker_boundary(source: usize, hover: usize) -> usize {
    if hover > source { hover + 1 } else { hover }
}

/// What a move that changed nothing says: the selected row is already at the
/// edge it was sent toward, so the order and the sort mode stay as they are.
fn already_at_edge_message(kind: &str, dir: MoveDir) -> String {
    let edge = match dir {
        MoveDir::Up | MoveDir::Top => "top",
        MoveDir::Down | MoveDir::Bottom => "bottom",
    };
    format!("That {kind} is already at the {edge} of the list, so nothing moved.")
}

impl App {
    /// Move the selected agent within the list and switch the sort to manual
    /// (like the web's drag-to-reorder), keeping the selection on the moved
    /// agent. The move is made in the order the user is looking at, the same
    /// baseline a drag uses, so under a computed sort the agent steps past the
    /// row it is drawn next to. Guards like the rest of the palette: no selected
    /// agent posts a status and does nothing; an already-at-the-edge move says
    /// so and leaves the sort mode untouched.
    pub(crate) fn move_selected_agent(&mut self, dir: MoveDir) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select an agent to move.");
            return;
        };
        self.move_list_row(&LeftRow::Agent(session_id), dir, "agent");
    }

    /// Move the selected project or standalone terminal within the list, as
    /// [`Self::move_selected_agent`] moves an agent. Only a terminal row of the
    /// list moves: an agent's own terminals are ordered in its dock.
    pub(crate) fn move_selected_terminal(&mut self, dir: MoveDir) {
        let Some(terminal_id) = self.selected_terminal_row() else {
            self.set_error(
                "Select a project or standalone terminal in the agents list to move it. An \
                 agent's own terminals are moved along its terminal dock with move-tab-left and \
                 move-tab-right.",
            );
            return;
        };
        self.move_list_row(&LeftRow::Terminal(terminal_id), dir, "terminal");
    }

    fn move_list_row(&mut self, row: &LeftRow, dir: MoveDir, kind: &str) {
        let order = self.list_drag_baseline();
        if order.len() < 2 {
            self.set_info("Only one row in the list; nothing to reorder.");
            return;
        }
        let Some(idx) = order.iter().position(|candidate| candidate == row) else {
            return;
        };
        let new_order = move_in_order(&order, idx, dir);
        if new_order == order {
            self.set_info(already_at_edge_message(kind, dir));
            return;
        }
        self.apply_list_order(&order, &new_order, row);
    }

    /// The complete row order a move or a drop is computed against: every agent
    /// and every project and standalone terminal, in the order the user is
    /// actually looking at.
    ///
    /// The cross-language twin of the web's `displayedSessionOrder`, and it must
    /// stay that way: the persisted agent order is shared, so a drop made on
    /// either surface has to mean the same thing. Every row is included, never
    /// just the rows a live query leaves on screen, because the stored order is
    /// total. Under a computed sort the baseline is the displayed order: the
    /// main list as that comparator arranges it, then the quiet tail. Under
    /// `Manual` the stored order is taken verbatim, quiet agents left
    /// interleaved where it has them.
    pub(crate) fn list_drag_baseline(&self) -> Vec<LeftRow> {
        let mode = AgentSortMode::from_config_str(&self.engine.config.ui.agent_sort);
        let terminal_ids = self.list_terminal_ids();
        let terminals = self.list_terminal_facts(&terminal_ids);
        let row = |flat: FlatRow| match flat {
            FlatRow::Agent(i) => LeftRow::Agent(self.engine.sessions[i].id.clone()),
            FlatRow::Terminal(i) => LeftRow::Terminal(terminal_ids[i].clone()),
        };
        if mode == AgentSortMode::Manual {
            return dux_core::flat_list::manual_rows(&self.engine.sessions, &terminals)
                .into_iter()
                .map(row)
                .collect();
        }
        // The same "working or waiting on you" mask the sidebar builds its rows
        // with, so the baseline and the screen agree about the active float.
        let hot: Vec<bool> = self
            .engine
            .sessions
            .iter()
            .map(|s| {
                self.engine.session_is_streaming(&s.id)
                    || self.engine.session_needs_attention(&s.id)
            })
            .collect();
        let has_terminal: Vec<bool> = self
            .engine
            .sessions
            .iter()
            .map(|s| self.session_terminal_count(&s.id) > 0)
            .collect();
        let order = dux_core::flat_list::order_rows(
            &self.engine.sessions,
            &terminals,
            mode.to_flat_sort_mode(),
            &dux_core::flat_list::AgentRowFacts {
                is_hot: &|i| hot[i],
                is_visible: &|_| true,
                has_running_terminal: &|i| has_terminal[i],
            },
            &|_| true,
        );
        order
            .main
            .into_iter()
            .map(row)
            .chain(
                order
                    .inactive
                    .into_iter()
                    .map(|i| LeftRow::Agent(self.engine.sessions[i].id.clone())),
            )
            .collect()
    }

    /// Apply a new list order: flip the sort to manual, persist the agents'
    /// order, place each terminal after the agent it now follows, rebuild the
    /// list, and leave the selection on `follow` (the row that moved).
    ///
    /// `baseline` is the order the caller computed `new_order` from, and an
    /// order that comes back equal to it is a no-op: nothing is persisted and
    /// the sort mode is left alone, so a drop back where it started does not
    /// silently take the list off its computed sort.
    ///
    /// Every caller that reorders the list goes through here, so the manual
    /// flip, the persisted write and the status the user reads cannot drift
    /// apart.
    pub(crate) fn apply_list_order(
        &mut self,
        baseline: &[LeftRow],
        new_order: &[LeftRow],
        follow: &LeftRow,
    ) {
        if new_order == baseline {
            return;
        }
        // Switch to manual so the display honors the new order.
        self.engine.set_agent_sort("manual");
        let agents = |order: &[LeftRow]| -> Vec<String> {
            order
                .iter()
                .filter_map(|row| match row {
                    LeftRow::Agent(id) => Some(id.clone()),
                    LeftRow::Terminal(_) | LeftRow::InactiveToggle => None,
                })
                .collect()
        };
        let agent_order = agents(new_order);
        let stored: Vec<String> = self.engine.sessions.iter().map(|s| s.id.clone()).collect();
        if agent_order != stored {
            match self.engine.apply(Command::ReorderAgents {
                session_ids: agent_order,
            }) {
                Ok(reaction) => self.apply_reaction(reaction),
                Err(err) => {
                    self.set_error(format!("Could not reorder the list: {err}"));
                    return;
                }
            }
        }
        // Each terminal now follows the agent above it, or leads the list.
        let mut after: Option<String> = None;
        let mut terminal_order: Vec<String> = Vec::new();
        for row in new_order {
            match row {
                LeftRow::Agent(id) => after = Some(id.clone()),
                LeftRow::Terminal(id) => {
                    self.engine.place_terminal_in_list(id, after.clone());
                    terminal_order.push(id.clone());
                }
                LeftRow::InactiveToggle => {}
            }
        }
        // The terminals' own order: the list's in the new order, then every
        // agent's dock in its own order, untouched.
        let mut docked: Vec<(&String, u64)> = self
            .engine
            .companion_terminals
            .iter()
            .filter(|(id, _)| !terminal_order.contains(id))
            .map(|(id, terminal)| (id, terminal.sort_order))
            .collect();
        docked.sort_by_key(|(_, order)| *order);
        let docked: Vec<String> = docked.into_iter().map(|(id, _)| id.clone()).collect();
        terminal_order.extend(docked);
        if let Err(err) = self.engine.apply(Command::ReorderTerminals {
            terminal_ids: terminal_order,
        }) {
            self.set_error(format!("Could not reorder the list: {err}"));
            return;
        }
        self.rebuild_left_items();
        // The selection follows the moved row to its new place.
        if let Some(pos) = self.left_rows.iter().position(|row| row == follow) {
            self.selected_left = pos;
            self.reconcile_center_target();
        }
        self.set_info("Reordered the agents list. Sorting is now manual.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker boundary and the drop must agree: for every source/hover pair,
    /// the row the drop actually lands on the far side of is the row the boundary
    /// names. This is the pin that keeps the painted line honest about where the
    /// release will put the row.
    #[test]
    fn the_marker_boundary_names_the_gap_the_drop_lands_in() {
        let ids = vec!["a", "b", "c", "d", "e"];
        for source in 0..ids.len() {
            for hover in 0..ids.len() {
                if hover == source {
                    continue;
                }
                let next = move_to_target(&ids, &ids[source], &ids[hover]);
                let landed = next
                    .iter()
                    .position(|id| *id == ids[source])
                    .expect("the moved row is still in the list");
                // The other rows, in their unchanged order: the boundary counts
                // gaps in the ORIGINAL list, so translate the landing slot into
                // the same terms by counting how many originals precede it.
                let boundary = drop_marker_boundary(source, hover);
                let originals_before = next
                    .iter()
                    .take(landed)
                    .filter(|id| **id != ids[source])
                    .count();
                let expected = if boundary > source {
                    // The source itself sat before the gap in the original list.
                    originals_before + 1
                } else {
                    originals_before
                };
                assert_eq!(
                    boundary, expected,
                    "source {source} hovering {hover} lands as {next:?}",
                );
            }
        }
    }

    /// `move_to_target` is the twin of the web's `moveItem`; these are its own
    /// test vectors, so a change to one language that is not mirrored fails here.
    #[test]
    fn move_to_target_moves_an_item_forward_to_the_over_slot() {
        assert_eq!(
            move_to_target(&["a", "b", "c", "d"], &"a", &"c"),
            vec!["b", "c", "a", "d"]
        );
    }

    #[test]
    fn move_to_target_moves_an_item_backward_to_the_over_slot() {
        assert_eq!(
            move_to_target(&["a", "b", "c", "d"], &"d", &"b"),
            vec!["a", "d", "b", "c"]
        );
    }

    #[test]
    fn move_to_target_is_a_no_op_for_the_same_item_or_a_missing_one() {
        assert_eq!(
            move_to_target(&["a", "b", "c"], &"b", &"b"),
            vec!["a", "b", "c"]
        );
        assert_eq!(move_to_target(&["a", "b"], &"x", &"a"), vec!["a", "b"]);
        assert_eq!(move_to_target(&["a", "b"], &"a", &"x"), vec!["a", "b"]);
    }

    #[test]
    fn move_up_swaps_with_the_previous_item() {
        assert_eq!(
            move_in_order(&["a", "b", "c"], 1, MoveDir::Up),
            vec!["b", "a", "c"]
        );
    }

    #[test]
    fn move_up_at_the_top_is_a_no_op() {
        assert_eq!(
            move_in_order(&["a", "b", "c"], 0, MoveDir::Up),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn move_down_swaps_with_the_next_item() {
        assert_eq!(
            move_in_order(&["a", "b", "c"], 1, MoveDir::Down),
            vec!["a", "c", "b"]
        );
    }

    #[test]
    fn move_down_at_the_bottom_is_a_no_op() {
        assert_eq!(
            move_in_order(&["a", "b", "c"], 2, MoveDir::Down),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn move_top_brings_the_item_to_the_front_keeping_the_rest_in_order() {
        assert_eq!(
            move_in_order(&["a", "b", "c", "d"], 2, MoveDir::Top),
            vec!["c", "a", "b", "d"]
        );
    }

    #[test]
    fn move_bottom_sends_the_item_to_the_end_keeping_the_rest_in_order() {
        assert_eq!(
            move_in_order(&["a", "b", "c", "d"], 1, MoveDir::Bottom),
            vec!["a", "c", "d", "b"]
        );
    }

    #[test]
    fn an_out_of_range_index_leaves_the_order_unchanged() {
        assert_eq!(move_in_order(&["a", "b"], 5, MoveDir::Up), vec!["a", "b"]);
    }
}
