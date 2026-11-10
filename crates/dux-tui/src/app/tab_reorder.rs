//! Reordering the selected agent's provider tabs: a mouse drag along the strip
//! and the `move-tab-left` / `move-tab-right` palette commands. The order is the
//! agent's own and is stored by the engine, so it survives a restart; which tab
//! holds the agent's first-tab slot never changes here.
//!
//! The drag is keyed by tab id and checked against the strip again on release,
//! because the strip is rebuilt whenever a tab arrives or closes, and a release
//! must never move a tab nobody picked up.

use super::input::contains_point;
use super::reorder::{MoveDir, move_in_order, move_to_target};
use super::{App, DrawnTabStrip, TabDragState};
use crossterm::event::MouseEvent;
use dux_core::engine::Command;
use dux_core::ids::{SessionIdRef, TabIdRef};

impl App {
    /// Arm a drag on the pill the press landed on. Not yet a drag: it becomes
    /// one when the pointer reaches a different pill, so a press that releases
    /// where it landed stays the plain click that focuses the tab.
    pub(super) fn arm_tab_drag(&mut self, tab_id: &str) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        self.tab_drag = Some(TabDragState {
            session_id,
            source_id: tab_id.to_string(),
            hover_id: None,
            promoted: false,
        });
    }

    /// The pill the pointer is over, or `None` between pills and off the strip.
    fn tab_under_pointer(&self, mouse: MouseEvent) -> Option<String> {
        self.agent_tab_regions
            .iter()
            .find(|(_, rect)| contains_point(*rect, mouse.column, mouse.row))
            .map(|(id, _)| id.clone())
    }

    /// Track the pointer while a tab drag is armed. The gesture is promoted the
    /// moment the pointer reaches a pill other than the one it pressed, and
    /// `hover` is `None` wherever a drop would mean nothing.
    pub(super) fn continue_tab_drag(&mut self, mouse: MouseEvent) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let over = self.tab_under_pointer(mouse);
        if !drag.promoted && over.as_deref().is_none_or(|id| id == drag.source_id) {
            self.tab_drag = Some(drag);
            return;
        }
        if !drag.promoted {
            // The press is a drag now, so it can no longer pair into a double
            // click with the next one.
            self.last_mouse_click = None;
        }
        drag.promoted = true;
        drag.hover_id = over.filter(|id| *id != drag.source_id);
        self.tab_drag = Some(drag);
    }

    /// Retire the drag on release and, when it ended on another pill, move the
    /// picked-up tab to the place that pill holds.
    pub(super) fn finish_tab_drag(&mut self) {
        let Some(drag) = self.tab_drag.take() else {
            return;
        };
        let (true, Some(target)) = (drag.promoted, drag.hover_id) else {
            return;
        };
        if self.selected_session().map(|s| s.id.as_str()) != Some(drag.session_id.as_str()) {
            return;
        }
        let order = self.session_tab_ids(&drag.session_id);
        if !order.contains(&drag.source_id) || !order.contains(&target) {
            return;
        }
        let moved = move_to_target(&order, &drag.source_id, &target);
        self.apply_tab_order(&drag.session_id, &drag.source_id, &order, moved);
    }

    /// Move the focused tab of the selected agent one place toward the start
    /// (`MoveDir::Up`) or the end (`MoveDir::Down`) of its strip, and say so,
    /// including when it is already at that end.
    pub(crate) fn move_focused_tab(&mut self, dir: MoveDir) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select an agent to move one of its tabs.");
            return;
        };
        let order = self.session_tab_ids(&session_id);
        if order.len() < 2 {
            self.set_info("This agent has one tab; there is nothing to reorder.");
            return;
        }
        let tab_id = self.focused_tab_id(&session_id);
        let Some(index) = order.iter().position(|id| *id == tab_id) else {
            return;
        };
        let moved = move_in_order(&order, index, dir);
        if moved == order {
            let end = if dir == MoveDir::Up { "left" } else { "right" };
            self.set_info(format!(
                "That tab is already at the {end} end of the strip, so nothing moved."
            ));
            return;
        }
        self.apply_tab_order(&session_id, &tab_id, &order, moved);
    }

    /// Move the view beside a focused tab that another screen closed.
    ///
    /// The close already happened in the shared engine, so the focus memory names
    /// a tab that is gone and would fall back to the first tab. The strip as it
    /// was last drawn still has the tab's neighbours, so the landing is the same
    /// one a close made here gets. Then the drawn strip is remembered.
    pub(super) fn land_beside_a_tab_closed_elsewhere(&mut self) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        let session_id = session_id.as_str();
        let now = self.session_tab_ids(session_id);
        let now = now.as_slice();
        if let Some(drawn) = self.last_tab_strip.clone()
            && drawn.session_id == session_id
            && !now.contains(&drawn.focused)
            && let Some(neighbour) =
                dux_core::agent_tabs::tab_after_close(&drawn.tabs, &drawn.focused)
            && now.iter().any(|id| id == neighbour)
        {
            self.set_focused_tab(session_id, neighbour);
        }
        self.last_tab_strip = Some(DrawnTabStrip {
            session_id: session_id.to_string(),
            tabs: now.to_vec(),
            focused: self.focused_tab_id(session_id),
        });
    }

    /// Hand a new strip order to the engine, then confirm it: the pill moving is
    /// visible, but a small move in a long strip is easy to miss.
    fn apply_tab_order(
        &mut self,
        session_id: &str,
        moved_id: &str,
        before: &[String],
        after: Vec<String>,
    ) {
        if after == before {
            return;
        }
        let total = after.len();
        let place = after.iter().position(|id| id == moved_id).map(|i| i + 1);
        match self.engine.apply(Command::ReorderAgentTabs {
            session_id: session_id.to_string(),
            tab_ids: after,
        }) {
            Ok(reaction) => self.apply_reaction(reaction),
            Err(err) => {
                self.set_error(format!("Could not reorder the tabs. {err}"));
                return;
            }
        }
        let label = self
            .engine
            .tab_prose_label(SessionIdRef::new(session_id), TabIdRef::new(moved_id));
        match (label, place) {
            (Some(label), Some(place)) => self.set_info(format!(
                "Moved the {label} tab to place {place} of {total} in the strip."
            )),
            _ => self.set_info("Reordered the tabs."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::{CenterMode, FocusPane};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
    use dux_core::ids::TabId;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// An app whose one agent has three provider tabs, all stored: the slot tab
    /// `session-1-slot` first, then `tab-2` and `tab-3`.
    fn app_with_three_tabs() -> App {
        let mut app = test_app(default_bindings());
        let session = app.engine.sessions[0].clone();
        app.engine.session_store.create_session(&session).unwrap();
        for (order, id, provider) in [(1, "tab-2", "claude"), (2, "tab-3", "opencode")] {
            let tab = dux_core::model::AgentTab {
                id: id.to_string(),
                session_id: session.id.clone(),
                provider: dux_core::model::ProviderKind::new(provider),
                sort_order: order,
                created_at: chrono::Utc::now(),
            };
            app.engine.session_store.insert_agent_tab(&tab).unwrap();
            app.engine.agent_tabs.insert(TabId::new(id), tab);
        }
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app
    }

    fn strip(app: &App) -> Vec<String> {
        app.session_tab_ids("session-1")
    }

    fn draw(app: &mut App, terminal: &mut Terminal<TestBackend>) {
        terminal.draw(|frame| app.render(frame)).expect("render");
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn middle_of(app: &App, tab: &str) -> (u16, u16) {
        let (_, rect) = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == tab)
            .unwrap_or_else(|| panic!("the strip drew {tab}: {:?}", app.agent_tab_regions));
        (rect.x + rect.width / 2, rect.y + 1)
    }

    #[test]
    fn dragging_a_pill_onto_another_moves_it_there_and_keeps_the_slot_where_it_was() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        draw(&mut app, &mut terminal);
        let (from_x, y) = middle_of(&app, "session-1-slot");
        let (to_x, _) = middle_of(&app, "tab-3");

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), from_x, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to_x, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to_x, y));

        assert_eq!(strip(&app), vec!["tab-2", "tab-3", "session-1-slot"]);
        assert_eq!(
            app.engine.sessions[0].slot_tab_id().as_str(),
            "session-1-slot"
        );
        assert!(app.tab_drag.is_none(), "the release retires the gesture");
    }

    #[test]
    fn a_press_that_stays_on_its_pill_is_a_click_and_moves_nothing() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        draw(&mut app, &mut terminal);
        let (x, y) = middle_of(&app, "tab-2");

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 1, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 1, y));

        assert_eq!(strip(&app), vec!["session-1-slot", "tab-2", "tab-3"]);
        assert_eq!(
            app.focused_tab_id("session-1"),
            "tab-2",
            "the press focused it"
        );
    }

    #[test]
    fn a_release_off_the_strip_drops_nothing() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        draw(&mut app, &mut terminal);
        let (x, y) = middle_of(&app, "tab-2");
        let (to_x, _) = middle_of(&app, "tab-3");

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to_x, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to_x, y + 10));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to_x, y + 10));

        assert_eq!(strip(&app), vec!["session-1-slot", "tab-2", "tab-3"]);
    }

    #[test]
    fn move_tab_commands_step_the_focused_tab_and_confirm_the_new_place() {
        let mut app = app_with_three_tabs();
        app.set_focused_tab("session-1", "tab-2");

        app.move_focused_tab(MoveDir::Up);
        assert_eq!(strip(&app), vec!["tab-2", "session-1-slot", "tab-3"]);
        assert!(app.status.message().contains("place 1 of 3"));

        app.move_focused_tab(MoveDir::Down);
        app.move_focused_tab(MoveDir::Down);
        assert_eq!(strip(&app), vec!["session-1-slot", "tab-3", "tab-2"]);
    }

    #[test]
    fn move_tab_commands_report_an_end_they_are_already_at() {
        let mut app = app_with_three_tabs();
        app.set_focused_tab("session-1", "session-1-slot");
        app.move_focused_tab(MoveDir::Up);
        assert_eq!(strip(&app), vec!["session-1-slot", "tab-2", "tab-3"]);
        assert!(
            app.status.message().contains("already at the left end"),
            "got: {}",
            app.status.message()
        );

        let mut app = app_with_three_tabs();
        app.set_focused_tab("session-1", "tab-3");
        app.move_focused_tab(MoveDir::Down);
        assert_eq!(strip(&app), vec!["session-1-slot", "tab-2", "tab-3"]);
        assert!(
            app.status.message().contains("already at the right end"),
            "got: {}",
            app.status.message()
        );
    }

    #[test]
    fn the_order_survives_a_reload_of_the_stored_tabs() {
        let mut app = app_with_three_tabs();
        app.set_focused_tab("session-1", "tab-3");
        app.move_focused_tab(MoveDir::Up);
        app.move_focused_tab(MoveDir::Up);

        let mut stored = app.engine.session_store.load_agent_tabs().unwrap();
        stored.sort_by_key(|tab| tab.sort_order);
        let stored: Vec<&str> = stored.iter().map(|tab| tab.id.as_str()).collect();
        assert_eq!(stored, vec!["tab-3", "session-1-slot", "tab-2"]);
    }

    #[test]
    fn the_first_tab_wears_the_pin_wherever_it_sits_and_only_with_two_tabs() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        let pin = app.glyphs().pin;
        let pins_in_strip = |terminal: &Terminal<TestBackend>| -> Vec<u16> {
            let buf = terminal.backend().buffer();
            (0..170).filter(|x| buf[(*x, 2)].symbol() == pin).collect()
        };

        draw(&mut app, &mut terminal);
        let (slot_x, _) = middle_of(&app, "session-1-slot");
        let first = pins_in_strip(&terminal);
        assert_eq!(first.len(), 1, "exactly one pill is pinned");
        let (_, slot_rect) = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == "session-1-slot")
            .unwrap();
        assert!(
            slot_rect.x < first[0] && first[0] < slot_rect.x + slot_rect.width,
            "the pin is inside the first tab's pill (slot at {slot_x}): {first:?}"
        );

        // The slot moves to the end of the strip; the pin goes with it.
        app.set_focused_tab("session-1", "session-1-slot");
        app.move_focused_tab(MoveDir::Down);
        app.move_focused_tab(MoveDir::Down);
        draw(&mut app, &mut terminal);
        let (_, last_rect) = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == "session-1-slot")
            .unwrap();
        let moved = pins_in_strip(&terminal);
        assert_eq!(moved.len(), 1);
        assert!(last_rect.x < moved[0] && moved[0] < last_rect.x + last_rect.width);

        // One tab left shows no pin: the strip is only drawn by the always-show
        // setting, and the lone tab is not "the first of several".
        app.engine.agent_tabs.clear();
        app.engine.config.ui.always_show_tab_strip = true;
        draw(&mut app, &mut terminal);
        assert_eq!(pins_in_strip(&terminal), Vec::<u16>::new());
    }

    fn close_focused(app: &mut App, tab: &str) {
        app.set_focused_tab("session-1", tab);
        app.close_focused_tab_prompt();
        app.resolve_confirm_close_tab(crate::app::ConfirmAnswer::Confirm);
    }

    #[test]
    fn closing_the_tab_on_screen_lands_on_its_right_neighbour_or_the_left_when_last() {
        let mut app = app_with_three_tabs();
        close_focused(&mut app, "tab-2");
        assert_eq!(app.focused_tab_id("session-1"), "tab-3");

        let mut app = app_with_three_tabs();
        close_focused(&mut app, "tab-3");
        assert_eq!(
            app.focused_tab_id("session-1"),
            "tab-2",
            "the last tab lands on the one to its left, not on the first"
        );
    }

    #[test]
    fn closing_the_first_tab_lands_beside_it_whoever_takes_the_slot() {
        let mut app = app_with_three_tabs();
        // The slot sits in the middle; the leftmost tab takes it, but the view
        // lands beside the tab that closed.
        app.set_focused_tab("session-1", "session-1-slot");
        app.move_focused_tab(MoveDir::Down);
        assert_eq!(strip(&app), vec!["tab-2", "session-1-slot", "tab-3"]);

        close_focused(&mut app, "session-1-slot");

        assert_eq!(app.engine.sessions[0].slot_tab_id().as_str(), "tab-2");
        assert_eq!(app.focused_tab_id("session-1"), "tab-3");
    }

    #[test]
    fn a_tab_closed_from_another_screen_lands_beside_it_not_on_the_first_tab() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        app.set_focused_tab("session-1", "tab-2");
        draw(&mut app, &mut terminal);

        // The browser or the command line closes it: the row is gone from the
        // shared engine before this surface looks.
        app.engine.close_tab("session-1", "tab-2").unwrap();
        draw(&mut app, &mut terminal);

        assert_eq!(app.focused_tab_id("session-1"), "tab-3");
    }

    #[test]
    fn the_first_tab_closed_from_another_screen_lands_beside_it_too() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        app.set_focused_tab("session-1", "session-1-slot");
        app.move_focused_tab(MoveDir::Down);
        draw(&mut app, &mut terminal);
        assert_eq!(strip(&app), vec!["tab-2", "session-1-slot", "tab-3"]);

        app.engine.close_tab("session-1", "session-1-slot").unwrap();
        draw(&mut app, &mut terminal);

        assert_eq!(app.focused_tab_id("session-1"), "tab-3");
    }

    #[test]
    fn a_tab_that_exits_cleanly_while_on_screen_lands_beside_it() {
        use dux_core::engine::{PrunedPty, PrunedPtyKind};
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        app.set_focused_tab("session-1", "tab-2");
        draw(&mut app, &mut terminal);

        // The row is already gone from the shared engine when this surface
        // captures what it was looking at, as when the prune runs a moment after
        // the exit.
        app.engine.close_tab("session-1", "tab-2").unwrap();
        let context = crate::app::PruneViewContext::capture(&app);
        let exited = PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: "tab-2".to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session(
                "session-1".to_string(),
            )),
            agent_detached: false,
            label: "claude".to_string(),
            tab_closed: true,
            exit_success: Some(true),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
            refused_resume_excerpt: None,
            closed_tab: None,
        };
        app.apply_pruned_agent_tabs(std::slice::from_ref(&exited), &context);

        assert_eq!(app.focused_tab_id("session-1"), "tab-3");
    }

    #[test]
    fn a_focus_change_made_while_the_strip_is_not_drawn_survives_a_close_elsewhere() {
        let mut app = app_with_three_tabs();
        let mut terminal = Terminal::new(TestBackend::new(170, 30)).unwrap();
        app.set_focused_tab("session-1", "tab-2");
        draw(&mut app, &mut terminal);

        // The user moves to the first tab while a fullscreen overlay hides the
        // strip, and then another screen closes the tab that was last drawn.
        app.set_focused_tab("session-1", "session-1-slot");
        app.engine.close_tab("session-1", "tab-2").unwrap();
        draw(&mut app, &mut terminal);

        assert_eq!(app.focused_tab_id("session-1"), "session-1-slot");
    }
}
