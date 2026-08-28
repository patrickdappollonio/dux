//! Manual reordering of the sidebar's agents and terminals, driven by the
//! `move-agent-*` / `move-terminal-*` palette commands (the TUI's equivalent of
//! the web's drag-to-reorder). The pure `move_in_order` helper does the list
//! math; the `impl App` handlers apply it to the live agent/terminal order,
//! switch the sort to manual, persist, and keep the selection on the moved item.

use super::*;
use dux_core::engine::Command;

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

impl App {
    /// Move the selected agent within the global agent order and switch the sort
    /// to manual (like the web's drag-to-reorder), keeping the selection on the
    /// moved agent. Guards like the rest of the palette: no selected agent posts
    /// a status and does nothing; a single agent or an already-at-the-edge move
    /// is a no-op that leaves the sort mode untouched.
    pub(crate) fn move_selected_agent(&mut self, dir: MoveDir) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select an agent to move.");
            return;
        };
        let order: Vec<String> = self.engine.sessions.iter().map(|s| s.id.clone()).collect();
        if order.len() < 2 {
            self.set_info("Only one agent; nothing to reorder.");
            return;
        }
        let Some(idx) = order.iter().position(|id| *id == session_id) else {
            return;
        };
        let new_order = move_in_order(&order, idx, dir);
        self.apply_agent_order(&order, new_order, &session_id);
    }

    /// The COMPLETE agent id order a drop is computed against: the whole roster
    /// in the order the user is actually looking at.
    ///
    /// This is the cross-language twin of the web's `displayedSessionOrder`, and
    /// it must stay that way: the persisted order is shared, so a drop made on
    /// either surface has to mean the same thing. Every session is included,
    /// never just the rows a live query leaves on screen, because the stored
    /// order is total. Under a computed sort (active first, by name, by
    /// recency) the baseline is the displayed order: the main list as that
    /// comparator arranges it, then the quiet tail. Under MANUAL the stored
    /// order is taken verbatim instead, quiet agents left interleaved where the
    /// stored order has them, which is exactly how the web's manual drags have
    /// always computed their move.
    pub(crate) fn agent_drag_baseline(&self) -> Vec<String> {
        let mode = AgentSortMode::from_config_str(&self.engine.config.ui.agent_sort);
        if mode == AgentSortMode::Manual {
            return self.engine.sessions.iter().map(|s| s.id.clone()).collect();
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
        let order = dux_core::flat_list::order_sessions(
            &self.engine.sessions,
            mode.to_flat_sort_mode(),
            &|i| hot[i],
            &|_| true,
        );
        order
            .active
            .into_iter()
            .chain(order.inactive)
            .filter_map(|i| self.engine.sessions.get(i).map(|s| s.id.clone()))
            .collect()
    }

    /// Apply a new GLOBAL agent order: flip the sort to manual, persist the
    /// order, rebuild the sidebar, and leave the selection on `follow` (the id of
    /// the agent that moved).
    ///
    /// `baseline` is the order the caller computed `new_order` FROM, and an order
    /// that comes back equal to it is a no-op: nothing is persisted and the sort
    /// mode is left alone, so an already-at-the-edge move or a drop back where it
    /// started does not silently take the list off its computed sort. The
    /// baseline is a parameter rather than the current `engine.sessions` order
    /// because a drop's baseline is what the SCREEN shows, which in a computed
    /// sort mode is not the Vec order (the web's `handleDragEnd` compares against
    /// the same displayed baseline, for the same reason).
    ///
    /// Every caller that reorders agents goes through here, so the manual flip,
    /// the persisted write and the status the user reads cannot drift apart.
    pub(crate) fn apply_agent_order(
        &mut self,
        baseline: &[String],
        new_order: Vec<String>,
        follow: &str,
    ) {
        if new_order == baseline {
            return;
        }
        // Switch to manual so the display honors the new order.
        self.engine.set_agent_sort("manual");
        match self.engine.apply(Command::ReorderAgents {
            session_ids: new_order,
        }) {
            Ok(reaction) => self.apply_reaction(reaction),
            Err(err) => {
                self.set_error(format!("Could not reorder agents: {err}"));
                return;
            }
        }
        self.rebuild_left_items();
        // The selection follows the moved agent to its new row.
        let target = self
            .left_items()
            .iter()
            .enumerate()
            .find_map(|(pos, item)| match item {
                LeftItem::Session(i)
                    if self.engine.sessions.get(*i).map(|s| s.id.as_str()) == Some(follow) =>
                {
                    Some(pos)
                }
                _ => None,
            });
        if let Some(pos) = target {
            self.selected_left = pos;
        }
        self.set_info("Reordered agents. Sorting is now manual.");
    }

    /// Move the selected terminal within the terminal order and switch the sort
    /// to manual, keeping the selection on the moved terminal. Runtime-only
    /// (terminal order resets on restart). A single terminal or an
    /// already-at-the-edge move is a no-op that leaves the sort mode untouched.
    pub(crate) fn move_selected_terminal(&mut self, dir: MoveDir) {
        // The selection indexes the VISIBLE list, so the terminal to move comes
        // from there; the move itself happens in the FULL order, which is the
        // thing being persisted. Same shape as `move_selected_agent`, which
        // resolves the agent from the filtered sidebar and then moves it within
        // `engine.sessions`: a reorder must never rewrite the order as if the
        // rows a live query is hiding did not exist.
        let Some(terminal_id) = self
            .terminal_items()
            .get(self.selected_terminal_index)
            .map(|(id, _)| (*id).clone())
        else {
            self.set_error("Select a terminal to move.");
            return;
        };
        let order: Vec<String> = self
            .sorted_terminal_items()
            .iter()
            .map(|(id, _)| (*id).clone())
            .collect();
        if order.len() < 2 {
            self.set_info("Only one terminal; nothing to reorder.");
            return;
        }
        let Some(idx) = order.iter().position(|id| *id == terminal_id) else {
            return;
        };
        let new_order = move_in_order(&order, idx, dir);
        if new_order == order {
            return; // already at the relevant edge; leave the sort mode as it was
        }
        self.engine.set_agent_sort("manual");
        match self.engine.apply(Command::ReorderTerminals {
            terminal_ids: new_order,
        }) {
            Ok(reaction) => self.apply_reaction(reaction),
            Err(err) => {
                self.set_error(format!("Could not reorder terminals: {err}"));
                return;
            }
        }
        // The selection follows the moved terminal to its new row.
        if let Some(pos) = self
            .terminal_items()
            .iter()
            .position(|(id, _)| *id == &terminal_id)
        {
            self.selected_terminal_index = pos;
        }
        self.set_info("Reordered terminals. Sorting is now manual.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
