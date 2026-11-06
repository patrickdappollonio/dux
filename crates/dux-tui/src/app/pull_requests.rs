//! An agent's pull requests on the terminal UI: which one the band shows and
//! the next-pull-request key that steps it, and the Pull requests modal, a
//! Picker over the agent's pull requests or over the branches its worktree has
//! been on. Every list is read from engine memory each frame, in the order
//! `dux_core::pr_list` gives, so the band, the badge and the modal agree with
//! the web and the command line.

use dux_core::engine::EventReaction;
use dux_core::pr_list::{MoveTo, OrderedPrs, PrSortMode, Step};
use dux_core::viewmodel::{PrBranchesView, PrKeyView};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListItem, Paragraph};

use super::components::ellipsis::{ellipsize_spans, fit_to_width, pad_to_width};
use super::components::{Hint, PickerList, labelled_name, modal_hint_line};
use super::render::centered_rect;
use super::{App, OverlayMouseLayout, PromptState, PullRequestsPrompt, PullRequestsView};
use crate::keybindings::{Action, BindingScope};
use crate::model::{AgentPr, PrKey, PrState};

/// The pull request the band shows, with where it sits in the agent's order.
pub(crate) struct BandPr<'a> {
    pub(crate) pr: &'a AgentPr,
    /// Zero-based place in the visible list.
    pub(crate) index: usize,
    /// How many pull requests the agent shows.
    pub(crate) count: usize,
}

/// One row of the Pull requests modal's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModalRow {
    Heading(&'static str),
    Blank,
    /// The folded Hidden section: how many it holds.
    HiddenSummary(usize),
    Pr(PrKey),
    /// A branch, by its place in the branch list.
    Branch(usize),
}

impl ModalRow {
    fn selectable(&self) -> bool {
        matches!(self, Self::Pr(_) | Self::Branch(_))
    }
}

fn key_of(view: &PrKeyView) -> PrKey {
    PrKey::new(view.host.clone(), view.owner_repo.clone(), view.number)
}

/// "Showing pull request 2 of 3 on "store-tab-rows"; Ctrl-n steps to the
/// next one."
fn stepped_message(index: usize, count: usize, agent: &str, key: &str) -> String {
    let tail = if key.is_empty() {
        String::new()
    } else {
        format!("; {key} steps to the next one")
    };
    format!(
        "Showing pull request {} of {count} on \"{agent}\"{tail}.",
        index + 1
    )
}

