use super::*;

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

        Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .render(content_area, frame.buffer_mut());

        // Hint bar with top border (same style as agent terminal).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge("ctrl+f", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge("ctrl+b", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                spans.push(Span::styled(" up. ", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge("^B", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                spans.push(Span::styled(" ", desc_style));
                spans.extend(self.theme.dim_key_badge("^F", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                spans.push(Span::styled(" to scroll. ", desc_style));
            }
            spans.extend(self.theme.dim_key_badge("Esc", Color::Reset));
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

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let outer_block = self.themed_block(title, focused);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let is_input = self.input_target == InputTarget::Agent;
        let mut scrollback_offset: usize = 0;

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
                    // Render vt100 screen into ratatui buffer.
                    // Use a single lock to get both screen and scrollback
                    // offset atomically, avoiding race conditions with the
                    // background reader thread.
                    let (screen, sb_offset) = provider.screen_and_scrollback();
                    scrollback_offset = sb_offset;
                    let buf = frame.buffer_mut();
                    let (screen_rows, screen_cols) = screen.size();
                    for row in 0..screen_rows.min(term_area.height) {
                        for col in 0..screen_cols.min(term_area.width) {
                            let cell = screen.cell(row, col);
                            if let Some(cell) = cell {
                                if cell.is_wide_continuation() {
                                    continue;
                                }
                                let x = term_area.x + col;
                                let y = term_area.y + row;
                                let fg = convert_vt100_color(cell.fgcolor());
                                let bg = convert_vt100_color(cell.bgcolor());
                                let mut modifier = Modifier::empty();
                                if cell.bold() {
                                    modifier |= Modifier::BOLD;
                                }
                                if cell.italic() {
                                    modifier |= Modifier::ITALIC;
                                }
                                if cell.underline() {
                                    modifier |= Modifier::UNDERLINED;
                                }
                                if cell.inverse() {
                                    modifier |= Modifier::REVERSED;
                                }
                                let style = Style::default().fg(fg).bg(bg).add_modifier(modifier);
                                let contents = cell.contents();
                                let ratatui_cell = &mut buf[(x, y)];
                                if contents.is_empty() {
                                    ratatui_cell.set_symbol(" ");
                                } else {
                                    ratatui_cell.set_symbol(&contents);
                                }
                                ratatui_cell.set_style(style);
                            }
                        }
                    }

                    // Render cursor if in input mode.
                    if is_input && !screen.hide_cursor() {
                        let (cursor_row, cursor_col) = screen.cursor_position();
                        let cx = term_area.x + cursor_col;
                        let cy = term_area.y + cursor_row;
                        if cx < term_area.x + term_area.width && cy < term_area.y + term_area.height
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
        }

        // Hint bar with top border.
        if hint_area.height > 0 {
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
                spans.extend(self.theme.dim_key_badge("^G", Color::Reset));
                spans.push(Span::styled(" to bring the focus back.", desc_style));
                Line::from(spans)
            } else if scrollback_offset > 0 {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(
                    format!("Scrolled back {scrollback_offset} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge("ctrl+f", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge("ctrl+b", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                spans.push(Span::styled(" up.", desc_style));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if session_active {
                    spans.extend(self.theme.dim_key_badge("i", Color::Reset));
                    spans.push(Span::styled(" or ", desc_style));
                    spans.extend(self.theme.dim_key_badge("Enter", Color::Reset));
                    spans.push(Span::styled(" to interact. ", desc_style));
                    spans.extend(self.theme.dim_key_badge("^B", Color::Reset));
                    spans.push(Span::styled("/", desc_style));
                    spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge("^F", Color::Reset));
                    spans.push(Span::styled("/", desc_style));
                    spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                    spans.push(Span::styled(" to scroll.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge("r", Color::Reset));
                    spans.push(Span::styled(" or ", desc_style));
                    spans.extend(self.theme.dim_key_badge("enter", Color::Reset));
                    spans.push(Span::styled(" to relaunch.", desc_style));
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
    ) {
        let inner_width = area.width.saturating_sub(2) as usize; // minus borders
        let is_active_section = pane_focused && self.right_section == section;
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
        let title = format!("{title_prefix} ({})", files.len());
        let selected = if is_active_section {
            Some(self.files_index)
        } else {
            None
        };
        let mut state = ListState::default().with_selected(selected);
        StatefulWidget::render(
            List::new(items).block(self.themed_block(&title, is_active_section)),
            area,
            frame.buffer_mut(),
            &mut state,
        );
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
                let mut row: u16 = 0;
                let mut col: usize = 0;
                for (i, ch) in self.commit_input.char_indices() {
                    if i == self.commit_input_cursor {
                        break;
                    }
                    if ch == '\n' {
                        row += 1;
                        col = 0;
                    } else {
                        col += 1;
                        if col >= text_w {
                            row += 1;
                            col = 0;
                        }
                    }
                }
                if row < self.commit_scroll {
                    self.commit_scroll = row;
                } else if row >= self.commit_scroll + visible_h {
                    self.commit_scroll = row - visible_h + 1;
                }
            } else {
                self.commit_scroll = 0;
            }

            Paragraph::new(display_text)
                .style(style)
                .wrap(Wrap { trim: false })
                .scroll((self.commit_scroll, 0))
                .render(text_area, frame.buffer_mut());

            if focused {
                let (mut cursor_row, mut cursor_col) = (0u16, 0usize);
                for (i, ch) in self.commit_input.char_indices() {
                    if i == self.commit_input_cursor {
                        break;
                    }
                    if ch == '\n' {
                        cursor_row += 1;
                        cursor_col = 0;
                    } else {
                        cursor_col += 1;
                        if text_w > 0 && cursor_col >= text_w {
                            cursor_row += 1;
                            cursor_col = 0;
                        }
                    }
                }
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
            let mut spans: Vec<Span> = Vec::new();
            if focused {
                spans.extend(self.theme.dim_key_badge("Esc", Color::Reset));
                spans.push(Span::styled(" Exit", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge("i/Enter", Color::Reset));
                spans.push(Span::styled(" Edit  ", desc_style));
                spans.extend(self.theme.dim_key_badge("^G", Color::Reset));
                spans.push(Span::styled(" AI msg  ", desc_style));
                spans.extend(self.theme.dim_key_badge("c", Color::Reset));
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
        let hints = keybindings::hints_for(ctx);
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

    fn render_help(&self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(72, 70, frame.area());
        Clear.render(area, frame.buffer_mut());
        let help_bindings = keybindings::help_sections();
        let mut lines: Vec<Line> = Vec::new();
        for (section_idx, (section, bindings)) in help_bindings.iter().enumerate() {
            if section_idx > 0 {
                lines.push(Line::from(""));
            }
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
            let key = "^X";
            let desc = "Hold Ctrl and press X (e.g. ^P = Ctrl+P)";
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
        Paragraph::new(lines)
            .block(self.themed_overlay_block("Help"))
            .wrap(Wrap { trim: false })
            .render(area, frame.buffer_mut());
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
                let commands = keybindings::filtered_palette(input);
                let items = if commands.is_empty() {
                    vec![ListItem::new("No matching commands.")]
                } else {
                    // Compute column widths for aligned layout
                    let name_col = commands
                        .iter()
                        .map(|b| b.palette.as_ref().unwrap().name.len())
                        .max()
                        .unwrap_or(0);
                    let badge_col = commands
                        .iter()
                        .filter_map(|b| {
                            b.palette.as_ref().unwrap().shortcut.map(|s| s.len() + 3) // <key>
                        })
                        .max()
                        .unwrap_or(0);
                    // Available width inside borders: popup.width - 2 (borders) - 1 (left pad)
                    let inner_w = popup.width as usize - 3;
                    // Gap between columns
                    let gap = 2usize;
                    commands
                        .iter()
                        .map(|binding| {
                            let p = binding.palette.as_ref().unwrap();
                            // Pad name to fixed column width
                            let name_padded = format!("{:width$}", p.name, width = name_col);
                            let mut spans = vec![Span::styled(
                                name_padded,
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            )];
                            // Description column: fill the space between name and badge
                            let desc_avail = inner_w
                                .saturating_sub(name_col + gap)
                                .saturating_sub(if badge_col > 0 { badge_col + gap } else { 0 });
                            let desc = p.description;
                            let desc_display = if desc.len() > desc_avail && desc_avail > 1 {
                                format!("  {}\u{2026}", &desc[..desc_avail - 1])
                            } else {
                                format!("  {:width$}", desc, width = desc_avail)
                            };
                            spans.push(Span::styled(
                                desc_display,
                                Style::default().fg(self.theme.hint_desc_fg),
                            ));
                            // Right-aligned key badge
                            if badge_col > 0 {
                                if let Some(shortcut) = p.shortcut {
                                    // Right-pad the gap, then the badge
                                    let badge_len = shortcut.len() + 3;
                                    let pre_pad = gap + badge_col - badge_len;
                                    spans.push(Span::raw(" ".repeat(pre_pad)));
                                    spans.extend(self.theme.key_badge(shortcut, Color::Reset));
                                } else {
                                    spans.push(Span::raw(" ".repeat(gap + badge_col)));
                                }
                            }
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
                let mut bottom_spans = vec![Span::raw(" ")];
                if *searching {
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " done  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " run  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " complete  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
                let prompt_prefix = if *searching { "/ " } else { "> " };
                Paragraph::new(format!("{}{}", prompt_prefix, input))
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
                        format!("go: {}█", path_input)
                    } else {
                        format!("/ {}", filter)
                    };
                    Paragraph::new(input_text)
                        .block(self.themed_overlay_block(&title))
                        .render(filter_area, frame.buffer_mut());
                    let mut bottom_spans = vec![Span::raw(" ")];
                    if *editing_path {
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
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " done  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " clear",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else {
                        bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " search  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " open  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("g", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " go to  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
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
                    let mut bottom_spans = vec![Span::raw(" ")];
                    bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " open  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("o", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " add current  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("g", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " go to  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
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
                    (self.theme.button_confirm_border, self.theme.button_active_fg)
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
            PromptState::None => {}
        }
    }

    fn render_overlay(&self, frame: &mut Frame) {
        if !matches!(self.prompt, PromptState::None) {
            self.render_prompt(frame);
            return;
        }
        if self.help_overlay {
            self.render_help(frame);
        }
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

pub(crate) fn convert_vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
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
