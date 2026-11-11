//! An agent's terminal dock: the agent's companion terminals drawn under it in
//! the center pane, with a strip of their own.
//!
//! The center pane shares itself three ways, per agent and in runtime memory
//! only: the agent alone, the split (the agent on top, the dock below, at
//! `[ui] terminal_dock_pct`), or the dock alone. The dock is a pane of its own
//! for focus: Tab stops at it, and the fullscreen key, the close binding and the
//! tab keys act on it while it has focus. Which terminal it shows is remembered
//! per agent as well.

use super::components::wrap_lines::display_width;
use super::render::TabStripItem;
use super::reorder::{MoveDir, move_in_order};
use super::{App, CenterPart, DockLayout, FocusPane, FullscreenOverlay, InputTarget, PillStrip};
use crate::keybindings::Action;
use crate::model::SessionSurface;
use crate::theme::FocusState;
use components::FocusStep;
use dux_core::engine::Command;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Style, Widget};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::components;
use super::components::{CardContent, PaneCardBlock};

/// What stopping a terminal says, from the tab key and the palette alike.
pub(crate) const TERMINAL_STOP_REFUSAL: &str = "A terminal is closed rather than stopped: close it \
     with close-tab, or press Ctrl-C in it to stop what is running there and keep the shell.";

/// The words the layout control shows, in cycle order.
const LAYOUTS: [DockLayout; 3] = [DockLayout::Agent, DockLayout::Split, DockLayout::Term];

/// The layout control's width: its three words, the two separators between
/// them, and a cell of padding on each side.
fn layout_control_width() -> u16 {
    let words: usize = LAYOUTS.iter().map(|layout| layout.label().len()).sum();
    u16::try_from(words + 3 * (LAYOUTS.len() - 1) + 2).unwrap_or(u16::MAX)
}

/// Whether the agent's part carries the layout control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutControl {
    Shown,
    Hidden,
}

/// The fewest rows a part of the split keeps, so a dragged divider never leaves
/// a part with no grid at all.
const MIN_PART_ROWS: u16 = 4;

impl App {
    /// Whether the selected agent's two strips show. Both show, or neither.
    pub(crate) fn strips_shown(&self) -> bool {
        let Some(session) = self.selected_session() else {
            return false;
        };
        dux_core::agent_tabs::strips_shown(
            self.session_tab_ids(&session.id).len(),
            self.dock_terminal_ids(&session.id).len(),
            dux_core::agent_tabs::StripSetting::from(self.engine.config.ui.always_show_tab_strip),
        )
    }

    /// Whether the dock of the selected agent has focus.
    pub(crate) fn dock_has_focus(&self) -> bool {
        self.focus == FocusPane::Center
            && self.focused_surface() == SessionSurface::Terminal
            && self.selected_terminal_row().is_none()
            && self.selected_session().is_some()
            && self.layout_parts().contains(&CenterPart::Dock)
    }

    /// Show `session_id`'s center pane in `layout`, moving focus to a part the
    /// layout draws. A stopped agent's layout lasts while it stays stopped;
    /// starting it returns to the one remembered.
    pub(crate) fn set_dock_layout(&mut self, session_id: &str, layout: DockLayout) {
        let running = self.selected_session().is_some_and(|s| s.id == session_id)
            && self.selected_agent_running();
        if running {
            self.dock_layouts.insert(session_id.to_string(), layout);
            // A layout held while the agent's terminal had the keyboard is
            // replaced by the one asked for now.
            self.stopped_layout = None;
        } else {
            self.stopped_layout = Some((session_id.to_string(), layout));
        }
        match layout {
            DockLayout::Agent => self.session_surface = SessionSurface::Agent,
            DockLayout::Term => self.session_surface = SessionSurface::Terminal,
            DockLayout::Split => {}
        }
        if self.input_target == InputTarget::Terminal
            && self.session_surface != SessionSurface::Terminal
            || self.input_target == InputTarget::Agent
                && self.session_surface != SessionSurface::Agent
        {
            self.input_target = InputTarget::None;
        }
        self.mark_frame_dirty();
    }

    /// The layout key: agent, split, terminal, agent. An agent with no
    /// terminals has nothing to share the pane with, which is said rather than
    /// done silently.
    pub(crate) fn cycle_terminal_layout(&mut self) {
        if let Some(terminal) = self.selected_terminal_row() {
            let label = self
                .engine
                .companion_terminals
                .get(&terminal)
                .map_or_else(|| "This terminal".to_string(), |t| t.label.clone());
            self.set_info(format!(
                "{label} is a terminal of its own row, with no agent to share the center pane \
                 with."
            ));
            return;
        }
        let Some(session) = self.selected_session() else {
            self.set_info("Select an agent first, then change how it shares the center pane.");
            return;
        };
        let session_id = session.id.clone();
        let name = session.display_label();
        if self.dock_terminal_ids(&session_id).is_empty() {
            let key = self.bindings.label_for(Action::ShowTerminal);
            self.set_info(format!("{name} has no terminals; press {key} to open one."));
            return;
        }
        let next = self.selected_dock_layout().next();
        self.set_dock_layout(&session_id, next);
    }