impl App {
    /// The selected agent's pull requests in its order, and the hidden ones.
    pub(crate) fn ordered_prs(&self, session_id: &str) -> OrderedPrs<'_> {
        let rows = self
            .engine
            .agent_prs
            .get(session_id)
            .map_or(&[][..], Vec::as_slice);
        dux_core::pr_list::order(rows, self.engine.pr_sort_mode(session_id))
    }

    /// The pull request the band shows for `session_id`: the one the user
    /// stepped to while it is still listed, otherwise the first.
    pub(crate) fn band_pr(&self, session_id: &str) -> Option<BandPr<'_>> {
        let visible = self.ordered_prs(session_id).visible;
        let count = visible.len();
        let index = self
            .pr_cursors
            .get(session_id)
            .and_then(|key| visible.iter().position(|row| row.is(key)))
            .unwrap_or(0);
        visible.get(index).map(|pr| BandPr { pr, index, count })
    }

    /// The next-pull-request key: step the band to the agent's next pull
    /// request, wrapping after the last, and say which one it shows.
    pub(crate) fn step_pull_request(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let session_id = session.id.clone();
        let agent = session.display_label();
        if session.workspace.as_managed().is_none() {
            self.set_info(format!(
                "\"{agent}\" is a standalone agent; it has no branch for pull requests."
            ));
            return;
        }
        let (count, from) = self
            .band_pr(&session_id)
            .map_or((0, 0), |band| (band.count, band.index));
        match count {
            0 => self.set_info(format!("\"{agent}\" has no pull requests.")),
            1 => self.set_info(format!(
                "\"{agent}\" has only one pull request, so there is nothing to step to."
            )),
            _ => {
                let to = dux_core::pr_list::step(count, from, Step::Next);
                if let Some(key) = self
                    .ordered_prs(&session_id)
                    .visible
                    .get(to)
                    .map(|row| row.key())
                {
                    self.pr_cursors.insert(session_id, key);
                }
                let label = self.bindings.label_for(Action::NextPullRequest);
                self.set_info(stepped_message(to, count, &agent, &label));
            }
        }
    }

    /// `pull-requests`: open the modal for the selected agent.
    pub(crate) fn open_pull_requests_modal(&mut self) {
        let Some(session) = self.selected_session() else {
            self.set_error("No agent selected. Select an agent to see its pull requests.");
            return;
        };
        if session.workspace.as_managed().is_none() {
            let agent = session.display_label();
            self.set_info(format!(
                "\"{agent}\" is a standalone agent; it has no branch for pull requests."
            ));
            return;
        }
        let session_id = session.id.clone();
        self.input_target = super::InputTarget::None;
        self.fullscreen_overlay = super::FullscreenOverlay::None;
        self.prompt = PromptState::PullRequests(PullRequestsPrompt {
            session_id,
            view: PullRequestsView::PullRequests,
            cursor: 0,
            show_hidden: false,
        });
        self.set_prompt_hint(
            "Choose a pull request to open in your browser. A hidden one stays off the band and \
             the row until you show it again.",
        );
    }

    /// The branch list the Branches view reads, or nothing for an agent the
    /// engine refuses (gone, or standalone).
    fn pull_request_branches(&self, session_id: &str) -> Option<PrBranchesView> {
        self.engine.pull_request_branches(session_id).ok()
    }

    /// The modal's rows for `prompt`, in the order they are drawn.
    pub(crate) fn pull_request_modal_rows(&self, prompt: &PullRequestsPrompt) -> Vec<ModalRow> {
        let mut rows = Vec::new();
        match prompt.view {
            PullRequestsView::PullRequests => {
                let ordered = self.ordered_prs(&prompt.session_id);
                match self.engine.pr_sort_mode(&prompt.session_id) {
                    PrSortMode::Automatic => {
                        let (open, done): (Vec<&AgentPr>, Vec<&AgentPr>) = ordered
                            .visible
                            .iter()
                            .partition(|row| matches!(row.state, PrState::Open | PrState::Draft));
                        for (heading, group) in
                            [("Open, newest first", open), ("Merged and closed", done)]
                        {
                            if !group.is_empty() {
                                rows.push(ModalRow::Heading(heading));
                                rows.extend(group.iter().map(|row| ModalRow::Pr(row.key())));
                            }
                        }
                    }
                    PrSortMode::Manual => {
                        if !ordered.visible.is_empty() {
                            rows.push(ModalRow::Heading("Your order"));
                            rows.extend(ordered.visible.iter().map(|row| ModalRow::Pr(row.key())));
                        }
                    }
                }
                if !ordered.hidden.is_empty() {
                    if !rows.is_empty() {
                        rows.push(ModalRow::Blank);
                    }
                    if prompt.show_hidden {
                        rows.push(ModalRow::Heading("Hidden"));
                        rows.extend(ordered.hidden.iter().map(|row| ModalRow::Pr(row.key())));
                    } else {
                        rows.push(ModalRow::HiddenSummary(ordered.hidden.len()));
                    }
                }
            }
            PullRequestsView::Branches => {
                let Some(branches) = self.pull_request_branches(&prompt.session_id) else {
                    return rows;
                };
                rows.push(ModalRow::Heading(
                    "Branches this worktree has been on, current first",
                ));
                rows.extend((0..branches.branches.len()).map(ModalRow::Branch));
                if !branches.on_no_listed_branch.is_empty() {
                    rows.push(ModalRow::Blank);
                    rows.push(ModalRow::Heading("On no branch listed here"));
                    rows.extend(
                        branches
                            .on_no_listed_branch
                            .iter()
                            .map(|key| ModalRow::Pr(key_of(key))),
                    );
                }
            }
        }
        rows
    }

    /// The selectable row the cursor is on, clamped to the rows there are.
    fn pull_request_modal_selected(
        &self,
        prompt: &PullRequestsPrompt,
    ) -> Option<(usize, ModalRow)> {
        let rows = self.pull_request_modal_rows(prompt);
        let selectable: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].selectable()).collect();
        let at = *selectable.get(prompt.cursor.min(selectable.len().checked_sub(1)?))?;
        Some((at, rows[at].clone()))
    }

    /// The agent's row for `key`, visible or hidden.
    fn agent_pr(&self, session_id: &str, key: &PrKey) -> Option<&AgentPr> {
        self.engine
            .agent_prs
            .get(session_id)?
            .iter()
            .find(|row| row.is(key))
    }

    /// The modal's keys. Its own scope first, then the shared picker
    /// vocabulary, so a rebind can never leave the hints naming one key and
    /// the modal answering another.
    pub(crate) fn handle_pull_requests_prompt_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Option<bool> {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return None;
        };
        let prompt = prompt.clone();
        let action = self
            .bindings
            .lookup(&key, BindingScope::PullRequestList)
            .or_else(|| self.bindings.lookup(&key, BindingScope::Palette));
        let selectable = self
            .pull_request_modal_rows(&prompt)
            .iter()
            .filter(|row| row.selectable())
            .count();
        match action {
            Some(Action::CloseOverlay) => {
                self.prompt = PromptState::None;
                self.clear_prompt_hint();
            }
            Some(Action::MoveDown) => {
                self.set_pull_requests_cursor(
                    (prompt.cursor + 1).min(selectable.saturating_sub(1)),
                );
            }
            Some(Action::MoveUp) => self.set_pull_requests_cursor(prompt.cursor.saturating_sub(1)),
            Some(Action::Confirm) => self.open_selected_pull_request(),
            Some(Action::PullRequestsAttach) => self.open_attach_pull_request_prompt(prompt),
            Some(Action::PullRequestsToggleView) => {
                let view = match prompt.view {
                    PullRequestsView::PullRequests => PullRequestsView::Branches,
                    PullRequestsView::Branches => PullRequestsView::PullRequests,
                };
                if view == PullRequestsView::Branches {
                    let agent = self.pull_requests_agent_name(&prompt.session_id);
                    self.set_prompt_hint(format!(
                        "Every branch \"{agent}\" has had checked out, with the pull requests dux \
                         found for each."
                    ));
                }
                self.prompt = PromptState::PullRequests(PullRequestsPrompt {
                    view,
                    cursor: 0,
                    ..prompt
                });
            }
            _ if prompt.view == PullRequestsView::Branches => {}
            Some(Action::PullRequestsToggleHidden) => {
                self.prompt = PromptState::PullRequests(PullRequestsPrompt {
                    show_hidden: !prompt.show_hidden,
                    ..prompt
                });
            }
            Some(Action::PullRequestsToggleSort) => {
                let mode = match self.engine.pr_sort_mode(&prompt.session_id) {
                    PrSortMode::Automatic => PrSortMode::Manual,
                    PrSortMode::Manual => PrSortMode::Automatic,
                };
                let outcome = self.engine.set_pull_request_sort(&prompt.session_id, mode);
                self.report_pull_request_change(outcome);
            }
            Some(Action::PullRequestHideOrDetach) => self.hide_or_detach_selected_pull_request(),
            Some(Action::PullRequestMoveUp) => self.move_selected_pull_request(MoveTo::Up),
            Some(Action::PullRequestMoveDown) => self.move_selected_pull_request(MoveTo::Down),
            Some(Action::PullRequestMoveTop) => self.move_selected_pull_request(MoveTo::Top),
            _ => {}
        }
        Some(false)
    }

    fn pull_requests_agent_name(&self, session_id: &str) -> String {
        self.engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(crate::model::AgentSession::display_label)
            .unwrap_or_default()
    }

    pub(crate) fn set_pull_requests_cursor(&mut self, cursor: usize) {
        if let PromptState::PullRequests(prompt) = &mut self.prompt {
            prompt.cursor = cursor;
        }
    }

    /// Put the cursor on `key`'s row, wherever a change moved it.
    fn follow_pull_request(&mut self, key: &PrKey) {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return;
        };
        let at = self
            .pull_request_modal_rows(prompt)
            .iter()
            .filter(|row| row.selectable())
            .position(|row| *row == ModalRow::Pr(key.clone()));
        if let Some(at) = at {
            self.set_pull_requests_cursor(at);
        }
    }

    fn report_pull_request_change(
        &mut self,
        outcome: anyhow::Result<dux_core::engine::StatusUpdate>,
    ) {
        // The modal stays open, so its instruction would otherwise hold the
        // line and queue the outcome of the change behind it.
        self.clear_prompt_hint();
        match outcome {
            Ok(update) => self.apply_reaction(EventReaction::Status(update)),
            Err(err) => self.set_error(format!("{err:#}")),
        }
    }

    /// The confirm key: open the selected pull request, or the selected
    /// branch's first one.
    pub(crate) fn open_selected_pull_request(&mut self) {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return;
        };
        let prompt = prompt.clone();
        let key = match self.pull_request_modal_selected(&prompt) {
            Some((_, ModalRow::Pr(key))) => key,
            Some((_, ModalRow::Branch(index))) => {
                let branch = self
                    .pull_request_branches(&prompt.session_id)
                    .and_then(|view| view.branches.into_iter().nth(index));
                let Some(branch) = branch else {
                    return;
                };
                let Some(first) = branch.prs.first() else {
                    self.set_info(format!(
                        "Branch \"{}\" has no pull request dux knows of.",
                        branch.name
                    ));
                    return;
                };
                key_of(first)
            }
            _ => return,
        };
        if let Some(pr) = self.agent_pr(&prompt.session_id, &key).cloned() {
            self.open_pr_in_browser(&pr);
        }
    }

    /// The hide key, which the row decides: a hidden one is shown again, an
    /// attached one detached, and one dux found hidden.
    fn hide_or_detach_selected_pull_request(&mut self) {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return;
        };
        let prompt = prompt.clone();
        let Some((_, ModalRow::Pr(key))) = self.pull_request_modal_selected(&prompt) else {
            return;
        };
        let Some(row) = self.agent_pr(&prompt.session_id, &key) else {
            return;
        };
        let outcome = if row.hidden {
            self.engine.show_pull_request(&prompt.session_id, &key)
        } else if row.attached {
            self.engine.detach_pull_request(&prompt.session_id, &key)
        } else {
            self.engine.hide_pull_request(&prompt.session_id, &key)
        };
        self.report_pull_request_change(outcome);
        // A row that left its section keeps the cursor on it where it landed;
        // one that left the list leaves the cursor where it was, clamped.
        self.follow_pull_request(&key);
    }

    fn move_selected_pull_request(&mut self, to: MoveTo) {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return;
        };
        let prompt = prompt.clone();
        let Some((_, ModalRow::Pr(key))) = self.pull_request_modal_selected(&prompt) else {
            return;
        };
        let outcome = self.engine.move_pull_request(&prompt.session_id, &key, to);
        self.report_pull_request_change(outcome);
        self.follow_pull_request(&key);
    }

    /// The attach key: the reference field, which the close key leaves for
    /// this modal as it was.
    fn open_attach_pull_request_prompt(&mut self, back: PullRequestsPrompt) {
        if !self.github_pr_agent_command_available() {
            self.set_error(
                "Attaching a pull request requires GitHub integration and an authenticated gh CLI.",
            );
            return;
        }
        self.prompt = PromptState::AttachPullRequestInput {
            session_id: back.session_id.clone(),
            back,
            input: super::text_input::TextInput::new(),
        };
    }

    /// The sentence over the attach field: what the new pull request joins.
    pub(crate) fn attach_pull_request_lead(&self, session_id: &str) -> (String, String) {
        let count = self.ordered_prs(session_id).visible.len();
        let lead = match count {
            0 => "Becomes the first pull request on ".to_string(),
            n => format!(
                "Joins the {} on ",
                dux_core::text::count_of(n, "pull request")
            ),
        };
        (lead, self.pull_requests_agent_name(session_id))
    }

    /// The state word's color, GitHub's own palette for each state.
    pub(crate) fn pr_state_label_color(&self, state: &PrState) -> Color {
        match state {
            PrState::Open => self.theme.pr_open_label,
            PrState::Draft => self.theme.pr_draft_label,
            PrState::Merged => self.theme.pr_merged_label,
            PrState::Closed => self.theme.pr_closed_label,
        }
    }

    /// The band's background for a state.
    pub(crate) fn pr_state_band_color(&self, state: &PrState) -> Color {
        match state {
            PrState::Open => self.theme.pr_open_bg,
            PrState::Draft => self.theme.pr_draft_bg,
            PrState::Merged => self.theme.pr_merged_bg,
            PrState::Closed => self.theme.pr_closed_bg,
        }
    }

    pub(crate) fn render_pull_requests_prompt(&mut self, frame: &mut Frame<'_>) {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return;
        };
        let prompt = prompt.clone();
        // The agent went away under the modal: so does the modal.
        if !self
            .engine
            .sessions
            .iter()
            .any(|s| s.id == prompt.session_id)
        {
            self.prompt = PromptState::None;
            self.clear_prompt_hint();
            return;
        }
        self.render_dim_overlay(frame);
        let area = centered_rect(84, 70, frame.area());
        self.clear_overlay_area(frame, area);

        let rows = self.pull_request_modal_rows(&prompt);
        let selected = self.pull_request_modal_selected(&prompt);
        let [header_area, list_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(3)])
            .areas(area);

        let header = self.pull_requests_header(
            &prompt,
            selected.as_ref().map(|(_, row)| row),
            header_area.width.saturating_sub(2),
        );
        frame.render_widget(header, header_area);

        let width = usize::from(list_area.width.saturating_sub(2));
        let items: Vec<ListItem<'static>> = rows
            .iter()
            .map(|row| {
                ListItem::new(Line::from(
                    self.pull_request_modal_line(&prompt, row, width),
                ))
            })
            .collect();
        let attach = self.bindings.label_for(Action::PullRequestsAttach);
        let empty = match prompt.view {
            PullRequestsView::PullRequests if attach.is_empty() => {
                "No pull requests yet.".to_string()
            }
            PullRequestsView::PullRequests => {
                format!("No pull requests yet. Press {attach} to attach one.")
            }
            PullRequestsView::Branches => "No branches recorded yet.".to_string(),
        };
        let list_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(self.theme.overlay_border))
            .style(Style::default().bg(self.theme.overlay_bg));
        let list = PickerList::new(items, selected.as_ref().map(|(at, _)| *at), empty)
            .block(list_block)
            .render(frame, list_area, &self.theme);
        self.overlay_layout.active = OverlayMouseLayout::PullRequests {
            list: list.list,
            items: list.items,
            offset: list.offset,
        };
    }

    /// The selectable index of the row a click landed on, if it is one.
    pub(crate) fn pull_request_modal_selectable_at(&self, item: usize) -> Option<usize> {
        let PromptState::PullRequests(prompt) = &self.prompt else {
            return None;
        };
        let rows = self.pull_request_modal_rows(prompt);
        rows.get(item)
            .filter(|row| row.selectable())
            .map(|_| rows[..item].iter().filter(|row| row.selectable()).count())
    }

    fn pull_requests_header(
        &self,
        prompt: &PullRequestsPrompt,
        selected: Option<&ModalRow>,
        hint_width: u16,
    ) -> Paragraph<'static> {
        let label = Style::default().fg(self.theme.hint_desc_fg);
        let on = Style::default()
            .fg(self.theme.title_focused)
            .add_modifier(Modifier::BOLD);
        let ordered = self.ordered_prs(&prompt.session_id);
        let branch_count = self
            .pull_request_branches(&prompt.session_id)
            .map_or(0, |view| view.branches.len());
        let (prs_style, branches_style) = match prompt.view {
            PullRequestsView::PullRequests => (on, label),
            PullRequestsView::Branches => (label, on),
        };
        let order = match self.engine.pr_sort_mode(&prompt.session_id) {
            PrSortMode::Automatic => "automatic",
            PrSortMode::Manual => "your own",
        };
        let agent = self.pull_requests_agent_name(&prompt.session_id);
        let lines = vec![
            labelled_name(" Agent:  ", &agent, label, &self.theme),
            Line::from(vec![
                Span::styled(" View:   ", label),
                Span::styled(
                    format!("Pull requests ({})", ordered.visible.len()),
                    prs_style,
                ),
                Span::styled("   ", label),
                Span::styled(format!("Branches ({branch_count})"), branches_style),
                Span::styled(format!("   Order: {order}"), label),
            ]),
        ];
        let hints = self.pull_requests_hints(prompt, selected);
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.theme.overlay_border))
                .style(Style::default().bg(self.theme.overlay_bg))
                .title(" Pull Requests ")
                .title_style(
                    Style::default()
                        .fg(self.theme.title_focused)
                        .add_modifier(Modifier::BOLD),
                )
                .title_bottom(modal_hint_line(&self.theme, &hints, hint_width)),
        )
    }

    fn pull_requests_hints(
        &self,
        prompt: &PullRequestsPrompt,
        selected: Option<&ModalRow>,
    ) -> Vec<Hint> {
        let key = |action| self.bindings.label_for(action);
        let close = Hint::key(key(Action::CloseOverlay), "close").pinned();
        match prompt.view {
            PullRequestsView::Branches => vec![
                Hint::key(key(Action::MoveDown), "down"),
                Hint::key(key(Action::MoveUp), "up"),
                Hint::key(key(Action::Confirm), "open its pull request"),
                Hint::key(key(Action::PullRequestsToggleView), "pull requests"),
                close,
            ],
            PullRequestsView::PullRequests => {
                let row = match selected {
                    Some(ModalRow::Pr(pr)) => self.agent_pr(&prompt.session_id, pr),
                    _ => None,
                };
                let hide_word = match row {
                    Some(row) if row.hidden => "show again",
                    Some(row) if row.attached => "detach",
                    _ => "hide",
                };
                let hidden_word = if prompt.show_hidden {
                    "hide hidden"
                } else {
                    "show hidden"
                };
                let mut hints = vec![
                    Hint::key(key(Action::MoveDown), "down"),
                    Hint::key(key(Action::MoveUp), "up"),
                    Hint::key(key(Action::Confirm), "open"),
                    Hint::key(key(Action::PullRequestsAttach), "attach"),
                ];
                if row.is_some() {
                    hints.push(Hint::key(key(Action::PullRequestHideOrDetach), hide_word));
                }
                if !self.ordered_prs(&prompt.session_id).hidden.is_empty() {
                    hints.push(Hint::key(
                        key(Action::PullRequestsToggleHidden),
                        hidden_word,
                    ));
                }
                hints.push(Hint::key(key(Action::PullRequestsToggleView), "branches"));
                hints.push(Hint::key(key(Action::PullRequestsToggleSort), "order"));
                if row.is_some_and(|row| !row.hidden) {
                    hints.push(Hint::keys(
                        [
                            key(Action::PullRequestMoveDown),
                            key(Action::PullRequestMoveUp),
                        ],
                        "move",
                    ));
                    hints.push(Hint::key(key(Action::PullRequestMoveTop), "to top"));
                }
                hints.push(close);
                hints
            }
        }
    }

    /// One row of the modal, laid out in columns: glyph and number, state,
    /// branch, title, and on the right what sets the row apart.
    fn pull_request_modal_line(
        &self,
        prompt: &PullRequestsPrompt,
        row: &ModalRow,
        width: usize,
    ) -> Vec<Span<'static>> {
        let heading = Style::default()
            .fg(self.theme.help_section_header_fg)
            .add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(self.theme.hint_desc_fg);
        match row {
            ModalRow::Blank => vec![Span::raw("")],
            ModalRow::Heading(text) => {
                let text = if *text == "Hidden" {
                    format!(
                        " Hidden ({})",
                        self.ordered_prs(&prompt.session_id).hidden.len()
                    )
                } else {
                    format!(" {text}")
                };
                vec![Span::styled(text, heading)]
            }
            ModalRow::HiddenSummary(count) => {
                let mut spans = vec![Span::styled(format!(" Hidden ({count})"), heading)];
                let key = self.bindings.label_for(Action::PullRequestsToggleHidden);
                if !key.is_empty() {
                    spans.push(Span::styled(format!("   {key} shows them"), muted));
                }
                spans
            }
            ModalRow::Pr(key) => {
                let Some(pr) = self.agent_pr(&prompt.session_id, key) else {
                    return vec![Span::raw("")];
                };
                let band = self
                    .band_pr(&prompt.session_id)
                    .is_some_and(|band| band.pr.is(key));
                let tag = if pr.hidden {
                    "hidden"
                } else if band {
                    "in the band"
                } else {
                    ""
                };
                let where_from = match &pr.head_branch {
                    Some(branch) if pr.found => branch.clone(),
                    _ if pr.attached => "attached by you".to_string(),
                    _ => String::new(),
                };
                self.pr_columns(prompt, pr, &where_from, tag, width)
            }
            ModalRow::Branch(index) => {
                let Some(branch) = self
                    .pull_request_branches(&prompt.session_id)
                    .and_then(|view| view.branches.into_iter().nth(*index))
                else {
                    return vec![Span::raw("")];
                };
                let text = Style::default().fg(self.theme.text_fg);
                let mut left = vec![
                    Span::styled(format!("  {}", pad_to_width(&branch.name, 22)), text),
                    Span::styled(
                        pad_to_width(if branch.current { "current" } else { "" }, 9),
                        text,
                    ),
                    Span::styled(
                        pad_to_width(&first_seen_words(branch.first_seen_at.as_deref()), 24),
                        muted,
                    ),
                ];
                match branch
                    .prs
                    .first()
                    .and_then(|key| self.agent_pr(&prompt.session_id, &key_of(key)))
                {
                    Some(pr) => {
                        let color = self.pr_state_label_color(&pr.state);
                        left.push(Span::styled(
                            format!(
                                "{} {}  ",
                                self.glyphs().pull_request,
                                self.engine.pull_request_label(&prompt.session_id, pr)
                            ),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ));
                        left.push(Span::styled(
                            pad_to_width(pr.state.word(), 8),
                            Style::default().fg(color),
                        ));
                        if pr.hidden {
                            left.push(Span::styled("hidden", muted));
                        }
                        if branch.prs.len() > 1 {
                            left.push(Span::styled(
                                format!("  +{} more", branch.prs.len() - 1),
                                muted,
                            ));
                        }
                    }
                    None => left.push(Span::styled("no pull request", muted)),
                }
                ellipsize_spans(left, u16::try_from(width).unwrap_or(u16::MAX))
            }
        }
    }

    fn pr_columns(
        &self,
        prompt: &PullRequestsPrompt,
        pr: &AgentPr,
        where_from: &str,
        tag: &str,
        width: usize,
    ) -> Vec<Span<'static>> {
        let color = self.pr_state_label_color(&pr.state);
        let label = self.engine.pull_request_label(&prompt.session_id, pr);
        let left = vec![
            Span::styled(
                format!(" {} ", self.glyphs().pull_request),
                Style::default().fg(color),
            ),
            Span::styled(
                pad_to_width(&label, 16),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(pad_to_width(pr.state.word(), 8), Style::default().fg(color)),
            Span::styled(
                format!("{} ", fit_to_width(where_from, 19)),
                Style::default().fg(self.theme.hint_desc_fg),
            ),
            Span::styled(
                pr.title.trim().to_string(),
                Style::default().fg(self.theme.text_fg),
            ),
        ];
        let right = if tag.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(
                format!("{tag} "),
                Style::default().fg(self.theme.hint_desc_fg),
            )]
        };
        super::render::right_align_line(left, right, u16::try_from(width).unwrap_or(u16::MAX), 2)
    }
}

