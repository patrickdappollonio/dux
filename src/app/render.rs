use super::*;

/// ASCII art logo displayed in the agent pane when no content is active.
const ASCII_LOGO: &[&str] = &[
    "       ░██                       ",
    "       ░██                       ",
    " ░████████ ░██    ░██ ░██    ░██ ",
    "░██    ░██ ░██    ░██  ░██  ░██  ",
    "░██    ░██ ░██    ░██   ░█████   ",
    "░██   ░███ ░██   ░███  ░██  ░██  ",
    " ░█████░██  ░█████░██ ░██    ░██ ",
];
/// Display width of each line in `ASCII_LOGO` (all lines are equal width).
const ASCII_LOGO_WIDTH: u16 = 33;
/// Number of lines in `ASCII_LOGO`.
const ASCII_LOGO_HEIGHT: u16 = 7;

impl App {
    pub(crate) fn render(&mut self, frame: &mut Frame) {
        let term_w = frame.area().width as usize;
        let status_text_len = self.status.text().len() + 3; // " ● " prefix
        let status_lines: u16 = if term_w > 0 && status_text_len > term_w {
            2
        } else {
            1
        };
        let footer_h = 1 + status_lines; // 1 for hints + status lines
        let [header, body, footer] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(footer_h),
            ])
            .areas(frame.area());
        self.render_header(frame, header);
        let [left, center, right] = if self.left_collapsed {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(4),
                    Constraint::Min(20),
                    Constraint::Percentage(self.right_width_pct),
                ])
                .areas(body)
        } else {
            let center_pct = 100u16
                .saturating_sub(self.left_width_pct + self.right_width_pct)
                .max(20);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(self.left_width_pct),
                    Constraint::Percentage(center_pct),
                    Constraint::Percentage(self.right_width_pct),
                ])
                .areas(body)
        };
        self.render_left(frame, left);
        self.render_center(frame, center);
        self.render_files(frame, right);
        self.render_footer(frame, footer);
        self.render_overlay(frame);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let bg = self.theme.header_bg;
        let sep_fg = self.theme.header_separator_fg;
        let label_fg = self.theme.header_label_fg;
        let mut spans = vec![
            Span::styled(" dux ", Style::default().fg(label_fg).bg(bg)),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ),
        ];
        if let Some(project) = self.selected_project() {
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "project: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.name.clone(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "branch: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.current_branch.clone(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "provider: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.default_provider.as_str().to_string(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    fn render_left(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Left;

        if self.left_collapsed {
            let collapsed_left_items = self.left_items();
            let items = collapsed_left_items
                .iter()
                .enumerate()
                .map(|(i, item)| match item {
                    LeftItem::Project(index) => {
                        let project = &self.projects[*index];
                        let icon = if self.collapsed_projects.contains(&project.id) {
                            "▸"
                        } else {
                            "▾"
                        };
                        ListItem::new(Line::from(Span::styled(
                            icon,
                            Style::default().fg(self.theme.project_icon),
                        )))
                    }
                    LeftItem::Session(index) => {
                        let session = &self.sessions[*index];
                        let (dot, dot_color) = self.theme.session_dot(&session.status);
                        let is_last = !collapsed_left_items
                            .get(i + 1)
                            .is_some_and(|next| matches!(next, LeftItem::Session(_)));
                        let connector = if is_last { "└" } else { "├" };
                        ListItem::new(Line::from(vec![
                            Span::styled(connector, Style::default().fg(self.theme.project_icon)),
                            Span::styled(dot.to_string(), Style::default().fg(dot_color)),
                        ]))
                    }
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(self.selected_left));
            StatefulWidget::render(
                List::new(items)
                    .block(self.themed_block("", focused))
                    .highlight_style(self.theme.selection_style()),
                area,
                frame.buffer_mut(),
                &mut state,
            );
            return;
        }

        let session_counts: HashMap<String, usize> = {
            let mut counts = HashMap::new();
            for session in &self.sessions {
                *counts.entry(session.project_id.clone()).or_insert(0) += 1;
            }
            counts
        };
        let left_items = self.left_items();
        let items = left_items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                LeftItem::Project(index) => {
                    let project = &self.projects[*index];
                    let count = session_counts.get(&project.id).copied().unwrap_or(0);
                    let icon = if self.collapsed_projects.contains(&project.id) {
                        "▸ "
                    } else {
                        "▾ "
                    };
                    let mut spans = vec![
                        Span::styled(icon, Style::default().fg(self.theme.project_icon)),
                        Span::raw(project.name.clone()),
                    ];
                    if count > 0 {
                        spans.push(Span::styled(
                            format!(" ({count})"),
                            Style::default().fg(self.theme.provider_label_fg),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
                LeftItem::Session(index) => {
                    let session = &self.sessions[*index];
                    let is_last = !left_items
                        .get(i + 1)
                        .is_some_and(|next| matches!(next, LeftItem::Session(_)));
                    let connector = if is_last { "└ " } else { "├ " };
                    let label = session
                        .title
                        .clone()
                        .unwrap_or_else(|| session.branch_name.clone());
                    let (dot, dot_color) = self.theme.session_dot(&session.status);
                    ListItem::new(Line::from(vec![
                        Span::styled(connector, Style::default().fg(self.theme.project_icon)),
                        Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
                        Span::styled(label, Style::default().fg(dot_color)),
                        Span::styled(
                            format!(" ({})", session.provider.as_str()),
                            Style::default().fg(self.theme.provider_label_fg),
                        ),
                    ]))
                }
            })
            .collect::<Vec<_>>();
        let title = format!("Projects ({})", self.projects.len());
        let mut state = ListState::default().with_selected(Some(self.selected_left));
        StatefulWidget::render(
            List::new(items)
                .block(self.themed_block(&title, focused))
                .highlight_style(self.theme.selection_style()),
            area,
            frame.buffer_mut(),
            &mut state,
        );
    }

    fn render_center(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Center;
        match &self.center_mode {
            CenterMode::Diff { .. } => {
                self.render_diff(frame, area, focused);
            }
            CenterMode::Agent if self.fullscreen_agent => {
                // Skip agent rendering here — fullscreen overlay handles it.
                // Rendering in both places causes the PTY to be resized twice
                // per frame (once to the small pane, once to the overlay).
                self.themed_block("Agent", focused)
                    .render(area, frame.buffer_mut());
            }
            CenterMode::Agent => {
                self.render_agent_terminal(frame, area, "Agent", focused);
            }
        }
    }

    fn render_diff(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let (lines, scroll) = match &self.center_mode {
            CenterMode::Diff { lines, scroll } => (lines.clone(), *scroll),
            _ => return,
        };

        let outer_block = self.themed_block("Diff", focused);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 3 || inner.width < 4 {
            return;
        }

        let hint_height = 2;
        let [content_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);

        self.last_diff_height = content_area.height;

        // Compute visual line count accounting for wrapping.
        let w = content_area.width.max(1) as usize;
        self.last_diff_visual_lines = lines
            .iter()
            .map(|l| {
                let lw = l.width();
                if lw <= w {
                    1u16
                } else {
                    ((lw + w - 1) / w) as u16
                }
            })
            .sum();

        // Clamp scroll so content never overflows past the last visual line.
        let max_scroll = self
            .last_diff_visual_lines
            .saturating_sub(content_area.height);
        let scroll = scroll.min(max_scroll);

        Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .render(content_area, frame.buffer_mut());

        // Hint bar with top border (same style as agent terminal).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
                spans.push(Span::styled(" up. ", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
                spans.push(Span::styled(" ", desc_style));
                spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
                spans.push(Span::styled(" to scroll. ", desc_style));
            }
            spans.extend(self.theme.dim_key_badge(&close, Color::Reset));
            spans.push(Span::styled(" close diff.", desc_style));

            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    /// Render the ASCII "dux" logo centered in the given area.
    fn render_ascii_logo(&self, frame: &mut Frame, area: Rect) {
        if area.width < ASCII_LOGO_WIDTH || area.height < ASCII_LOGO_HEIGHT {
            return;
        }

        let x = area.x + (area.width - ASCII_LOGO_WIDTH) / 2;
        let y = area.y + (area.height - ASCII_LOGO_HEIGHT) / 2;
        let style = Style::default().fg(self.theme.border_normal);

        let lines: Vec<Line> = ASCII_LOGO.iter().map(|l| Line::styled(*l, style)).collect();
        Paragraph::new(lines).render(
            Rect::new(x, y, ASCII_LOGO_WIDTH, ASCII_LOGO_HEIGHT),
            frame.buffer_mut(),
        );
    }

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let outer_block = self.themed_block(title, focused);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let is_input = self.input_target == InputTarget::Agent;
        let mut scrollback_offset: usize = 0;
        let mut rendered_content = false;

        // Reserve 2 lines at the bottom for the hint bar (top border + text).
        let hint_height = 2;
        let [term_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);

        // Get the selected session's PTY screen.
        let session_id = self.selected_session().map(|s| s.id.clone());
        let session_provider_name = self
            .selected_session()
            .map(|s| s.provider.as_str().to_owned());
        let session_active = session_id
            .as_ref()
            .map(|id| self.providers.contains_key(id))
            .unwrap_or(false);

        if let Some(ref sid) = session_id {
            if let Some(provider) = self.providers.get(sid) {
                rendered_content = true;
                // Resize PTY if needed.
                let new_size = (term_area.height, term_area.width);
                if new_size != self.last_pty_size && new_size.0 > 0 && new_size.1 > 0 {
                    let _ = provider.resize(new_size.0, new_size.1);
                    self.last_pty_size = new_size;
                }

                if !provider.has_output() {
                    // Show a centered loading card until the PTY produces output.
                    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                    let idx = (self.tick_count as usize) % spinner_chars.len();
                    let spinner = spinner_chars[idx];
                    let (label_spans, label_len) = match session_provider_name.as_deref() {
                        Some(name) => {
                            let text_len = "Starting ".len() + name.len() + "...".len();
                            let spans = vec![
                                Span::styled(
                                    "Starting ",
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                                Span::styled(
                                    name.to_owned(),
                                    Style::default().fg(self.theme.branch_fg),
                                ),
                                Span::styled("...", Style::default().fg(self.theme.hint_desc_fg)),
                            ];
                            (spans, text_len)
                        }
                        None => {
                            let text = "Starting agent...";
                            let spans = vec![Span::styled(
                                text,
                                Style::default().fg(self.theme.hint_desc_fg),
                            )];
                            (spans, text.len())
                        }
                    };

                    // Card dimensions: border + padding + content + padding + border.
                    // +2 for spinner + space prefix.
                    let content_w = label_len as u16 + 2;
                    let card_w = (content_w + 2 + 6).min(term_area.width); // 2 borders + 6 padding
                    let card_h: u16 = 5; // top border, blank, spinner, blank, bottom border

                    if term_area.width >= card_w && term_area.height >= card_h {
                        let cx = term_area.x + (term_area.width - card_w) / 2;
                        let cy = term_area.y + (term_area.height - card_h) / 2;
                        let card_area = Rect::new(cx, cy, card_w, card_h);

                        let card_block = Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(Style::default().fg(self.theme.border_normal));
                        let card_inner = card_block.inner(card_area);
                        card_block.render(card_area, frame.buffer_mut());

                        // Render spinner + label centered inside the card.
                        let mut spans = vec![Span::styled(
                            format!("{spinner} "),
                            Style::default()
                                .fg(self.theme.hint_key_fg)
                                .add_modifier(Modifier::BOLD),
                        )];
                        spans.extend(label_spans);
                        let line = Line::from(spans);
                        Paragraph::new(line)
                            .alignment(ratatui::layout::Alignment::Center)
                            .render(
                                Rect::new(
                                    card_inner.x,
                                    card_inner.y + card_inner.height / 2,
                                    card_inner.width,
                                    1,
                                ),
                                frame.buffer_mut(),
                            );
                    }
                } else {
                    // Render the current terminal viewport into the ratatui
                    // buffer from an owned snapshot to avoid holding locks
                    // during painting.
                    let snapshot = provider.snapshot();
                    scrollback_offset = snapshot.scrollback_offset;
                    let buf = frame.buffer_mut();
                    for cell in &snapshot.cells {
                        if cell.row >= snapshot.rows
                            || cell.col >= snapshot.cols
                            || cell.row >= term_area.height
                            || cell.col >= term_area.width
                        {
                            continue;
                        }
                        let x = term_area.x + cell.col;
                        let y = term_area.y + cell.row;
                        let style = Style::default()
                            .fg(cell.fg)
                            .bg(cell.bg)
                            .add_modifier(cell.modifier);
                        let ratatui_cell = &mut buf[(x, y)];
                        ratatui_cell.set_symbol(&cell.symbol);
                        ratatui_cell.set_style(style);
                    }

                    // Render cursor if in input mode.
                    if is_input {
                        if let Some(cursor) = snapshot.cursor {
                            if cursor.row < snapshot.rows && cursor.col < snapshot.cols {
                                let cx = term_area.x + cursor.col;
                                let cy = term_area.y + cursor.row;
                                if cx < term_area.x + term_area.width
                                    && cy < term_area.y + term_area.height
                                {
                                    let cursor_cell = &mut buf[(cx, cy)];
                                    cursor_cell.set_style(
                                        Style::default()
                                            .fg(Color::Black)
                                            .bg(self.theme.prompt_cursor),
                                    );
                                }
                            }
                        }
                    }

                    if let Some(label) = scrollback_indicator_label(
                        snapshot.scrollback_offset,
                        snapshot.scrollback_total,
                    ) {
                        let badge_width = label.len() as u16;
                        if term_area.height > 0 && badge_width <= term_area.width {
                            Paragraph::new(label)
                                .style(
                                    Style::default()
                                        .fg(self.theme.scroll_indicator_fg)
                                        .bg(self.theme.scroll_indicator_bg),
                                )
                                .render(
                                    Rect::new(
                                        term_area.x + term_area.width - badge_width,
                                        term_area.y,
                                        badge_width,
                                        1,
                                    ),
                                    frame.buffer_mut(),
                                );
                        }
                    }
                }
            }
        }

        if !rendered_content {
            self.render_ascii_logo(frame, term_area);
        }

        // Hint bar with top border.
        if hint_area.height > 0 {
            // Pre-compute all key labels so they outlive the Span borrows.
            let exit_key = self.bindings.label_for(Action::ExitInteractive);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let focus_agent = self.bindings.labels_for(Action::FocusAgent);
            let reconnect = self.bindings.labels_for(Action::ReconnectAgent);

            let hint_line = if is_input {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                let cli_name = session_provider_name.as_deref().unwrap_or("agent");
                let capitalized = {
                    let mut c = cli_name.chars();
                    match c.next() {
                        Some(first) => format!("{}{}", first.to_uppercase(), c.as_str()),
                        None => String::new(),
                    }
                };
                spans.push(Span::styled(
                    format!("{capitalized} is holding focus. Press "),
                    desc_style,
                ));
                spans.extend(self.theme.dim_key_badge(&exit_key, Color::Reset));
                spans.push(Span::styled(" to return to the app. ", desc_style));
                spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
                if scrollback_offset > 0 {
                    spans.push(Span::styled(" page down, or ", desc_style));
                    spans.extend(self.theme.dim_key_badge("Space", Color::Reset));
                    spans.push(Span::styled(" down one line.", desc_style));
                } else {
                    spans.push(Span::styled(" page down.", desc_style));
                }
                Line::from(spans)
            } else if scrollback_offset > 0 {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(
                    format!("Scrolled back {scrollback_offset} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
                spans.push(Span::styled(" up.", desc_style));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if session_active {
                    spans.extend(self.theme.dim_key_badge(&focus_agent, Color::Reset));
                    spans.push(Span::styled(" to interact. ", desc_style));
                    spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
                    spans.push(Span::styled(" to scroll.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge(&reconnect, Color::Reset));
                    spans.push(Span::styled(" to relaunch or ", desc_style));
                    spans.extend(self.theme.dim_key_badge(&focus_agent, Color::Reset));
                    spans.push(Span::styled(" to interact.", desc_style));
                } else {
                    spans.push(Span::styled("No agent selected.", desc_style));
                }
                Line::from(spans)
            };
            Paragraph::new(hint_line)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    fn render_files(&mut self, frame: &mut Frame, area: Rect) {
        let has_staged = !self.staged_files.is_empty();
        let focused = self.focus == FocusPane::Files;

        if has_staged {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3), // Changes (unstaged) — always on top
                    Constraint::Min(3), // Staged Changes (with commit input)
                ])
                .split(area);
            self.render_file_list(
                frame,
                chunks[0],
                "Changes",
                &self.unstaged_files,
                RightSection::Unstaged,
                focused,
                true,
            );
            self.render_staged_with_commit(frame, chunks[1], focused);
        } else {
            self.render_file_list(
                frame,
                area,
                "Changes",
                &self.unstaged_files,
                RightSection::Unstaged,
                focused,
                true,
            );
        }
    }

    /// Render the "Staged Changes" file list and the commit input as two
    /// separate bordered blocks (bubbles).
    fn render_staged_with_commit(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        let commit_height = 10u16;
        let [files_area, commit_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(commit_height)])
            .areas(area);

        // Staged files — normal titled block.
        self.render_file_list(
            frame,
            files_area,
            "Staged Changes",
            &self.staged_files,
            RightSection::Staged,
            pane_focused,
            false,
        );

        // Commit input block.
        self.render_commit_input_inner(frame, commit_area, pane_focused);
    }

    fn render_file_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        title_prefix: &str,
        files: &[ChangedFile],
        section: RightSection,
        pane_focused: bool,
        show_hint: bool,
    ) {
        let is_active_section = pane_focused && self.right_section == section;
        let title = format!("{title_prefix} ({})", files.len());
        let block = self.themed_block(&title, is_active_section);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        // Optionally reserve 2 lines at the bottom for the hint bar (border + text).
        let (list_area, hint_area) = if show_hint && pane_focused && inner.height >= 4 {
            let [la, ha] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(2)])
                .areas(inner);
            (la, Some(ha))
        } else {
            (inner, None)
        };

        let inner_width = list_area.width as usize;
        let sel_style = self.theme.selection_style();
        let items = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let is_selected = is_active_section && index == self.files_index;

                // Build the right-aligned stats string, e.g. "+12 -3".
                let stats = format_line_stats(file.additions, file.deletions);
                let stats_width = stats.iter().map(|s| s.width()).sum::<usize>();

                // Status prefix takes 3 chars ("M  ").
                let prefix_width = 3;
                // Leave 1 char gap between path and stats.
                let path_budget = inner_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(stats_width)
                    .saturating_sub(1);

                let path = if is_selected {
                    file.path.clone()
                } else {
                    git::ellipsize_middle(&file.path, path_budget.max(10))
                };

                let path_display_width = path.chars().count();
                let padding = inner_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(path_display_width)
                    .saturating_sub(stats_width);

                let base_style = if is_selected {
                    sel_style
                } else {
                    Style::default()
                };

                let mut spans = vec![
                    Span::styled(
                        format!("{:>2} ", file.status),
                        base_style.fg(self.theme.file_status_fg),
                    ),
                    Span::styled(path, base_style),
                    Span::styled(" ".repeat(padding), base_style),
                ];
                // For stats spans, keep their green/red fg but apply selection bg when selected.
                let stats_base = if is_selected {
                    Style::default()
                        .bg(self.theme.selection_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.extend(stats.into_iter().map(|s| {
                    let fg = s.style.fg.unwrap_or(Color::Reset);
                    Span::styled(s.content, stats_base.fg(fg))
                }));
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>();
        let selected = if is_active_section {
            Some(self.files_index)
        } else {
            None
        };
        let mut state = ListState::default().with_selected(selected);
        StatefulWidget::render(List::new(items), list_area, frame.buffer_mut(), &mut state);

        // Hint bar inside the block (same style as agent terminal / diff view).
        if let Some(ha) = hint_area {
            let stage_key = self.bindings.label_for(Action::StageUnstage);
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let mut spans: Vec<Span> = Vec::new();
            spans.extend(self.theme.dim_key_badge(&stage_key, Color::Reset));
            spans.push(Span::styled(" stage/unstage.", desc_style));
            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(ha, frame.buffer_mut());
        }
    }

    /// Render the commit input as its own bordered block.
    fn render_commit_input_inner(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        let is_active_section = pane_focused && self.right_section == RightSection::CommitInput;
        let focused = self.input_target == InputTarget::CommitMessage;

        let block = self.themed_block("Commit Message", is_active_section || focused);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        // Reserve 1 line at the bottom for the hint bar.
        let [text_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .areas(inner);

        if self.commit_generating {
            let dots = ".".repeat((self.tick_count as usize / 5) % 4);
            let text = format!("Generating commit message{dots}");
            Paragraph::new(text)
                .style(Style::default().fg(self.theme.hint_desc_fg))
                .render(text_area, frame.buffer_mut());
        } else if self.commit_input.is_empty() && !focused {
            // Placeholder text when not engaged.
        } else {
            let display_text = if self.commit_input.is_empty() {
                "Type your commit message…"
            } else {
                &self.commit_input
            };
            let style = if self.commit_input.is_empty() {
                Style::default().fg(self.theme.hint_desc_fg)
            } else {
                Style::default()
            };

            let visible_h = text_area.height;
            let text_w = text_area.width as usize;
            if text_w > 0 && !self.commit_input.is_empty() {
                let (row, _) =
                    cursor_pos_in_wrapped(&self.commit_input, self.commit_input_cursor, text_w);
                if row < self.commit_scroll {
                    self.commit_scroll = row;
                } else if row >= self.commit_scroll + visible_h {
                    self.commit_scroll = row - visible_h + 1;
                }
            } else {
                self.commit_scroll = 0;
            }

            let wrapped_text = wrap_text_at_width(display_text, text_w);
            Paragraph::new(wrapped_text)
                .style(style)
                .scroll((self.commit_scroll, 0))
                .render(text_area, frame.buffer_mut());

            if focused {
                let (cursor_row, cursor_col) =
                    cursor_pos_in_wrapped(&self.commit_input, self.commit_input_cursor, text_w);
                let screen_row = cursor_row.saturating_sub(self.commit_scroll);
                let cx = text_area.x + cursor_col as u16;
                let cy = text_area.y + screen_row;
                if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
                    frame.set_cursor_position((cx, cy));
                }
            }
        }

        // Hint bar.
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let exit = self.bindings.labels_for(Action::ExitCommitInput);
            let engage = self.bindings.labels_for(Action::EngageCommitInput);
            let ai_msg = self.bindings.label_for(Action::GenerateCommitMessage);
            let commit = self.bindings.label_for(Action::CommitChanges);
            let mut spans: Vec<Span> = Vec::new();
            if focused {
                spans.extend(self.theme.dim_key_badge(&exit, Color::Reset));
                spans.push(Span::styled(" Exit", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge(&engage, Color::Reset));
                spans.push(Span::styled(" Edit  ", desc_style));
                spans.extend(self.theme.dim_key_badge(&ai_msg, Color::Reset));
                spans.push(Span::styled(" AI msg  ", desc_style));
                spans.extend(self.theme.dim_key_badge(&commit, Color::Reset));
                spans.push(Span::styled(" Commit", desc_style));
            }
            Paragraph::new(Line::from(spans)).render(hint_area, frame.buffer_mut());
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let is_on_project = !matches!(
            self.left_items().get(self.selected_left),
            Some(LeftItem::Session(_))
        );
        let ctx = match self.focus {
            FocusPane::Left if is_on_project => HintContext::LeftProject,
            FocusPane::Left => HintContext::LeftSession,
            FocusPane::Center => HintContext::Center,
            FocusPane::Files => HintContext::Files,
        };
        let hints = self.bindings.hints_for(ctx);
        let [hints_area, status_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(area);
        let max_w = hints_area.width as usize;
        let ellipsis = "…";
        let ellipsis_w = 1;

        let mut hint_spans: Vec<Span> = Vec::new();
        let bar_bg = self.theme.hint_bar_bg;
        let mut used = 0usize;
        for (i, (key, desc)) in hints.iter().enumerate() {
            // width of this hint: separator + <key> + space + desc
            let sep = if i > 0 { 1 } else { 0 };
            let hint_w = sep + key.len() + 2 + 1 + desc.len();
            if used + hint_w > max_w {
                if used + ellipsis_w <= max_w {
                    hint_spans.push(Span::styled(
                        ellipsis,
                        Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
                    ));
                }
                break;
            }
            if i > 0 {
                hint_spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            hint_spans.extend(self.theme.key_badge(key, bar_bg));
            hint_spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
            ));
            used += hint_w;
        }

        Paragraph::new(Line::from(hint_spans))
            .style(Style::default().bg(self.theme.hint_bar_bg))
            .render(hints_area, frame.buffer_mut());

        let tone = self.status.tone();
        let (dot, dot_color) = self.theme.status_dot(tone);
        let status_text = self.status.text();
        let msg_color = match tone {
            StatusTone::Info => self.theme.status_info_fg,
            StatusTone::Busy => self.theme.status_busy_fg,
            StatusTone::Error => self.theme.status_error_fg,
        };
        let status_bg = match tone {
            StatusTone::Info => self.theme.status_info_bg,
            StatusTone::Busy => self.theme.status_busy_bg,
            StatusTone::Error => self.theme.status_error_bg,
        };
        let prefix = format!(" {dot} ");
        let prefix_w = prefix.len();
        let max_status_chars = (status_area.width as usize) * (status_area.height as usize);
        let available = max_status_chars.saturating_sub(prefix_w);
        let truncated = if status_text.len() > available && available > 1 {
            format!("{}…", &status_text[..available - 1])
        } else {
            status_text
        };
        let status_line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(dot_color).bg(status_bg)),
            Span::styled(truncated, Style::default().fg(msg_color).bg(status_bg)),
        ]);
        Paragraph::new(status_line)
            .style(Style::default().bg(status_bg))
            .wrap(Wrap { trim: false })
            .render(status_area, frame.buffer_mut());
    }

    fn render_help(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(72, 70, frame.area());
        Clear.render(area, frame.buffer_mut());

        let outer_block = self.themed_overlay_block("Help");
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 3 || inner.width < 4 {
            return;
        }

        let hint_height = 2;
        let [content_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);

        // Build help content lines.
        let mut lines: Vec<Line> = Vec::new();

        // Config banner
        lines.push(Line::from(vec![
            Span::styled(
                "All keybindings are configurable. See ",
                Style::default().fg(self.theme.hint_desc_fg),
            ),
            Span::styled(
                self.paths.config_path.display().to_string(),
                Style::default()
                    .fg(self.theme.hint_key_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let help_bindings = self.bindings.help_sections();
        for (section, bindings) in &help_bindings {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                section.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for (key, desc) in bindings {
                let padding = 14usize.saturating_sub(key.len() + 2);
                let mut spans = vec![Span::raw("  ")];
                spans.extend(self.theme.key_badge(key, Color::Reset));
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                lines.push(Line::from(spans));
            }
        }
        // Key notation legend
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Key notation",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            let key = "Ctrl-x";
            let desc = "Hold Ctrl and press X (e.g. Ctrl-p)";
            let padding = 14usize.saturating_sub(key.len() + 2);
            let mut spans = vec![Span::raw("  ")];
            spans.extend(self.theme.key_badge(key, Color::Reset));
            spans.push(Span::raw(" ".repeat(padding)));
            spans.push(Span::styled(
                desc,
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            lines.push(Line::from(spans));
        }
        // Session state legend
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Session states",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        let session_states: &[(&str, Color, &str)] = &[
            ("●", self.theme.session_active, "Active — agent is running"),
            (
                "◐",
                self.theme.session_detached,
                "Detached — agent process disconnected",
            ),
            (
                "○",
                self.theme.session_exited,
                "Exited — agent has finished",
            ),
        ];
        for (dot, color, desc) in session_states {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(*dot, Style::default().fg(*color)),
                Span::raw("  "),
                Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ),
            ]));
        }

        // Track content size for scroll clamping in input handler.
        let total_lines = lines.len() as u16;
        self.last_help_lines = total_lines;
        self.last_help_height = content_area.height;

        // Clamp scroll offset.
        let max_scroll = total_lines.saturating_sub(content_area.height);
        let scroll = self.help_scroll.unwrap_or(0).min(max_scroll);

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .render(content_area, frame.buffer_mut());

        // Hint bar with top border (same pattern as diff view).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let move_down = self.bindings.label_for(Action::MoveDown);
            let move_up = self.bindings.label_for(Action::MoveUp);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
            }
            spans.extend(self.theme.dim_key_badge(&move_down, Color::Reset));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge(&move_up, Color::Reset));
            spans.push(Span::styled(" scroll, ", desc_style));
            spans.extend(self.theme.dim_key_badge(&scroll_down, Color::Reset));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge(&scroll_up, Color::Reset));
            spans.push(Span::styled(" page. ", desc_style));
            spans.extend(self.theme.dim_key_badge(&close, Color::Reset));
            spans.push(Span::styled(" close.", desc_style));

            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    fn render_prompt(&self, frame: &mut Frame) {
        match &self.prompt {
            PromptState::Command {
                input,
                selected,
                searching,
            } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(72, 40, frame.area());
                Clear.render(popup, frame.buffer_mut());
                let commands = self.bindings.filtered_palette(input);
                let items = if commands.is_empty() {
                    vec![ListItem::new("No matching commands.")]
                } else {
                    let name_col = commands
                        .iter()
                        .map(|b| b.palette_name.unwrap().len())
                        .max()
                        .unwrap_or(0);
                    let inner_w = popup.width as usize - 3;
                    let gap = 2usize;
                    commands
                        .iter()
                        .map(|binding| {
                            let name = binding.palette_name.unwrap();
                            let name_padded = format!("{name:name_col$}");
                            let mut spans = vec![Span::styled(
                                name_padded,
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            )];
                            let desc_avail = inner_w.saturating_sub(name_col + gap);
                            let desc = binding.palette_description.unwrap_or("");
                            let desc_display = if desc.len() > desc_avail && desc_avail > 1 {
                                format!("  {}\u{2026}", &desc[..desc_avail - 1])
                            } else {
                                format!("  {desc:desc_avail$}")
                            };
                            spans.push(Span::styled(
                                desc_display,
                                Style::default().fg(self.theme.hint_desc_fg),
                            ));
                            ListItem::new(Line::from(spans))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(commands.len().saturating_sub(1))));
                let [input_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(3)])
                    .areas(popup);
                let title = if *searching {
                    "Command Palette (searching)"
                } else {
                    "Command Palette"
                };
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let search_key = self.bindings.label_for(Action::SearchToggle);
                let mut bottom_spans = vec![Span::raw(" ")];
                if *searching {
                    bottom_spans.extend(self.theme.key_badge(&confirm_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " done  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    bottom_spans.extend(self.theme.key_badge(&search_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&confirm_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " run  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    // Tab autocomplete is text-input behavior, not a rebindable action.
                    bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " complete  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
                let prompt_prefix = if *searching { "/ " } else { "> " };
                Paragraph::new(format!("{prompt_prefix}{input}"))
                    .block(
                        self.themed_overlay_block(title)
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(input_area, frame.buffer_mut());
                StatefulWidget::render(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                                .border_type(ratatui::widgets::BorderType::Rounded)
                                .border_style(Style::default().fg(self.theme.overlay_border)),
                        )
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
            }
            PromptState::BrowseProjects {
                current_dir,
                entries,
                loading,
                selected,
                filter,
                searching,
                editing_path,
                path_input,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 70, frame.area());
                Clear.render(area, frame.buffer_mut());
                let visible: Vec<_> = if filter.is_empty() {
                    entries.iter().collect()
                } else {
                    let needle = filter.to_lowercase();
                    entries
                        .iter()
                        .filter(|e| e.label.to_lowercase().contains(&needle))
                        .collect()
                };
                let items = if *loading {
                    let spinner = match (self.tick_count / 2) % 4 {
                        0 => "⠋",
                        1 => "⠙",
                        2 => "⠹",
                        _ => "⠸",
                    };
                    vec![ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{spinner} "),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::raw("Loading…"),
                    ]))]
                } else if visible.is_empty() {
                    vec![ListItem::new(if filter.is_empty() {
                        "No child directories here."
                    } else {
                        "No matching entries."
                    })]
                } else {
                    let last = visible.len() - 1;
                    visible
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            let prefix = if entry.label == "../" {
                                ""
                            } else if i == last {
                                "└── "
                            } else {
                                "├── "
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    prefix.to_string(),
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                                Span::raw(entry.label.clone()),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(visible.len().saturating_sub(1))));
                let has_filter = !filter.is_empty();
                let show_top_input = *searching || has_filter || *editing_path;
                let (top_areas, list_render_area) = if show_top_input {
                    let [filter_area, list_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(3)])
                        .areas(area);
                    (Some(filter_area), list_area)
                } else {
                    (None, area)
                };
                if let Some(filter_area) = top_areas {
                    let title = format!("Add Project: {}", current_dir.display());
                    let input_text = if *editing_path {
                        format!("go: {path_input}█")
                    } else {
                        format!("/ {filter}")
                    };
                    Paragraph::new(input_text)
                        .block(self.themed_overlay_block(&title))
                        .render(filter_area, frame.buffer_mut());
                    let confirm_key = self.bindings.label_for(Action::Confirm);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    if *editing_path {
                        // Path editor: Tab/Enter/Esc are text-input controls, not rebindable.
                        bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " complete  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " go  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else if *searching {
                        bottom_spans.extend(self.theme.key_badge(&confirm_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " done  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " clear",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else {
                        bottom_spans.extend(self.theme.key_badge(&search_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " search  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge(&open_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " open  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge(&goto_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " go to  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    }
                    StatefulWidget::render(
                        List::new(items)
                            .block(
                                Block::default()
                                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                                    .border_style(Style::default().fg(self.theme.overlay_border))
                                    .title_bottom(Line::from(bottom_spans)),
                            )
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                } else {
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let add_key = self.bindings.label_for(Action::AddCurrentDir);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    bottom_spans.extend(self.theme.key_badge(&search_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&open_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " open  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&add_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " add current  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&goto_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " go to  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    StatefulWidget::render(
                        List::new(items)
                            .block(
                                self.themed_overlay_block(&format!(
                                    "Add Project: {}",
                                    current_dir.display()
                                ))
                                .title_bottom(Line::from(bottom_spans)),
                            )
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                }
            }
            PromptState::PickEditor {
                session_label,
                worktree_path,
                editors,
                selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(64, 34, frame.area());
                Clear.render(area, frame.buffer_mut());

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge(&move_down, Color::Reset));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge(&move_up, Color::Reset));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge(&confirm_key, Color::Reset));
                bottom_spans.push(Span::styled(
                    " open  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge(&close_key, Color::Reset));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(4)])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Agent: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            session_label.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Path: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::raw(worktree_path.as_str()),
                    ]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Open Worktree In")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let configured_default = self.config.editor.default.trim();
                let items = editors
                    .iter()
                    .map(|editor| {
                        let mut spans = vec![Span::styled(
                            format!("{:<14}", editor.label),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )];
                        spans.push(Span::styled(
                            format!(" {}", editor.command),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        if crate::editor::matches_configured_editor(editor, configured_default) {
                            spans.push(Span::styled(
                                "  default",
                                Style::default().fg(self.theme.branch_fg),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(editors.len().saturating_sub(1))));
                StatefulWidget::render(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                                .border_style(Style::default().fg(self.theme.overlay_border)),
                        )
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
            }
            PromptState::ConfirmDeleteAgent {
                branch_name,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                // Outer dialog: border + title.
                let area = centered_rect(56, 30, frame.area());
                Clear.render(area, frame.buffer_mut());
                let outer = self.themed_overlay_block("Delete Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                // Body text.
                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Are you sure you want to delete "),
                        Span::styled(
                            branch_name.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " All uncommitted and unpushed changes in this",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::styled(
                        " worktree will be permanently lost.",
                        Style::default().fg(Color::Yellow),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                // Button area: two bordered panels side by side.
                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let (cancel_border, cancel_fg) = if !confirm_selected {
                    (Color::Cyan, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };
                let (delete_border, delete_fg) = if *confirm_selected {
                    (Color::Red, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };

                Paragraph::new(Line::from(Span::styled(
                    "Cancel",
                    Style::default().fg(cancel_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(cancel_border)),
                )
                .render(cancel_area, frame.buffer_mut());

                Paragraph::new(Line::from(Span::styled(
                    "Delete",
                    Style::default().fg(delete_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(delete_border)),
                )
                .render(delete_area, frame.buffer_mut());
            }
            PromptState::ConfirmQuit {
                active_count,
                confirm_selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                Clear.render(area, frame.buffer_mut());
                let outer = self.themed_overlay_block("Quit dux");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let agent_word = if *active_count == 1 {
                    "agent"
                } else {
                    "agents"
                };
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(" {active_count} running {agent_word} will be ")),
                        Span::styled(
                            "killed",
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" if you quit."),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Any in-progress work by those agents will be lost.",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::styled(
                        " File changes in worktrees are preserved.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let quit_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let (cancel_border, cancel_fg) = if !confirm_selected {
                    (Color::Cyan, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };
                let (quit_border, quit_fg) = if *confirm_selected {
                    (Color::Red, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };

                Paragraph::new(Line::from(Span::styled(
                    "Cancel",
                    Style::default().fg(cancel_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(cancel_border)),
                )
                .render(cancel_area, frame.buffer_mut());

                Paragraph::new(Line::from(Span::styled(
                    "Quit",
                    Style::default().fg(quit_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(quit_border)),
                )
                .render(quit_area, frame.buffer_mut());
            }
            PromptState::ConfirmDiscardFile {
                file_path,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                Clear.render(area, frame.buffer_mut());
                let outer = self.themed_overlay_block("Discard Changes");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Discard all changes to \""),
                        Span::styled(
                            file_path.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("\"?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " This action cannot be undone.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let discard_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let (cancel_border, cancel_fg) = if !confirm_selected {
                    (
                        self.theme.button_confirm_border,
                        self.theme.button_active_fg,
                    )
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };
                let (discard_border, discard_fg) = if *confirm_selected {
                    (self.theme.button_danger_border, self.theme.button_active_fg)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };

                Paragraph::new(Line::from(Span::styled(
                    "Cancel",
                    Style::default().fg(cancel_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(cancel_border)),
                )
                .render(cancel_area, frame.buffer_mut());

                Paragraph::new(Line::from(Span::styled(
                    "Discard",
                    Style::default().fg(discard_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(discard_border)),
                )
                .render(discard_area, frame.buffer_mut());
            }
            PromptState::RenameSession { input, cursor, .. } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 14, frame.area());
                Clear.render(area, frame.buffer_mut());

                let outer = self.themed_overlay_block("Rename Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, _, input_area, _, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                Paragraph::new(Line::from(Span::styled(
                    " Enter a new name (empty to reset):",
                    Style::default().fg(Color::White),
                )))
                .render(label_area, frame.buffer_mut());

                // Show the input with a cursor indicator.
                let display = if *cursor < input.len() {
                    let (before, after) = input.split_at(*cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default().fg(Color::Black).bg(Color::White),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {input}")),
                        Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
                    ])
                };
                Paragraph::new(display)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_set(border::ROUNDED)
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .render(input_area, frame.buffer_mut());

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge(&confirm_key, Color::Reset));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge(&close_key, Color::Reset));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
            }
            PromptState::None => {}
        }
    }

    fn render_overlay(&mut self, frame: &mut Frame) {
        if self.fullscreen_agent {
            self.render_fullscreen_agent(frame);
            return;
        }
        if !matches!(self.prompt, PromptState::None) {
            self.render_prompt(frame);
            return;
        }
        if self.help_scroll.is_some() {
            self.render_help(frame);
        }
    }

    fn render_fullscreen_agent(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(96, 94, frame.area());
        Clear.render(area, frame.buffer_mut());
        self.render_agent_terminal(frame, area, " Agent (fullscreen) ", true);
    }

    fn themed_block<'a>(&self, title: &'a str, focused: bool) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                self.theme.title_style(focused),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(self.theme.border_style(focused))
    }

    fn themed_overlay_block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border))
    }

    fn render_dim_overlay(&self, frame: &mut Frame) {
        let full = frame.area();
        // Keep the statusline (bottom rows) undimmed so errors stay visible.
        let status_text_len = self.status.text().len() + 3;
        let status_lines: u16 = if full.width > 0 && status_text_len > full.width as usize {
            2
        } else {
            1
        };
        let footer_h = 1 + status_lines; // hints bar + status line(s)
        let dim_h = full.height.saturating_sub(footer_h);
        let buf = frame.buffer_mut();
        for y in full.y..full.y + dim_h {
            for x in full.x..full.x + full.width {
                let cell = &mut buf[(x, y)];
                cell.set_fg(Color::DarkGray);
                cell.set_bg(Color::Rgb(10, 10, 10));
            }
        }
    }
}

/// Format additions/deletions as right-aligned colored spans.
/// Returns an empty vec when both counts are zero.
pub(crate) fn format_line_stats(additions: usize, deletions: usize) -> Vec<Span<'static>> {
    if additions == 0 && deletions == 0 {
        return Vec::new();
    }
    let mut spans = Vec::new();
    if additions > 0 {
        spans.push(Span::styled(
            format!("+{additions}"),
            Style::default().fg(Color::Green),
        ));
    }
    if additions > 0 && deletions > 0 {
        spans.push(Span::raw(" "));
    }
    if deletions > 0 {
        spans.push(Span::styled(
            format!("-{deletions}"),
            Style::default().fg(Color::Red),
        ));
    }
    spans
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical)[1]
}

fn scrollback_indicator_label(scrolled: usize, total: usize) -> Option<String> {
    if scrolled == 0 {
        return None;
    }

    let total = total.max(scrolled);
    let noun = if total == 1 { "line" } else { "lines" };
    Some(format!(" {scrolled}/{total} {noun} "))
}

/// Pre-wrap text at exact character boundaries to match the manual cursor
/// position calculation used in the commit input box.
fn wrap_text_at_width(text: &str, width: usize) -> String {
    if width == 0 {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len() + text.len() / width);
    let mut col: usize = 0;
    for ch in text.chars() {
        if ch == '\n' {
            result.push('\n');
            col = 0;
        } else {
            col += 1;
            result.push(ch);
            if col >= width {
                result.push('\n');
                col = 0;
            }
        }
    }
    result
}

/// Compute the (row, col) position of a cursor in text that wraps at `width`.
/// This mirrors the inline cursor calculation used in `render_commit_input_inner`.
fn cursor_pos_in_wrapped(text: &str, cursor: usize, width: usize) -> (u16, usize) {
    let mut row: u16 = 0;
    let mut col: usize = 0;
    for (i, ch) in text.char_indices() {
        if i == cursor {
            break;
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
            if width > 0 && col >= width {
                row += 1;
                col = 0;
            }
        }
    }
    (row, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    // ── Unit tests for wrap_text_at_width ──────────────────────────

    #[test]
    fn wrap_empty_string() {
        assert_eq!(wrap_text_at_width("", 10), "");
    }

    #[test]
    fn scrollback_indicator_uses_fractional_label() {
        assert_eq!(
            scrollback_indicator_label(41, 800),
            Some(" 41/800 lines ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_handles_singular_total() {
        assert_eq!(
            scrollback_indicator_label(1, 1),
            Some(" 1/1 line ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_hides_at_live_bottom() {
        assert_eq!(scrollback_indicator_label(0, 800), None);
    }

    #[test]
    fn wrap_shorter_than_width() {
        assert_eq!(wrap_text_at_width("hello", 10), "hello");
    }

    #[test]
    fn wrap_exact_width() {
        // Exactly 5 chars at width 5 → wraps after the last char.
        assert_eq!(wrap_text_at_width("abcde", 5), "abcde\n");
    }

    #[test]
    fn wrap_longer_than_width() {
        assert_eq!(wrap_text_at_width("abcdefgh", 5), "abcde\nfgh");
    }

    #[test]
    fn wrap_multiple_lines() {
        assert_eq!(wrap_text_at_width("abcdefghij", 3), "abc\ndef\nghi\nj");
    }

    #[test]
    fn wrap_preserves_existing_newlines() {
        assert_eq!(wrap_text_at_width("ab\ncdefgh", 5), "ab\ncdefg\nh");
    }

    #[test]
    fn wrap_newline_resets_column() {
        // "abcde" fills width 5, then "\n" resets, then "fg" fits.
        assert_eq!(wrap_text_at_width("abcde\nfg", 5), "abcde\n\nfg");
    }

    #[test]
    fn wrap_width_one() {
        assert_eq!(wrap_text_at_width("abc", 1), "a\nb\nc\n");
    }

    #[test]
    fn wrap_width_zero_returns_unchanged() {
        assert_eq!(wrap_text_at_width("abc", 0), "abc");
    }

    // ── Unit tests for cursor_pos_in_wrapped ───────────────────────

    #[test]
    fn cursor_at_start() {
        assert_eq!(cursor_pos_in_wrapped("hello", 0, 10), (0, 0));
    }

    #[test]
    fn cursor_mid_line() {
        assert_eq!(cursor_pos_in_wrapped("hello", 3, 10), (0, 3));
    }

    #[test]
    fn cursor_at_wrap_boundary() {
        // "abcde" at width 5: after 'e' col hits 5, wraps → cursor at (1, 0).
        assert_eq!(cursor_pos_in_wrapped("abcdefgh", 5, 5), (1, 0));
    }

    #[test]
    fn cursor_after_wrap() {
        assert_eq!(cursor_pos_in_wrapped("abcdefgh", 6, 5), (1, 1));
    }

    #[test]
    fn cursor_after_newline() {
        assert_eq!(cursor_pos_in_wrapped("ab\ncd", 3, 10), (1, 0));
    }

    #[test]
    fn cursor_at_end() {
        // Cursor past last char (len = 5), sits at (1, 0) after wrapping.
        assert_eq!(cursor_pos_in_wrapped("abcde", 5, 5), (1, 0));
    }

    // ── Consistency: cursor pos matches wrapped text layout ────────

    /// For every possible cursor position in `text`, verify that the (row, col)
    /// from `cursor_pos_in_wrapped` points to the correct character in the
    /// output of `wrap_text_at_width`.
    fn assert_cursor_wrap_consistency(text: &str, width: usize) {
        let wrapped = wrap_text_at_width(text, width);
        let wrapped_lines: Vec<&str> = wrapped.split('\n').collect();

        for cursor in 0..=text.len() {
            // Only test at char boundaries.
            if !text.is_char_boundary(cursor) {
                continue;
            }
            let (row, col) = cursor_pos_in_wrapped(text, cursor, width);
            let row = row as usize;

            // The cursor should be within the wrapped output's line count.
            assert!(
                row < wrapped_lines.len(),
                "text={text:?} width={width} cursor={cursor}: row {row} >= line count {}",
                wrapped_lines.len()
            );

            let line = wrapped_lines[row];

            let at_char = text[cursor..].chars().next();
            if at_char == Some('\n') || at_char.is_none() {
                // Cursor at a newline or end of text: sits at end of current line.
                assert!(
                    col <= line.len(),
                    "text={text:?} width={width} cursor={cursor}: \
                     col {col} > line len {} at row {row}",
                    line.len()
                );
            } else {
                // Cursor points at a visible character; it should match.
                let expected_char = at_char.unwrap();
                let actual_char = line[col..].chars().next().unwrap_or('\0');
                assert_eq!(
                    actual_char, expected_char,
                    "text={text:?} width={width} cursor={cursor}: \
                     at ({row},{col}) expected {expected_char:?} got {actual_char:?}\n\
                     wrapped={wrapped:?}"
                );
            }
        }
    }

    #[test]
    fn consistency_short_text() {
        assert_cursor_wrap_consistency("hello", 10);
    }

    #[test]
    fn consistency_exact_width() {
        assert_cursor_wrap_consistency("abcde", 5);
    }

    #[test]
    fn consistency_wrapping_text() {
        assert_cursor_wrap_consistency("abcdefghijklmno", 5);
    }

    #[test]
    fn consistency_with_newlines() {
        assert_cursor_wrap_consistency("abc\ndef\nghi", 5);
    }

    #[test]
    fn consistency_mixed_wrap_and_newlines() {
        assert_cursor_wrap_consistency("abcdefg\nhij", 4);
    }

    #[test]
    fn consistency_width_one() {
        assert_cursor_wrap_consistency("abc", 1);
    }

    #[test]
    fn consistency_long_commit_message() {
        let msg = "fix: align commit input text wrapping with cursor position calculation for correctness";
        for w in 5..30 {
            assert_cursor_wrap_consistency(msg, w);
        }
    }

    // ── Ratatui TestBackend rendering test ──────────────────────────

    /// Render wrapped text into a Ratatui TestBackend and verify the character
    /// at the calculated cursor position matches the expected character.
    #[test]
    fn rendered_cursor_matches_buffer_content() {
        let text = "abcdefghijklmno";
        let width: u16 = 5;
        let height: u16 = 4;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        // Test cursor at several positions including wrap boundaries.
        for cursor in [0usize, 4, 5, 7, 10, 14] {
            let wrapped = wrap_text_at_width(text, width as usize);
            let (crow, ccol) = cursor_pos_in_wrapped(text, cursor, width as usize);

            terminal
                .draw(|frame| {
                    let area = frame.area();
                    Paragraph::new(wrapped.as_str()).render(area, frame.buffer_mut());
                })
                .unwrap();

            let buf = terminal.backend().buffer();
            let cell = buf.cell((ccol as u16, crow as u16)).unwrap();
            let expected = &text[cursor..cursor + 1];
            assert_eq!(
                cell.symbol(),
                expected,
                "cursor={cursor} at ({crow},{ccol}): buffer has {:?}, expected {expected:?}",
                cell.symbol()
            );
        }
    }

    /// Verify that after deleting a character near a wrap boundary, the cursor
    /// still points to the correct cell in the rendered buffer.
    #[test]
    fn cursor_correct_after_deletion_near_wrap() {
        let width: u16 = 5;
        let height: u16 = 4;

        // Simulate: text is "abcdefgh", cursor at 5 ('f'), delete → "abcdegh", cursor at 5 ('g').
        let original = "abcdefgh";
        let delete_at = 5; // byte index of 'f'
        let after_delete = format!("{}{}", &original[..delete_at], &original[delete_at + 1..]);
        let new_cursor = delete_at; // cursor stays at same byte pos, now pointing at 'g'

        let wrapped = wrap_text_at_width(&after_delete, width as usize);
        let (crow, ccol) = cursor_pos_in_wrapped(&after_delete, new_cursor, width as usize);

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                Paragraph::new(wrapped.as_str()).render(area, frame.buffer_mut());
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let cell = buf.cell((ccol as u16, crow as u16)).unwrap();
        let expected_char = after_delete[new_cursor..].chars().next().unwrap();
        assert_eq!(
            cell.symbol(),
            expected_char.to_string(),
            "After deletion: cursor at ({crow},{ccol}) should show {expected_char:?}, got {:?}",
            cell.symbol()
        );
    }
}