    /// Show `terminal_id` in the selected agent's dock and remember it.
    pub(crate) fn show_dock_terminal(&mut self, terminal_id: &str) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        if self.active_terminal_id.as_deref() != Some(terminal_id) {
            self.active_terminal_id = Some(terminal_id.to_string());
            self.part_snapshots.dock.last_id = None;
        }
        self.dock_active.insert(session_id, terminal_id.to_string());
        self.mark_frame_dirty();
    }

    /// The tab keys on the dock: the next or previous pill, wrapping.
    pub(crate) fn focus_dock_relative(&mut self, step: FocusStep) {
        let ids = self.selected_dock_terminal_ids();
        if ids.len() < 2 {
            return;
        }
        let current = self.active_terminal_id.clone().unwrap_or_default();
        let index = ids.iter().position(|id| *id == current).unwrap_or(0);
        let next = if step == FocusStep::Forward {
            (index + 1) % ids.len()
        } else {
            (index + ids.len() - 1) % ids.len()
        };
        self.show_dock_terminal(&ids[next]);
    }

    /// Select-tab N on the dock: the Nth pill, when there is one.
    pub(crate) fn focus_dock_index(&mut self, n: usize) {
        if let Some(id) = self.selected_dock_terminal_ids().get(n).cloned() {
            self.show_dock_terminal(&id);
        }
    }

    /// Move the dock's active terminal one place toward the start
    /// (`MoveDir::Up`) or the end (`MoveDir::Down`) of its strip.
    pub(crate) fn move_dock_terminal(&mut self, dir: MoveDir) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        let order = self.dock_terminal_ids(&session_id);
        if order.len() < 2 {
            self.set_info("This agent has one terminal; there is nothing to reorder.");
            return;
        }
        let Some(active) = self.active_terminal_id.clone() else {
            return;
        };
        let Some(index) = order.iter().position(|id| *id == active) else {
            return;
        };
        let moved = move_in_order(&order, index, dir);
        if moved == order {
            let end = if dir == MoveDir::Up { "left" } else { "right" };
            self.set_info(format!(
                "That terminal is already at the {end} end of the dock, so nothing moved."
            ));
            return;
        }
        self.apply_dock_order(&active, &order, &moved);
    }

    /// Hand a new dock order to the engine and confirm it. Only this agent's
    /// terminals change places, and only among the places they already hold,
    /// so no other dock and no row of the list moves.
    pub(crate) fn apply_dock_order(&mut self, moved_id: &str, before: &[String], after: &[String]) {
        if after == before {
            return;
        }
        let mut all: Vec<(String, u64)> = self
            .engine
            .companion_terminals
            .iter()
            .map(|(id, terminal)| (id.clone(), terminal.sort_order))
            .collect();
        all.sort_by_key(|(_, order)| *order);
        let mut replacements = after.iter();
        let terminal_ids: Vec<String> = all
            .into_iter()
            .map(|(id, _)| {
                if before.contains(&id) {
                    replacements.next().cloned().unwrap_or(id)
                } else {
                    id
                }
            })
            .collect();
        if let Err(err) = self
            .engine
            .apply(Command::ReorderTerminals { terminal_ids })
        {
            self.set_error(format!("Could not reorder the terminals. {err}"));
            return;
        }
        let label = self
            .engine
            .companion_terminals
            .get(moved_id)
            .map(|terminal| terminal.label.clone());
        let place = after.iter().position(|id| id == moved_id).map(|i| i + 1);
        match (label, place) {
            (Some(label), Some(place)) => self.set_info(format!(
                "Moved {label} to place {place} of {} in the terminal dock.",
                after.len()
            )),
            _ => self.set_info("Reordered the terminal dock."),
        }
    }

    /// One pill per dock terminal, named as the list names a terminal row: the
    /// program running in it, numbered among same-named siblings, or its own
    /// name when it is idle.
    fn dock_strip_items(&self, ids: &[String]) -> Vec<TabStripItem> {
        let tone = self.theme.session_active;
        ids.iter()
            .filter_map(|id| {
                let terminal = self.engine.companion_terminals.get(id)?;
                let siblings: Vec<Option<&str>> = ids
                    .iter()
                    .filter(|other| *other != id)
                    .filter_map(|other| self.engine.companion_terminals.get(other))
                    .map(|other| other.foreground_cmd.as_deref())
                    .collect();
                let label = dux_core::terminal_title::terminal_foreground_display(
                    terminal.foreground_cmd.as_deref(),
                )
                .map_or_else(
                    || terminal.label.clone(),
                    |_| {
                        dux_core::terminal_title::terminal_title(
                            &terminal.label,
                            terminal.foreground_cmd.as_deref(),
                            &siblings,
                        )
                    },
                );
                Some(TabStripItem {
                    id: id.clone(),
                    label: label.to_lowercase(),
                    working: self.engine.terminal_is_working(id),
                    needs_attention: false,
                    glyph_tone: tone,
                    pinned: false,
                })
            })
            .collect()
    }

    /// The selected agent's center pane: the agent, its dock, or both, as its
    /// layout says, with the layout control on the dock's strip, the agent's
    /// strip when the dock is hidden, or a title row when the strips are.
    pub(super) fn render_agent_stack(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focused: FocusState,
    ) {
        self.mouse_layout.center_stack = Some(area);
        // Only a layout that draws the agent's strip publishes its pills again.
        self.agent_tab_regions.clear();
        let dock = self.selected_dock_terminal_ids();
        let layout = self.selected_dock_layout();
        let part_focus = |app: &Self, part: CenterPart| {
            if focused == FocusState::Focused && app.focused_part() == part {
                FocusState::Focused
            } else {
                FocusState::Unfocused
            }
        };
        match layout {
            DockLayout::Agent => {
                let control = if dock.is_empty() {
                    LayoutControl::Hidden
                } else {
                    LayoutControl::Shown
                };
                let focus = part_focus(self, CenterPart::Main);
                self.render_agent_part(frame, area, focus, control);
                self.last_dock_strip = Some(dock);
            }
            DockLayout::Split => {
                let dock_rows = self.dock_rows(area.height);
                let [top, bottom] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(dock_rows)])
                    .areas(area);
                self.mouse_layout.dock_divider = Some(Rect {
                    height: 1,
                    ..bottom
                });
                let agent_focus = part_focus(self, CenterPart::Main);
                self.render_agent_part(frame, top, agent_focus, LayoutControl::Hidden);
                let dock_focus = part_focus(self, CenterPart::Dock);
                self.render_dock_part(frame, bottom, dock_focus, &dock);
            }
            DockLayout::Term => {
                let mut rest = area;
                if !self.selected_agent_running() && area.height > 6 {
                    let [band, below] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(1)])
                        .areas(area);
                    self.render_stopped_band(frame, band);
                    rest = below;
                }
                let dock_focus = part_focus(self, CenterPart::Dock);
                self.render_dock_part(frame, rest, dock_focus, &dock);
            }
        }
        self.render_stack_takeover_card(frame);
    }

    /// Whether the frame is drawing an agent's stack in the center pane, where
    /// one take-over card covers every part at once.
    pub(super) fn drawing_agent_stack(&self) -> bool {
        self.mouse_layout.center_stack.is_some()
            && matches!(self.fullscreen_overlay, FullscreenOverlay::None)
    }

    /// Whether a take-over card is over the center pane: over the one part a
    /// fullscreen view or a terminal's row shows, or over any part of an
    /// agent's stack, which all belong to one row with one driver.
    pub(crate) fn card_covers_center(&self) -> bool {
        self.covering_card().is_some()
    }

    /// The take-over card over the center pane and a pty of the row it
    /// covers. Over an agent's stack that is any drawn part with a pty, so a
    /// dormant tab in the focused part does not leave the row unclaimable.
    pub(crate) fn covering_card(
        &self,
    ) -> Option<(crate::app::pty_ownership::PtyTakeoverCard, String)> {
        let shows_stack = matches!(self.fullscreen_overlay, FullscreenOverlay::None)
            && matches!(self.center_mode, super::CenterMode::Agent)
            && self.selected_terminal_row().is_none()
            && self.selected_session().is_some();
        let parts = if shows_stack {
            self.layout_parts()
        } else {
            vec![self.focused_part()]
        };
        parts.into_iter().find_map(|part| {
            let card = self.part_takeover_card(part)?;
            Some((card, self.part_pty_id(part)?))
        })
    }

    /// The card over the stack, when the row is not this surface's: one card
    /// across every grid the layout draws, the dock's strip between them
    /// included, while the agent's strip above and the hint lanes stay visible
    /// as they do over a single pane.
    fn render_stack_takeover_card(&mut self, frame: &mut Frame<'_>) {
        self.mouse_layout.takeover_button = None;
        let grids = [self.mouse_layout.focused_term, self.mouse_layout.other_term];
        let Some(area) = grids.into_iter().flatten().reduce(Rect::union) else {
            return;
        };
        let card = self
            .layout_parts()
            .into_iter()
            .find_map(|part| self.part_takeover_card(part));
        if let Some(card) = card {
            if self.part_scroll_mode_active(CenterPart::Main) {
                self.reset_part_scrollback(CenterPart::Main);
            }
            if self.part_scroll_mode_active(CenterPart::Dock) {
                self.reset_part_scrollback(CenterPart::Dock);
            }
            self.mouse_layout.card_cover = Some(area);
            self.render_takeover_card(frame, area, &card);
        }
    }

    /// How many rows the dock takes in the split of a center pane `height`
    /// rows tall.
    fn dock_rows(&self, height: u16) -> u16 {
        let pct = dux_core::config_effective::effective_pane_height_pct(self.terminal_dock_pct);
        let rows = u16::try_from(u32::from(height) * u32::from(pct) / 100).unwrap_or(height);
        let floor = MIN_PART_ROWS.min(height / 2);
        rows.clamp(floor, height.saturating_sub(floor))
    }

    /// The agent's part: its strip when the strips show, then its terminal.
    fn render_agent_part(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focus: FocusState,
        control: LayoutControl,
    ) {
        let reserve = match control {
            LayoutControl::Shown => layout_control_width(),
            LayoutControl::Hidden => 0,
        };
        let (term_area, strip) = self.render_agent_tab_strip_if_needed(frame, area, reserve);
        let title = self.center_pane_agent_title();
        self.render_agent_terminal(frame, (CenterPart::Main, term_area), &title, focus);
        if control == LayoutControl::Shown {
            self.paint_layout_control(
                frame,
                strip.map_or(term_area, |strip| Rect {
                    y: strip.y + 1,
                    ..strip
                }),
            );
        }
    }

    /// The dock's part: its strip when the strips show, then the active
    /// terminal, with the layout control on the strip's row or, with the strips
    /// hidden, on the terminal block's title row.
    fn render_dock_part(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        focus: FocusState,
        dock: &[String],
    ) {
        self.last_dock_strip = Some(dock.to_vec());
        let shown = self.strips_shown() && area.height >= 6 && area.width >= 12;
        let term_area = if shown {
            let [strip, below] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)])
                .areas(area);
            super::render::fill_blank(
                frame.buffer_mut(),
                strip,
                Style::default().bg(self.theme.app_bg),
            );
            let items = self.dock_strip_items(dock);
            let focused = self.active_terminal_id.clone().unwrap_or_default();
            let pills = Rect {
                width: strip.width.saturating_sub(layout_control_width()),
                ..strip
            };
            self.mouse_layout.dock_pills =
                self.paint_pill_strip(frame, pills, &items, &focused, PillStrip::Dock);
            self.paint_layout_control(
                frame,
                Rect {
                    y: strip.y + 1,
                    ..strip
                },
            );
            below
        } else {
            area
        };
        let title = self.dock_caption();
        self.render_agent_terminal(frame, (CenterPart::Dock, term_area), &title, focus);
        if !shown {
            self.paint_layout_control(frame, term_area);
        }
    }

    /// Paint the `agent | split | term` control at the right end of the row
    /// `row` starts on, the current layout in the focused title style, and
    /// publish each word's cell for a click.
    fn paint_layout_control(&mut self, frame: &mut Frame<'_>, row: Rect) {
        let width = layout_control_width();
        if row.width < width + 2 || row.height == 0 {
            return;
        }
        let current = self.selected_dock_layout();
        let x0 = row.x + row.width - width - 1;
        let mut x = x0 + 1;
        let separator = Style::default().fg(self.theme.hint_dim_desc_fg);
        let mut spans = vec![Span::raw(" ")];
        for (index, layout) in LAYOUTS.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" | ", separator));
                x += 3;
            }
            let label = layout.label();
            let style = if *layout == current {
                self.theme.title_style(FocusState::Focused)
            } else {
                Style::default().fg(self.theme.hint_desc_fg)
            };
            spans.push(Span::styled(label, style));
            let cells = u16::try_from(display_width(label)).unwrap_or(0);
            self.mouse_layout
                .layout_control
                .push((*layout, Rect::new(x, row.y, cells, 1)));
            x += cells;
        }
        spans.push(Span::raw(" "));
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(self.theme.app_bg))
            .render(Rect::new(x0, row.y, width, 1), frame.buffer_mut());
    }

    /// The band that stands in for a stopped agent's strip while its terminal
    /// fills the pane: the agent is not running, how its last run ended when it
    /// ended badly, and Start.
    fn render_stopped_band(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let session_id = session.id.clone();
        let provider = self.focused_tab_provider(session).as_str().to_string();
        let slot = self
            .engine
            .slot_tab_id_of(dux_core::ids::SessionIdRef::new(&session_id))
            .to_string();
        let ending = self.engine.tab_run_verdict(&slot).map(|verdict| {
            dux_core::tab_verdict::ending_sentence(&verdict.ending, verdict.ended_seconds_ago())
        });
        let start = self.bindings.label_for(Action::ReconnectAgent);
        let layout_key = self.bindings.label_for(Action::CycleTerminalLayout);
        let label = "Start";
        let button_width = u16::try_from(display_width(label)).unwrap_or(0) + 6;
        let [text_area, button_area] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(button_width)])
            .areas(area);
        let text = Style::default().fg(self.theme.hint_desc_fg);
        let badge = |key: &str| -> Vec<Span<'static>> {
            self.theme
                .dim_key_badge_default(key)
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect()
        };
        let mut spans = vec![Span::styled(format!(" {provider} isn't running."), text)];
        let crashed = ending.is_some();
        if let Some(ending) = ending {
            spans.push(Span::styled(format!(" {ending}"), text));
        }
        spans.push(Span::styled(" Press ", text));
        spans.extend(badge(&start));
        spans.push(Span::styled(" to start it.", text));
        // The run's last output is on the agent's own card, which the next
        // layout shows; a band has no room for it.
        if crashed && !layout_key.is_empty() {
            spans.push(Span::styled(" Press ", text));
            spans.extend(badge(&layout_key));
            spans.push(Span::styled(" to see its last output.", text));
        }
        Paragraph::new(Line::from(spans)).render(
            Rect {
                y: text_area.y + text_area.height / 2,
                height: 1,
                ..text_area
            },
            frame.buffer_mut(),
        );
        let state = components::button::button_state_for(
            components::ButtonPressedTarget::DormantTabCard,
            self.dormant_tab_press,
            components::ButtonState::Normal,
        );
        components::Button::new(label)
            .state(state)
            .kind(components::ButtonKind::Confirm)
            .render(frame, button_area, &self.theme);
        self.mouse_layout.dormant_tab_button = Some(button_area);
    }

    /// Start a stopped agent from its band or its short card: its first tab,
    /// which is what selecting an agent starts on the web, whatever tab was on
    /// screen. Once it runs, its remembered layout comes back.
    pub(crate) fn start_stopped_agent(&mut self) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        let slot = self
            .engine
            .slot_tab_id_of(dux_core::ids::SessionIdRef::new(&session_id))
            .to_string();
        self.set_focused_tab(&session_id, &slot);
        self.launch_focused_extra_tab(&session_id, &slot, super::LaunchView::Minimized);
    }

    /// Whether the selected agent is stopped while its dock has terminals, the
    /// state its band and its short card are drawn for.
    pub(crate) fn selected_agent_stopped_beside_dock(&self) -> bool {
        !self.selected_agent_running() && !self.selected_dock_terminal_ids().is_empty()
    }

    /// The short card a stopped agent shows while it shares the pane with its
    /// terminals: one line, the key that starts it and Start, without the logo
    /// and its tip.
    pub(super) fn render_short_agent_card(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let provider = self.focused_tab_provider(session).as_str().to_string();
        let blocks = [
            PaneCardBlock::new(
                0,
                CardContent::Prose(format!(
                    "{provider} isn't running. Start picks up its last conversation."
                )),
            ),
            PaneCardBlock::new(
                1,
                CardContent::KeyHint {
                    before: "Press ".to_string(),
                    key: self.bindings.label_for(Action::ReconnectAgent),
                    after: " to start it.".to_string(),
                },
            ),
            PaneCardBlock::new(
                0,
                CardContent::Button {
                    label: "Start".to_string(),
                    state: components::button::button_state_for(
                        components::ButtonPressedTarget::DormantTabCard,
                        self.dormant_tab_press,
                        components::ButtonState::Normal,
                    ),
                },
            ),
        ];
        self.mouse_layout.dormant_tab_button = self.render_pane_card(frame, area, "", 40, &blocks);
    }

    /// Whether `part` is drawn grey and faded: the part without focus in the
    /// split, while focus is in the center pane. A minimized part is not drawn
    /// at all, and with focus elsewhere neither part is where typing goes, so
    /// both stay in full color.
    pub(crate) fn part_is_greyed(&self, part: CenterPart) -> bool {
        self.focus == FocusPane::Center
            && matches!(self.fullscreen_overlay, FullscreenOverlay::None)
            && self.selected_terminal_row().is_none()
            && self.selected_dock_layout() == DockLayout::Split
            && part != self.focused_part()
    }

    /// The caption over a shown terminal, with how many browsers have that
    /// terminal open, as the agent's caption counts the tab it shows.
    pub(crate) fn dock_caption(&self) -> String {
        let id = self.part_pty_id(CenterPart::Dock);
        // Named, so with the strips hidden the caption still says which one.
        let label = id
            .as_ref()
            .and_then(|id| self.engine.companion_terminals.get(id))
            .map_or_else(|| "Terminal".to_string(), |t| t.label.clone());
        let remote = id
            .and_then(|id| super::render::remote_viewers_segment(self.pty_browser_count(&id)))
            .map(|segment| format!(" · {segment}"))
            .unwrap_or_default();
        format!(" {label}{remote} ")
    }

    /// A press on the dock's chrome: a word of the layout control, or a dock
    /// pill, which shows that terminal, gives the dock focus and arms a drag.
    pub(super) fn handle_dock_chrome_press(&mut self, column: u16, row: u16) -> bool {
        let hit = |rect: &Rect| super::input::contains_point(*rect, column, row);
        if let Some(layout) = self
            .mouse_layout
            .layout_control
            .iter()
            .find(|(_, rect)| hit(rect))
            .map(|(layout, _)| *layout)
        {
            if let Some(session_id) = self.selected_session().map(|s| s.id.clone()) {
                self.focus = FocusPane::Center;
                self.set_dock_layout(&session_id, layout);
            }
            return true;
        }
        if let Some(id) = self
            .mouse_layout
            .dock_pills
            .iter()
            .find(|(_, rect)| hit(rect))
            .map(|(id, _)| id.clone())
        {
            self.focus = FocusPane::Center;
            self.session_surface = SessionSurface::Terminal;
            self.show_dock_terminal(&id);
            self.arm_strip_drag(PillStrip::Dock, &id);
            return true;
        }
        false
    }

    /// A press on the part of the split without focus gives it focus first, so
    /// the press goes on to act on the part under the pointer.
    pub(super) fn focus_part_under(&mut self, column: u16, row: u16) {
        let Some(other) = self.mouse_layout.other_term else {
            return;
        };
        if !super::input::contains_point(other, column, row) {
            return;
        }
        let part = match self.focused_part() {
            CenterPart::Main => CenterPart::Dock,
            CenterPart::Dock => CenterPart::Main,
        };
        self.session_surface = Self::part_surface(part);
        self.input_target = InputTarget::None;
        std::mem::swap(
            &mut self.mouse_layout.focused_term,
            &mut self.mouse_layout.other_term,
        );
        self.mark_frame_dirty();
    }

    /// A wheel over the part of the split without focus scrolls that part's
    /// own history, without moving focus to it. Returns whether the wheel was
    /// over it.
    pub(super) fn scroll_part_under(
        &mut self,
        column: u16,
        row: u16,
        direction: dux_core::pty::ScrollDirection,
    ) -> bool {
        let Some(other) = self.mouse_layout.other_term else {
            return false;
        };
        if !super::input::contains_point(other, column, row) {
            return false;
        }
        let part = match self.focused_part() {
            CenterPart::Main => CenterPart::Dock,
            CenterPart::Dock => CenterPart::Main,
        };
        let (Some(id), Some(client)) = (self.part_pty_id(part), self.part_client(part)) else {
            return true;
        };
        client.scroll(direction, super::input::MOUSE_WHEEL_LINES);
        if client.scrollback_offset() > 0 {
            self.scroll_mode.insert(id);
        } else {
            self.scroll_mode.remove(&id);
        }
        self.mark_frame_dirty();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::{CenterMode, LeftItem, PromptState};
    use crate::pty::PtyClient;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use dux_core::ids::{TabId, TabIdRef};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sleeper() -> PtyClient {
        PtyClient::spawn(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            std::path::Path::new("."),
            10,
            10,
            100,
        )
        .expect("spawn pty")
    }

    /// The first agent selected, focus on the center pane, and `terminals`
    /// terminals in its dock; a running agent's first tab has a live process.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Agent {
        Running,
        Stopped,
    }

    fn app_with_a_dock(terminals: usize, agent: Agent) -> App {
        let mut app = test_app(default_bindings());
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = Vec::new();
        let session_id = app.engine.sessions[0].id.clone();
        if agent == Agent::Running {
            let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
            app.engine.providers.insert(TabId::new(slot), sleeper());
        }
        for _ in 0..terminals {
            app.engine
                .create_companion_terminal(&session_id, 24, 80)
                .expect("companion terminal");
        }
        app.rebuild_left_items();
        let row = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(0)))
            .expect("the agent's row");
        app.selected_left = row;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.reconcile_center_target();
        app
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(140, 44)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        terminal
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn dock_ids(app: &App) -> Vec<String> {
        app.selected_dock_terminal_ids()
    }

    #[test]
    fn an_agent_with_terminals_opens_in_the_split_and_draws_both_parts() {
        let mut app = app_with_a_dock(1, Agent::Running);
        assert_eq!(app.selected_dock_layout(), DockLayout::Split);
        let terminal = draw(&mut app);
        let text = screen(&terminal);
        assert!(text.contains("agent | split | term"), "{text}");
        let drawn: Vec<String> = app.drawn_ptys.iter().map(|t| t.id.clone()).collect();
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        let dock = dock_ids(&app)[0].clone();
        assert!(drawn.contains(&slot) && drawn.contains(&dock), "{drawn:?}");
        assert!(app.mouse_layout.other_term.is_some());
    }

    /// Two parts drawn every frame each get their size once: a geometry sent is
    /// not sent again, which is what keeps two children from being resized
    /// on every frame.
    #[test]
    fn a_frame_drawing_both_parts_resizes_each_once_and_not_again() {
        let mut app = app_with_a_dock(1, Agent::Running);
        draw(&mut app);
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        let dock = dock_ids(&app)[0].clone();
        assert!(app.pty_sizes.contains_key(&slot) && app.pty_sizes.contains_key(&dock));
        // Something else re-grids both children; an unchanged frame sends
        // nothing, so both keep that grid.
        app.engine.providers[TabIdRef::new(&slot)]
            .resize(5, 5)
            .unwrap();
        app.engine.companion_terminals[&dock]
            .client
            .resize(5, 5)
            .unwrap();
        draw(&mut app);
        assert_eq!(
            app.engine.providers[TabIdRef::new(&slot)].grid_size(),
            Some((5, 5))
        );
        assert_eq!(
            app.engine.companion_terminals[&dock].client.grid_size(),
            Some((5, 5))
        );
    }

    #[test]
    fn the_layout_key_cycles_agent_split_and_term() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let ctrl_t = key(KeyCode::Char('t'), KeyModifiers::CONTROL);
        app.handle_key(ctrl_t).unwrap();
        assert_eq!(app.selected_dock_layout(), DockLayout::Term);
        assert_eq!(app.focused_part(), CenterPart::Dock);
        app.handle_key(ctrl_t).unwrap();
        assert_eq!(app.selected_dock_layout(), DockLayout::Agent);
        assert_eq!(app.focused_part(), CenterPart::Main);
        draw(&mut app);
        let drawn: Vec<String> = app.drawn_ptys.iter().map(|t| t.id.clone()).collect();
        assert!(
            !drawn.contains(&dock_ids(&app)[0]),
            "the dock is not drawn: {drawn:?}"
        );
        app.handle_key(ctrl_t).unwrap();
        assert_eq!(app.selected_dock_layout(), DockLayout::Split);
    }

    #[test]
    fn the_layout_key_says_so_when_the_agent_has_no_terminals() {
        let mut app = app_with_a_dock(0, Agent::Running);
        app.cycle_terminal_layout();
        let (_, message) = app.status.most_recent_tui().expect("a status");
        let open = app.bindings.label_for(Action::ShowTerminal);
        assert!(
            message.contains("has no terminals") && message.contains(&open),
            "{message}"
        );
    }

    #[test]
    fn tab_stops_at_the_dock_only_while_it_is_drawn() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.focus = FocusPane::Left;
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(tab).unwrap();
        assert_eq!(
            (app.focus, app.focused_part()),
            (FocusPane::Center, CenterPart::Main)
        );
        app.handle_key(tab).unwrap();
        assert_eq!(
            (app.focus, app.focused_part()),
            (FocusPane::Center, CenterPart::Dock)
        );
        app.handle_key(tab).unwrap();
        assert_eq!(app.focus, FocusPane::Files);

        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Agent);
        app.focus = FocusPane::Left;
        app.handle_key(tab).unwrap();
        app.handle_key(tab).unwrap();
        assert_eq!(app.focus, FocusPane::Files, "no dock drawn, no stop at it");
    }

    #[test]
    fn the_close_key_closes_the_docked_terminal_with_the_dock_focused() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let ctrl_d = key(KeyCode::Char('d'), KeyModifiers::CONTROL);
        app.session_surface = SessionSurface::Terminal;
        app.handle_key(ctrl_d).unwrap();
        assert!(
            matches!(&app.prompt, PromptState::ConfirmDeleteTerminal { terminal_id, .. } if *terminal_id == dock_ids(&app)[0]),
            "{:?}",
            app.prompt
        );
        app.prompt = PromptState::None;
        app.session_surface = SessionSurface::Agent;
        app.handle_key(ctrl_d).unwrap();
        assert!(
            matches!(app.prompt, PromptState::ConfirmDeleteAgent { .. }),
            "with the agent focused the key deletes the agent: {:?}",
            app.prompt
        );
    }

    #[test]
    fn close_terminal_refuses_with_nothing_to_close() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.focus = FocusPane::Left;
        app.close_terminal_from_palette();
        assert!(matches!(app.prompt, PromptState::None));
        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert!(
            message.contains("Focus an agent's terminal dock"),
            "{message}"
        );
    }

    #[test]
    fn the_tab_keys_walk_the_dock_with_the_dock_focused_and_leave_the_tab_alone() {
        let mut app = app_with_a_dock(3, Agent::Running);
        app.session_surface = SessionSurface::Terminal;
        let ids = dock_ids(&app);
        let tab_before = app.focused_tab_id(&app.engine.sessions[0].id.clone());
        assert_eq!(app.active_terminal_id.as_ref(), Some(&ids[0]));
        app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(app.active_terminal_id.as_ref(), Some(&ids[1]));
        // Back past the first pill wraps to the last.
        app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL))
            .unwrap();
        app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(app.active_terminal_id.as_ref(), Some(&ids[2]));
        assert_eq!(
            app.focused_tab_id(&app.engine.sessions[0].id.clone()),
            tab_before
        );
    }

    #[test]
    fn move_tab_right_with_the_dock_focused_reorders_the_dock() {
        let mut app = app_with_a_dock(2, Agent::Running);
        app.session_surface = SessionSurface::Terminal;
        let ids = dock_ids(&app);
        app.execute_command("move-tab-right").unwrap();
        assert_eq!(dock_ids(&app), vec![ids[1].clone(), ids[0].clone()]);
    }

    #[test]
    fn the_part_without_focus_is_greyed_only_in_the_split_with_focus_in_the_center() {
        let mut app = app_with_a_dock(1, Agent::Running);
        assert!(app.part_is_greyed(CenterPart::Dock));
        assert!(!app.part_is_greyed(CenterPart::Main));
        app.focus = FocusPane::Left;
        assert!(
            !app.part_is_greyed(CenterPart::Dock),
            "focus elsewhere: full colour"
        );
        app.focus = FocusPane::Center;
        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Term);
        assert!(!app.part_is_greyed(CenterPart::Main) && !app.part_is_greyed(CenterPart::Dock));
    }

    #[test]
    fn a_stopped_agent_with_a_running_terminal_opens_on_it_with_the_band() {
        let mut app = app_with_a_dock(1, Agent::Stopped);
        assert_eq!(app.selected_dock_layout(), DockLayout::Term);
        let terminal = draw(&mut app);
        let text = screen(&terminal);
        assert!(
            text.contains("isn't running") && text.contains("Start"),
            "{text}"
        );
        assert!(app.mouse_layout.dormant_tab_button.is_some());
        let drawn: Vec<String> = app.drawn_ptys.iter().map(|t| t.id.clone()).collect();
        assert_eq!(drawn, dock_ids(&app), "no agent pane is drawn");
    }

    #[test]
    fn a_click_on_the_layout_control_switches_the_layout() {
        let mut app = app_with_a_dock(1, Agent::Running);
        draw(&mut app);
        let (_, rect) = *app
            .mouse_layout
            .layout_control
            .iter()
            .find(|(layout, _)| *layout == DockLayout::Term)
            .expect("the control is drawn");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_dock_layout(), DockLayout::Term);
    }

    #[test]
    fn dragging_the_divider_resizes_the_dock_and_an_edge_shows_one_part() {
        let mut app = app_with_a_dock(1, Agent::Running);
        draw(&mut app);
        let stack = app.mouse_layout.center_stack.expect("the stack");
        let divider = app.mouse_layout.dock_divider.expect("the divider");
        let press = |kind, row| MouseEvent {
            kind,
            column: divider.x + 4,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(press(MouseEventKind::Down(MouseButton::Left), divider.y));
        app.handle_mouse(press(
            MouseEventKind::Drag(MouseButton::Left),
            stack.y + stack.height / 4,
        ));
        app.handle_mouse(press(
            MouseEventKind::Up(MouseButton::Left),
            stack.y + stack.height / 4,
        ));
        assert!(app.terminal_dock_pct > 50, "{}", app.terminal_dock_pct);
        assert_eq!(
            app.engine.config.ui.terminal_dock_pct,
            app.terminal_dock_pct
        );

        draw(&mut app);
        let divider = app.mouse_layout.dock_divider.expect("the divider");
        app.handle_mouse(press(MouseEventKind::Down(MouseButton::Left), divider.y));
        app.handle_mouse(press(
            MouseEventKind::Drag(MouseButton::Left),
            stack.bottom() - 1,
        ));
        app.handle_mouse(press(
            MouseEventKind::Up(MouseButton::Left),
            stack.bottom() - 1,
        ));
        assert_eq!(app.selected_dock_layout(), DockLayout::Agent);
    }

    #[test]
    fn the_fullscreen_key_on_the_dock_shows_the_terminal_and_launches_nothing() {
        let mut app = app_with_a_dock(1, Agent::Stopped);
        app.handle_key(key(KeyCode::Char('g'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert!(
            app.engine.providers.is_empty(),
            "the stopped agent was not started"
        );
    }

    #[test]
    fn output_on_the_part_without_focus_draws_the_next_frame() {
        let mut app = app_with_a_dock(1, Agent::Running);
        draw(&mut app);
        app.refresh_part_snapshot(CenterPart::Dock);
        app.redraw.dirty = false;
        let dock = dock_ids(&app)[0].clone();
        app.engine.companion_terminals[&dock]
            .client
            .write_bytes(b"hello\n")
            .unwrap();
        for _ in 0..200 {
            app.note_visible_pty_output();
            if app.redraw.dirty {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("output on the unfocused dock never marked the frame");
    }

    #[test]
    fn the_one_list_holds_standalone_terminals_but_not_an_agents_own() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.engine
            .create_standalone_terminal(24, 80)
            .expect("standalone");
        app.rebuild_left_items();
        assert_eq!(
            app.left_terminals.len(),
            1,
            "only the standalone terminal is a row"
        );
        let terminal = draw(&mut app);
        assert!(
            screen(&terminal).contains("Agents & Terminals (1 + 1)"),
            "{}",
            screen(&terminal)
        );
        let row = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Terminal(_)))
            .expect("the terminal's row");
        app.select_left_agent_item(row);
        assert_eq!(app.layout_parts(), vec![CenterPart::Dock]);
        assert_eq!(
            app.selected_terminal_surface_id(),
            app.selected_terminal_row()
        );
    }

    /// One selection and one pending press live at a time, each on the part it
    /// was made in: switching the dock to another terminal retires only what
    /// was stamped on the dock.
    #[test]
    fn switching_the_dock_terminal_keeps_a_selection_made_in_the_agent() {
        let mut app = app_with_a_dock(2, Agent::Running);
        draw(&mut app);
        // The agent's grid has been read, as it has by the time anything on it
        // can be selected.
        app.refresh_part_snapshot(CenterPart::Main);
        app.terminal_selection = Some(crate::app::TerminalSelection {
            anchor: crate::app::TermGridPos { row: 0, col: 0 },
            end: crate::app::TermGridPos { row: 0, col: 3 },
            dragging: false,
            origin: app.part_selection_origin(CenterPart::Main),
            part: CenterPart::Main,
        });
        let second = dock_ids(&app)[1].clone();
        app.show_dock_terminal(&second);
        app.refresh_part_snapshot(CenterPart::Dock);
        draw(&mut app);
        assert!(
            app.terminal_selection.is_some(),
            "the agent's selection survives"
        );

        app.terminal_selection = Some(crate::app::TerminalSelection {
            part: CenterPart::Dock,
            ..app.terminal_selection.clone().expect("selection")
        });
        let first = dock_ids(&app)[0].clone();
        app.show_dock_terminal(&first);
        app.refresh_part_snapshot(CenterPart::Dock);
        assert!(
            app.terminal_selection.is_none(),
            "the dock's own switch retires it"
        );
    }

    /// A wheel over the part without focus scrolls that part's history, and
    /// only that part enters scroll mode.
    #[test]
    fn a_wheel_over_the_unfocused_dock_scrolls_the_dock_only() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let dock = dock_ids(&app)[0].clone();
        let lines = "line\n".repeat(200);
        app.engine.companion_terminals[&dock]
            .client
            .write_bytes(lines.as_bytes())
            .unwrap();
        for _ in 0..200 {
            draw(&mut app);
            if app.part_snapshots.dock.buf.scrollback_total > 20 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let over = app.mouse_layout.other_term.expect("the dock's grid");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: over.x + 1,
            row: over.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.scroll_mode.contains(&dock), "the dock scrolled back");
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        assert!(!app.scroll_mode.contains(&slot));
        assert_eq!(
            app.focused_part(),
            CenterPart::Main,
            "focus stayed on the agent"
        );
    }

    fn draw_to_string(app: &mut App) -> String {
        screen(&draw(app))
    }

    /// The sidebar's interact key on an agent whose dock had focus hands the
    /// keyboard to the agent it puts fullscreen, so keys never land in the shell.
    #[test]
    fn interacting_with_the_agent_from_the_sidebar_sends_keys_to_the_agent() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(tab).unwrap();
        assert_eq!(app.focused_part(), CenterPart::Dock);
        app.handle_key(tab).unwrap();
        app.handle_key(tab).unwrap();
        assert_eq!(app.focus, FocusPane::Left);
        app.handle_key(key(KeyCode::Char('i'), KeyModifiers::NONE))
            .unwrap();
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        assert_eq!(app.selected_terminal_surface_id(), Some(slot));
        assert_eq!(app.drawn_parts(), vec![CenterPart::Main]);
    }

    /// Whatever the pane focus says, the part being typed into or shown
    /// fullscreen is the part that receives bytes.
    #[test]
    fn the_typing_target_always_names_the_part_that_receives_bytes() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.session_surface = SessionSurface::Terminal;
        app.input_target = InputTarget::Agent;
        assert_eq!(app.focused_part(), CenterPart::Main);
        app.session_surface = SessionSurface::Agent;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        assert_eq!(app.focused_part(), CenterPart::Dock);
    }

    /// Closing the terminal being typed into fullscreen, from anywhere, leaves
    /// nothing typing: keys do not move on to the neighbour the view lands on.
    #[test]
    fn a_closed_terminal_being_typed_into_stops_the_typing() {
        let mut app = app_with_a_dock(2, Agent::Running);
        draw(&mut app);
        let ids = dock_ids(&app);
        app.session_surface = SessionSurface::Terminal;
        app.input_target = InputTarget::Terminal;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        assert_eq!(app.do_delete_terminal(&ids[0]), None);
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(
            app.active_terminal_id.as_ref(),
            Some(&ids[1]),
            "the neighbour"
        );
    }

    /// The terminal on screen ending hands the dock to the one to its right,
    /// or the left when it was the last.
    #[test]
    fn a_closed_terminal_hands_the_dock_to_its_neighbour() {
        let mut app = app_with_a_dock(3, Agent::Running);
        draw(&mut app);
        let ids = dock_ids(&app);
        app.show_dock_terminal(&ids[1]);
        draw(&mut app);
        assert_eq!(app.do_delete_terminal(&ids[1]), None);
        assert_eq!(app.active_terminal_id.as_ref(), Some(&ids[2]));
        assert_eq!(app.do_delete_terminal(&ids[2]), None);
        assert_eq!(app.active_terminal_id.as_ref(), Some(&ids[0]));
    }

    /// Looking at the agent part clears its attention and lets its clipboard
    /// through, though its dock has terminals; looking at the dock does neither.
    #[test]
    fn attention_and_the_clipboard_follow_the_part_with_focus() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.engine.config.ui.attention_grace_seconds = 0;
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        assert_eq!(app.focused_host_passthrough_tab(), Some(slot.clone()));
        app.engine.needs_attention.insert(TabId::new(slot.clone()));
        app.note_focused_agent_viewed();
        assert!(!app.engine.tab_needs_attention(&slot));

        app.session_surface = SessionSurface::Terminal;
        assert_eq!(app.focused_host_passthrough_tab(), None);
        app.engine.needs_attention.insert(TabId::new(slot.clone()));
        app.note_focused_agent_viewed();
        assert!(app.engine.tab_needs_attention(&slot));
    }

    /// An agent started elsewhere while its terminal is being typed into does
    /// not take the keyboard by bringing back a layout that hides the dock.
    #[test]
    fn starting_a_stopped_agent_elsewhere_keeps_the_keyboard_on_its_terminal() {
        let mut app = app_with_a_dock(1, Agent::Stopped);
        let session_id = app.engine.sessions[0].id.clone();
        app.dock_layouts.insert(session_id, DockLayout::Agent);
        assert_eq!(app.focused_part(), CenterPart::Dock);
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        app.engine.providers.insert(TabId::new(slot), sleeper());
        app.reconcile_center_target();
        assert_eq!(app.focused_part(), CenterPart::Dock);
        assert!(app.layout_parts().contains(&CenterPart::Dock));
        // Once focus leaves the center, the remembered layout applies.
        app.focus = FocusPane::Left;
        app.reconcile_center_target();
        assert_eq!(app.selected_dock_layout(), DockLayout::Agent);
    }

    #[test]
    fn tab_reaches_a_dock_terminal_being_typed_into() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.engine.config.ui.tab_reaches_agent = true;
        app.session_surface = SessionSurface::Terminal;
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, FocusPane::Center);
        assert_eq!(
            app.focused_part(),
            CenterPart::Dock,
            "Tab went to the shell"
        );
    }

    #[test]
    fn the_tab_commands_act_on_the_dock_while_it_has_focus() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.session_surface = SessionSurface::Terminal;
        app.execute_command("new-agent-tab").unwrap();
        assert_eq!(
            dock_ids(&app).len(),
            2,
            "a new terminal, not a provider tab"
        );
        app.focus = FocusPane::Center;
        app.session_surface = SessionSurface::Terminal;
        app.execute_command("close-tab").unwrap();
        assert!(matches!(
            app.prompt,
            PromptState::ConfirmDeleteTerminal { .. }
        ));
        app.prompt = PromptState::None;
        app.execute_command("stop-agent-tab").unwrap();
        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(message, TERMINAL_STOP_REFUSAL);
    }

    #[test]
    fn a_mouse_drag_reorders_the_dock_pills() {
        let mut app = app_with_a_dock(2, Agent::Running);
        draw(&mut app);
        let ids = dock_ids(&app);
        let rect = |id: &str, app: &App| {
            app.mouse_layout
                .dock_pills
                .iter()
                .find(|(pill, _)| pill == id)
                .map(|(_, rect)| *rect)
                .expect("the pill is drawn")
        };
        let (from, to) = (rect(&ids[0], &app), rect(&ids[1], &app));
        let at = |kind, rect: Rect| MouseEvent {
            kind,
            column: rect.x + 2,
            row: rect.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), from));
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), to));
        app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), to));
        assert_eq!(dock_ids(&app), vec![ids[1].clone(), ids[0].clone()]);
    }

    /// The dock's selection and link tests read the dock's own grid, not the
    /// agent's.
    #[test]
    fn the_dock_reads_its_own_snapshot() {
        let mut app = app_with_a_dock(1, Agent::Running);
        app.part_snapshots.dock.buf.scrollback_offset = 7;
        app.part_snapshots.main.buf.scrollback_offset = 0;
        app.session_surface = SessionSurface::Terminal;
        assert_eq!(app.snapshot_selection_origin().scrollback_offset, 7);
        app.session_surface = SessionSurface::Agent;
        assert_eq!(app.snapshot_selection_origin().scrollback_offset, 0);
    }

    /// The control sits on the dock's strip, on the agent's strip when the
    /// dock is hidden, and on a title row when the strips are.
    #[test]
    fn the_layout_control_moves_with_the_strips_and_the_dock() {
        let control_row = |app: &App| {
            app.mouse_layout
                .layout_control
                .first()
                .map(|(_, rect)| rect.y)
                .expect("the control is drawn")
        };
        let mut app = app_with_a_dock(2, Agent::Running);
        draw(&mut app);
        let pill = app.mouse_layout.dock_pills[0].1;
        assert_eq!(control_row(&app), pill.y + 1, "on the dock's strip");

        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Agent);
        draw(&mut app);
        let tab = app.agent_tab_regions[0].1;
        assert_eq!(control_row(&app), tab.y + 1, "on the agent's strip");

        let mut app = app_with_a_dock(1, Agent::Running);
        app.session_surface = SessionSurface::Terminal;
        draw(&mut app);
        let grid = app.mouse_layout.focused_term.expect("the dock's grid");
        assert_eq!(control_row(&app), grid.y - 1, "on the dock's title row");
    }

    #[test]
    fn the_strips_show_and_hide_together() {
        let mut app = app_with_a_dock(1, Agent::Running);
        draw(&mut app);
        assert!(app.agent_tab_regions.is_empty() && app.mouse_layout.dock_pills.is_empty());
        let session_id = app.engine.sessions[0].id.clone();
        app.engine
            .create_companion_terminal(&session_id, 24, 80)
            .expect("a second terminal");
        draw(&mut app);
        assert_eq!(app.agent_tab_regions.len(), 1, "the lone tab's strip shows");
        assert_eq!(app.mouse_layout.dock_pills.len(), 2);
    }

    #[test]
    fn a_stopped_agent_in_the_split_shows_the_short_card() {
        let mut app = app_with_a_dock(1, Agent::Stopped);
        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Split);
        let text = draw_to_string(&mut app);
        assert!(text.contains("isn't running. Start picks up"), "{text}");
        assert!(text.contains("Start"), "{text}");
        assert!(app.mouse_layout.dormant_tab_button.is_some());
    }

    #[test]
    fn a_terminal_rows_header_names_the_terminal() {
        let mut app = app_with_a_dock(0, Agent::Running);
        app.engine
            .create_standalone_terminal(24, 80)
            .expect("standalone");
        app.rebuild_left_items();
        let row = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Terminal(_)))
            .expect("the terminal's row");
        app.select_left_agent_item(row);
        let text = draw_to_string(&mut app);
        let header = text.lines().next().unwrap_or_default();
        assert!(header.contains("terminal: Terminal"), "{header}");
    }

    /// The layout is remembered per agent across moving to another row and
    /// back.
    #[test]
    fn the_layout_survives_a_trip_to_another_row() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Term);
        app.engine
            .create_standalone_terminal(24, 80)
            .expect("standalone");
        app.rebuild_left_items();
        let terminal_row = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Terminal(_)))
            .expect("the terminal's row");
        app.select_left_agent_item(terminal_row);
        let agent_row = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(_)))
            .expect("the agent's row");
        app.select_left_agent_item(agent_row);
        assert_eq!(app.selected_dock_layout(), DockLayout::Term);
    }

    /// A rebuild keeps the cursor on a terminal's ROW, found by id, when a new
    /// row lands above it.
    #[test]
    fn a_rebuild_keeps_the_cursor_on_its_terminal_row() {
        let mut app = app_with_a_dock(0, Agent::Running);
        app.engine.set_agent_sort("name");
        let (kept, _) = app
            .engine
            .create_standalone_terminal(24, 80)
            .expect("standalone");
        app.rebuild_left_items();
        app.reselect_left_terminal(&kept);
        let (other, _) = app
            .engine
            .create_standalone_terminal(24, 80)
            .expect("another");
        // Opened with a name that sorts first.
        app.engine
            .companion_terminals
            .get_mut(&other)
            .expect("the new terminal")
            .label = "A first".to_string();
        app.rebuild_left_items();
        assert_eq!(app.selected_terminal_row(), Some(kept));
    }

    /// A step past a row the list does not show moves nothing on screen, so the
    /// move goes on past it, or says nothing moved.
    #[test]
    fn moving_a_terminal_past_a_hidden_row_moves_it_on_screen_or_says_so() {
        let mut app = app_with_a_dock(0, Agent::Running);
        app.engine.sessions[0].status = crate::model::SessionStatus::Active;
        let mut quiet = app.engine.sessions[0].clone();
        quiet.id = "quiet-agent".to_string();
        quiet.status = crate::model::SessionStatus::Exited;
        app.engine.sessions.push(quiet);
        app.engine.set_agent_sort("manual");
        let (terminal, _) = app
            .engine
            .create_standalone_terminal(24, 80)
            .expect("standalone");
        // After the quiet agent, which the collapsed tail hides.
        app.engine
            .place_terminal_in_list(&terminal, Some("quiet-agent".to_string()));
        app.inactive_tail.overridden = true;
        app.inactive_tail.collapsed = true;
        app.rebuild_left_items();
        app.reselect_left_terminal(&terminal);
        let before: Vec<_> = app.left_rows.clone();
        app.move_selected_terminal(super::super::reorder::MoveDir::Up);
        assert_ne!(
            app.left_rows, before,
            "the row moved above the visible agent"
        );
        assert_eq!(
            app.left_rows.first(),
            Some(&crate::app::LeftRow::Terminal(terminal))
        );
    }

    /// The painted cells of the part without focus are grey and faded, text in
    /// the terminal's default colour included, which greys from the theme's
    /// body text; with focus outside the center they are drawn as the child
    /// sent them.
    #[test]
    fn the_unfocused_parts_painted_cells_are_grey_and_faded() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        app.engine.providers.insert(
            TabId::new(slot),
            PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf GREYME; sleep 30".to_string()],
                std::path::Path::new("."),
                10,
                40,
                100,
            )
            .expect("spawn pty"),
        );
        app.session_surface = SessionSurface::Terminal;
        let marker_cell = |terminal: &Terminal<TestBackend>| {
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height).find_map(|y| {
                (0..buffer.area.width.saturating_sub(5)).find_map(|x| {
                    let word: String = (0..6).map(|i| buffer[(x + i, y)].symbol()).collect();
                    (word == "GREYME").then(|| buffer[(x, y)].clone())
                })
            })
        };
        let mut cell = None;
        for _ in 0..300 {
            if let Some(found) = marker_cell(&draw(&mut app)) {
                cell = Some(found);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let cell = cell.expect("the agent's output is on screen");
        let expected = crate::theme::unfocused_part_color(app.theme.text_fg, app.theme.app_bg)
            .expect("the theme's text has channels");
        assert_eq!(cell.fg, expected, "default-colour text greys and fades");

        app.focus = FocusPane::Left;
        let cell = marker_cell(&draw(&mut app)).expect("still on screen");
        assert_eq!(cell.fg, ratatui::style::Color::Reset, "full colour again");
    }

    /// The scroll-forwarding policy is the part with the keyboard's: the
    /// agent's provider setting while the agent is typed into, the terminal's
    /// automatic one while the dock is, whatever the pane focus says.
    #[test]
    fn the_forward_scroll_policy_follows_the_part_with_the_keyboard() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let session = app.engine.sessions[0].clone();
        let provider = app.focused_tab_provider(&session).as_str().to_string();
        app.engine
            .config
            .providers
            .commands
            .entry(provider)
            .or_default()
            .forward_scroll = Some(true);
        app.session_surface = SessionSurface::Terminal;
        assert_eq!(app.selected_surface_forward_scroll(), None);
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        assert_eq!(app.selected_surface_forward_scroll(), Some(true));
    }

    /// A layout held while the agent's terminal had the keyboard gives way at
    /// once to a layout asked for by the control or the key.
    #[test]
    fn asking_for_a_layout_replaces_a_held_one() {
        let mut app = app_with_a_dock(1, Agent::Stopped);
        let session_id = app.engine.sessions[0].id.clone();
        let slot = app.engine.sessions[0].slot_tab_id().as_str().to_string();
        app.engine.providers.insert(TabId::new(slot), sleeper());
        app.reconcile_center_target();
        assert_eq!(app.selected_dock_layout(), DockLayout::Term, "held");
        app.set_dock_layout(&session_id, DockLayout::Split);
        assert_eq!(app.selected_dock_layout(), DockLayout::Split);
        app.reconcile_center_target();
        assert_eq!(app.selected_dock_layout(), DockLayout::Split);
    }

    /// Fullscreen on the agent from the sidebar, with its terminal shown alone,
    /// keeps the keyboard on the agent frame after frame.
    #[test]
    fn the_agent_typed_into_from_a_terminal_only_layout_keeps_the_keyboard() {
        let mut app = app_with_a_dock(1, Agent::Running);
        let session_id = app.engine.sessions[0].id.clone();
        app.set_dock_layout(&session_id, DockLayout::Term);
        app.focus = FocusPane::Left;
        app.handle_key(key(KeyCode::Char('i'), KeyModifiers::NONE))
            .unwrap();
        draw(&mut app);
        draw(&mut app);
        assert_eq!(app.session_surface, SessionSurface::Agent);
        assert_eq!(app.focused_surface(), SessionSurface::Agent);
    }
}