/// "first seen Nov 3, 14:02" in local time, from the RFC 3339 the branch list
/// carries.
fn first_seen_words(at: Option<&str>) -> String {
    at.and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map_or_else(
            || "not recorded yet".to_string(),
            |at| {
                format!(
                    "first seen {}",
                    at.with_timezone(&chrono::Local).format("%b %-d, %H:%M")
                )
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FocusPane;
    use crate::app::test_support::{default_bindings, found_pr, test_app};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// Three pull requests on the fixture agent in a known automatic order:
    /// #418 open and newest, acme/infra#77 open and attached, #415 merged.
    fn three_prs(app: &mut App) {
        let at = |day: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, day)
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .map(|t| t.and_utc())
        };
        let mut newest = found_pr(
            "o/r",
            418,
            PrState::Open,
            "Store every tab as a row with a stable id",
        );
        newest.gh_created_at = at(3);
        let mut attached = found_pr("acme/infra", 77, PrState::Open, "Allow the preview host");
        attached.found = false;
        attached.attached = true;
        attached.gh_created_at = at(2);
        let mut merged = found_pr("o/r", 415, PrState::Merged, "Promote the next tab");
        merged.gh_created_at = at(1);
        app.engine
            .agent_prs
            .insert("session-1".to_string(), vec![merged, attached, newest]);
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers)).expect("key");
    }

    fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn screen(buf: &Buffer) -> String {
        let width = usize::from(buf.area.width);
        buf.content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn band_number(app: &App) -> Option<u64> {
        app.current_pr_info().map(|pr| pr.number)
    }

    #[test]
    fn the_step_key_walks_the_band_through_the_order_and_wraps() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        app.focus = FocusPane::Center;
        assert_eq!(band_number(&app), Some(418));

        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(band_number(&app), Some(77));
        let agent = app.engine.sessions[0].display_label();
        assert_eq!(
            app.status.message(),
            format!("Showing pull request 2 of 3 on \"{agent}\"; Ctrl-n steps to the next one.")
        );
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(band_number(&app), Some(415));
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            band_number(&app),
            Some(418),
            "after the last comes the first"
        );

        // Stepping is display-only: the badge and every other reader keep the
        // first in the order.
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            app.engine.shown_pr("session-1").map(|pr| pr.number),
            Some(418)
        );
    }

    #[test]
    fn the_step_key_says_why_it_cannot_step() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Center;
        let agent = app.engine.sessions[0].display_label();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            app.status.message(),
            format!("\"{agent}\" has no pull requests.")
        );

        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Center;
        app.engine.agent_prs.insert(
            "session-1".to_string(),
            vec![found_pr("o/r", 418, PrState::Open, "Only")],
        );
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            app.status.message(),
            format!("\"{agent}\" has only one pull request, so there is nothing to step to.")
        );
    }

    #[test]
    fn a_stepped_band_falls_back_to_the_first_when_its_pull_request_leaves() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        app.focus = FocusPane::Center;
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(band_number(&app), Some(77));
        app.engine
            .agent_prs
            .get_mut("session-1")
            .expect("rows")
            .retain(|row| row.number != 77);
        assert_eq!(band_number(&app), Some(418));
    }

    #[test]
    fn the_open_key_opens_the_pull_request_the_band_stepped_to() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        app.focus = FocusPane::Center;
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        app.url_opener = std::sync::Arc::new(move |url: &str| {
            let _ = tx.lock().expect("channel").send(url.to_string());
            Ok(())
        });
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)).ok(),
            Some("https://github.com/acme/infra/pull/77".to_string())
        );
    }

    /// The band's right end gives way in a fixed order as it narrows: the key
    /// hint first, then the counter, and only then the title past its floor.
    #[test]
    fn a_narrowing_band_drops_the_hint_then_the_counter_then_cuts_the_title() {
        let title = "Store every tab as a row with a stable id";
        let pr = found_pr("o/r", 418, PrState::Open, title);
        let band = |width: u16| {
            let mut app = test_app(default_bindings());
            let mut terminal = Terminal::new(TestBackend::new(width, 1)).expect("terminal");
            terminal
                .draw(|frame| {
                    app.render_pr_banner(
                        frame,
                        ratatui::layout::Rect::new(0, 0, width, 1),
                        &pr,
                        super::super::render::BandPlace { index: 0, count: 3 },
                    );
                })
                .expect("draw");
            screen(terminal.backend().buffer())
        };
        let wide = band(80);
        assert!(
            wide.contains(title) && wide.contains("1/3") && wide.contains("Ctrl-n next"),
            "{wide}"
        );
        let medium = band(45);
        assert!(
            medium.contains("1/3") && !medium.contains("Ctrl-n"),
            "{medium}"
        );
        assert!(!medium.contains(title), "the title is cut at 45: {medium}");
        let narrow = band(30);
        assert!(
            !narrow.contains("1/3") && narrow.contains("o/r#418"),
            "{narrow}"
        );
    }

    #[test]
    fn the_row_badge_shows_the_number_for_one_and_the_count_for_several() {
        let mut app = test_app(default_bindings());
        app.engine.agent_prs.insert(
            "session-1".to_string(),
            vec![found_pr("o/r", 418, PrState::Open, "Only")],
        );
        let glyph = app.glyphs().pull_request;
        let one = screen(&draw(&mut app, 120, 30));
        assert!(one.contains(&format!("{glyph}#418")), "{one}");
        three_prs(&mut app);
        let several = screen(&draw(&mut app, 120, 30));
        assert!(several.contains(&format!("{glyph}3")), "{several}");
        assert!(!several.contains(&format!("{glyph}#418")), "{several}");
    }

    #[test]
    fn a_draft_wears_githubs_draft_gray_on_the_band() {
        let mut app = test_app(default_bindings());
        app.theme.pr_draft_bg = Color::Rgb(1, 2, 3);
        app.engine.agent_prs.insert(
            "session-1".to_string(),
            vec![found_pr("o/r", 9, PrState::Draft, "Draft work")],
        );
        draw(&mut app, 120, 30);
        let band = app.mouse_layout.pr_banner.expect("the band is painted");
        let buf = draw(&mut app, 120, 30);
        assert_eq!(buf[(band.x + 2, band.y)].bg, Color::Rgb(1, 2, 3));
    }

    fn open_modal(app: &mut App) {
        app.engine.github_integration_enabled = true;
        app.execute_command("pull-requests").expect("open");
        assert!(
            matches!(app.prompt, PromptState::PullRequests(_)),
            "{:?}",
            app.prompt
        );
    }

    #[test]
    fn the_modal_lists_open_ones_then_merged_ones_and_marks_the_band() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        open_modal(&mut app);
        let shown = screen(&draw(&mut app, 140, 40));
        let open = shown.find("Open, newest first").expect(&shown);
        let first = shown.find("o/r#418").expect(&shown);
        let attached = shown.find("acme/infra#77").expect(&shown);
        let done = shown.find("Merged and closed").expect(&shown);
        let merged = shown.find("o/r#415").expect(&shown);
        assert!(
            open < first && first < attached && attached < done && done < merged,
            "{shown}"
        );
        assert!(
            shown.contains("attached by you") && shown.contains("in the band"),
            "{shown}"
        );
    }

    #[test]
    fn a_long_branch_name_is_cut_and_never_runs_into_the_title() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        app.engine
            .agent_prs
            .get_mut("session-1")
            .expect("rows")
            .iter_mut()
            .find(|row| row.number == 418)
            .expect("row")
            .head_branch = Some("login-session-cookie-handling".to_string());
        open_modal(&mut app);
        let shown = screen(&draw(&mut app, 140, 40));
        let row = shown
            .lines()
            .find(|line| line.contains("o/r#418"))
            .expect(&shown);
        assert!(
            row.contains("login-session-cook\u{2026} Store every tab"),
            "{row}"
        );
    }

    #[test]
    fn the_hide_key_hides_a_found_one_and_shows_it_again() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        open_modal(&mut app);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        let row = |app: &App| {
            app.engine.agent_prs["session-1"]
                .iter()
                .find(|row| row.number == 418)
                .cloned()
                .expect("row")
        };
        assert!(row(&app).hidden);
        assert!(
            app.status.message().starts_with("Hid o/r#418"),
            "{}",
            app.status.message()
        );

        // Listed again under Hidden, after the two still shown.
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert!(!row(&app).hidden, "{}", app.status.message());
    }

    #[test]
    fn the_hide_key_detaches_an_attached_one() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        open_modal(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert!(
            app.engine.agent_prs["session-1"]
                .iter()
                .all(|row| row.number != 77),
            "{}",
            app.status.message()
        );
    }

    #[test]
    fn a_move_from_automatic_order_switches_to_the_users_own() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        open_modal(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('K'), KeyModifiers::SHIFT);
        assert_eq!(app.engine.pr_sort_mode("session-1"), PrSortMode::Manual);
        let order: Vec<u64> = app
            .ordered_prs("session-1")
            .visible
            .iter()
            .map(|row| row.number)
            .collect();
        assert_eq!(order, vec![77, 418, 415]);
        let PromptState::PullRequests(prompt) = &app.prompt else {
            panic!("the modal stays open");
        };
        assert_eq!(prompt.cursor, 0, "the cursor followed the moved row");

        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
        assert_eq!(app.engine.pr_sort_mode("session-1"), PrSortMode::Automatic);
    }

    #[test]
    fn closing_attach_returns_to_the_modal_as_it_was() {
        let mut app = test_app(default_bindings());
        three_prs(&mut app);
        app.engine.gh_status = dux_core::model::GhStatus::Available;
        open_modal(&mut app);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let before = app.prompt.clone();
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        let shown = screen(&draw(&mut app, 120, 30));
        assert!(shown.contains("Joins the 3 pull requests on"), "{shown}");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(format!("{:?}", app.prompt), format!("{before:?}"));
    }

    #[test]
    fn the_modal_is_registered_as_a_picker() {
        let prompt =
            PromptState::PullRequests(crate::app::test_support::pull_requests_prompt("session-1"));
        assert_eq!(
            crate::app::modal::modal_spec(&prompt).map(|spec| spec.family),
            Some(crate::app::modal::ModalFamily::Picker)
        );
    }

    #[test]
    fn rebinding_a_modal_key_changes_its_hint() {
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings.insert(
            "pull_requests_toggle_view".to_string(),
            vec!["v".to_string()],
        );
        let mut app = test_app(crate::keybindings::RuntimeBindings::from_keys_config(&keys));
        three_prs(&mut app);
        open_modal(&mut app);
        let shown = screen(&draw(&mut app, 160, 40));
        assert!(shown.contains("v> branches"), "{shown}");
        assert!(!shown.contains("b> branches"), "{shown}");
        press(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
        let PromptState::PullRequests(prompt) = &app.prompt else {
            panic!("open");
        };
        assert_eq!(prompt.view, PullRequestsView::Branches);
        // None of the three names a head branch yet, so every one of them,
        // found or attached, is listed apart rather than left out.
        let shown = screen(&draw(&mut app, 160, 40));
        let apart = shown.find("On no branch listed here").expect(&shown);
        for pr in ["o/r#418", "acme/infra#77", "o/r#415"] {
            assert!(shown.find(pr).is_some_and(|at| at > apart), "{pr}: {shown}");
        }
    }
}
