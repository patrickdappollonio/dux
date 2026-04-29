use super::components::{
    Button, ButtonKind, ButtonPressedTarget, Checkbox, CheckboxState, button_state_for,
    shared_button_width,
};
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

/// Alternate braille-art duck logo, same width as the text logo.
const ASCII_LOGO_ALT: &[&str] = &[
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣤⠤⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠔⠉⠀⠀⢸⢰⢸⢰⢰⠉⠢⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡔⢰⠀⢸⢰⢸⢸⢸⢸⢸⢸⢸⢸⢈⢦⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⣰⢠⢸⠤⣤⠤⢸⢸⢸⢸⢸⠤⠤⢐⢸⢸⡆⠀⠀⠀⠀⠀⠀⠀",
    "⢠⠀⠀⠀⠀⠀⠀⠀⠀⢹⢸⢸⢸⢸⢸⣤⢰⢲⢤⣄⢸⢸⢸⢸⢸⡇⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠙⡄⠀⠀⠀⠀⠀⠀⠀⣄⢸⢰⣶⣿⣤⣤⣶⣤⣤⣬⣷⡤⢸⣰⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠈⢦⠀⠀⠀⠀⠀⠀⠈⢦⢸⢈⠛⠿⣿⣼⣼⠿⠛⠁⢸⡼⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠸⠉⢻⣍⠉⠤⣀⣠⣤⣾⢸⣿⣿⣿⣶⣾⣾⣿⣿⢸⢸⠓⠤⣄⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠈⠒⠭⣀⢸⢸⢈⢈⢸⢸⢸⠙⠻⢸⢸⢸⠿⠛⢸⢸⢸⢸⢸⢈⠑⢄⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣼⢸⢸⢈⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣠⠤⠒⠁⢸⣦⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣿⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢩⡂⢸⢸⣀⣴⣿⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠹⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢉⠉⠉⠁⢸⠃⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⢳⢨⠘⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣴⢸⢸⢸⢸⡟⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠳⣀⢠⢨⢘⢘⢸⢸⢸⢸⢸⢸⢸⢸⣤⣿⢸⢸⢸⣀⠛⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠀⠀⠀⠀⠀⠀⠀⠀",
];
/// Display width of each line in `ASCII_LOGO_ALT`.
const ASCII_LOGO_ALT_WIDTH: u16 = 33;
/// Number of lines in `ASCII_LOGO_ALT`.
const ASCII_LOGO_ALT_HEIGHT: u16 = 15;

/// Maximum display width for a tip line (logo width + padding on each side).
const TIP_MAX_WIDTH: u16 = 47;
/// Blank lines between the bottom of the logo and the tip.
const TIP_GAP: u16 = 2;
/// Maximum number of wrapped lines a tip may occupy.
const TIP_MAX_LINES: u16 = 3;

/// Welcome-screen tips shown beneath the ASCII logo. Wrap text in backticks
/// to highlight it in an accent color (the backticks themselves are not
/// rendered). Each function receives `&RuntimeBindings` so keybinding labels
/// stay accurate after rebinding.
const WELCOME_TIPS: &[fn(&RuntimeBindings) -> String] = &[
    // --- rewritten originals ---
    |b| {
        format!(
            "Lost? `{}` opens the command palette. Every action lives there, even the ones you forgot existed.",
            b.label_for(Action::OpenPalette)
        )
    },
    |b| {
        format!(
            "Need more room? `{}` toggles interactive mode, going fullscreen. Focus mode: activated.",
            b.label_for(Action::ExitInteractive)
        )
    },
    |b| {
        format!(
            "`{}` spawns a new agent in the current worktree. The more, the merrier.",
            b.label_for(Action::NewAgent)
        )
    },
    |_b| {
        "Any CLI tool can be a provider. Just set its `command` in config.toml. No plugins, no adapters.".into()
    },
    |b| {
        format!(
            "`{}` flips between agent and companion terminal. Two views, one worktree.",
            b.label_for(Action::ShowTerminal)
        )
    },
    |b| {
        format!(
            "`{}` stages or unstages the selected file. Git add, minus the typing.",
            b.label_for(Action::StageUnstage)
        )
    },
    |b| {
        format!(
            "Tired of writing commit messages? `{}` lets AI do it for you.",
            b.label_for(Action::GenerateCommitMessage)
        )
    },
    |b| {
        format!(
            "`{}` forks the current agent into a brand new session. Cloning never felt so good.",
            b.label_for(Action::ForkAgent)
        )
    },
    |b| {
        format!(
            "`{}` and `{}` hop between panes. Tab your way through everything.",
            b.label_for(Action::FocusNext),
            b.label_for(Action::FocusPrev)
        )
    },
    |b| {
        format!(
            "Open the palette with `{}` and run `change-agent-provider` to swap a worktree's CLI. Been here before? dux resumes that provider's last session automatically.",
            b.label_for(Action::OpenPalette)
        )
    },
    |_b| {
        "dux remembers which providers you've run on each worktree. Swap away and back, and each one picks up right where you left it.".into()
    },
    |_b| {
        "New agents spawning with the wrong CLI? Run `change-default-provider` in the palette to swap the default. Existing agents stay on their current brain.".into()
    },
    |_b| {
        "Swapped providers while an agent was still running? The sidebar shows `(old → new)` until you exit and relaunch. dux queues the swap, you run the show.".into()
    },
    // --- new tips ---
    |_b| {
        "The mouse works everywhere: click panes, scroll output, select files. Go ahead, click around.".into()
    },
    |_b| "Drag pane borders with the mouse to resize them. No keybindings required.".into(),
    |b| {
        format!(
            "Each agent gets its own companion terminal. Press `{}` to spawn more than one.",
            b.label_for(Action::ShowTerminal)
        )
    },
    |b| {
        format!(
            "Don't need the git pane? `{}` hides it. Want it gone for good? Check the command palette.",
            b.label_for(Action::ToggleGitPane)
        )
    },
    |b| {
        format!(
            "The `{}` key toggles the left sidebar. Maximum screen real estate, minimum distractions.",
            b.label_for(Action::ToggleSidebar)
        )
    },
    |_b| "Every keybinding is configurable. Open config.toml and make dux truly yours.".into(),
    |_b| {
        "Worktrees are the secret sauce: each agent gets its own isolated branch. No conflicts, ever."
            .into()
    },
    |b| {
        format!(
            "`{}` opens the project browser. Add worktrees from anywhere on disk.",
            b.label_for(Action::OpenProjectBrowser)
        )
    },
    |b| {
        format!(
            "`{}` opens the help overlay, the full keybinding reference, right in the app.",
            b.label_for(Action::ToggleHelp)
        )
    },
    |b| {
        format!(
            "Macros let you save and replay prompts. Configure them in config.toml, trigger with `{}`.",
            b.label_for(Action::OpenMacroBar)
        )
    },
    |_b| {
        "Launch 5 agents on 5 worktrees and let them all work in parallel. Conflicts? Let AI sort it out."
            .into()
    },
    |_b| {
        "Tired of typing the same prompt to your AI agent over and over? Turn it into a macro!"
            .into()
    },
    |_b| "Dux runs Claude the way Anthropic intended. No workarounds, no bans. Just vibes.".into(),
    |_b| {
        "The config file is also the documentation. Every option is configurable and the comments explain it all."
            .into()
    },
    |_b| {
        "Curious what you changed in your config? Run `dux config diff` to see exactly what's different from the defaults."
            .into()
    },
    |b| {
        format!(
            "Agent keybinds clashing with dux? `{}` toggles interactive mode. Most keys go straight to the agent.",
            b.label_for(Action::ExitInteractive)
        )
    },
    |_b| {
        "New agent prompt looking too empty? Tick the pet-name checkbox and let dux name your next chaos gremlin."
            .into()
    },
    |_b| {
        "Install the `gh` CLI and your agents can create commits and pull requests. Pair it with macros or skills to match your style."
            .into()
    },
    |_b| {
        "Your MCP servers, tools, and hooks? They all just work. We don't mess with your setup. Promise."
            .into()
    },
    |b| {
        format!(
            "Terminal looking glitchy? `{}` redraws the entire screen. Good as new.",
            b.label_for(Action::ForceRedraw)
        )
    },
    |b| {
        format!(
            "The command palette (`{}`) has features that don't have keybinds. Poke around, you might be surprised.",
            b.label_for(Action::OpenPalette)
        )
    },
];

/// Capitalize the first character of a string.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

fn wrapped_line_count(lines: &[Line<'_>], width: u16, trim: bool) -> u16 {
    if width == 0 {
        return 0;
    }

    let max_width = usize::from(width);
    let mut total = 0u16;
    for line in lines {
        let mut current_width = 0usize;
        let mut line_count = 1u16;
        for span in &line.spans {
            let content = if trim {
                span.content.trim_end_matches(' ')
            } else {
                span.content.as_ref()
            };
            for segment in content.split('\n') {
                let segment_width = segment.chars().count();
                if segment_width == 0 {
                    continue;
                }

                let remaining = if current_width == 0 {
                    max_width
                } else {
                    max_width.saturating_sub(current_width)
                };
                if segment_width <= remaining {
                    current_width += segment_width;
                } else {
                    let needed = if current_width == 0 {
                        segment_width
                    } else {
                        segment_width - remaining
                    };
                    line_count = line_count.saturating_add(((needed - 1) / max_width) as u16 + 1);
                    current_width = needed % max_width;
                    if current_width == 0 {
                        current_width = max_width;
                    }
                }
            }
        }
        total = total.saturating_add(line_count);
    }
    total
}

impl App {
    fn render_overlay_checkbox(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        checked: bool,
        state: CheckboxState,
        hint: Option<Line<'static>>,
    ) -> (Rect, u16) {
        let checkbox = Checkbox::new(label).checked(checked).state(state);
        let marker_style = checkbox.marker_style(match state {
            CheckboxState::Focused => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Hovered => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Disabled => Style::default().fg(self.theme.hint_desc_fg),
            CheckboxState::Normal => Style::default().fg(self.theme.hint_key_fg),
        });
        let label_style = checkbox.label_style(match state {
            CheckboxState::Focused => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Hovered => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Disabled => Style::default().fg(self.theme.hint_desc_fg),
            CheckboxState::Normal => Style::default().fg(self.theme.input_label_fg),
        });
        let layout = checkbox
            .layout(area.width, marker_style, label_style)
            .background(self.theme.overlay_bg);
        let checkbox_height = layout.height;
        let hint_height = u16::from(hint.is_some());
        let total_height = checkbox_height.saturating_add(hint_height);

        layout.render(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: checkbox_height,
            },
            frame.buffer_mut(),
        );

        if let Some(hint_line) = hint {
            Paragraph::new(hint_line).render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(checkbox_height),
                    width: area.width,
                    height: 1,
                },
                frame.buffer_mut(),
            );
        }

        (
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: total_height,
            },
            total_height,
        )
    }

    pub(crate) fn render(&mut self, frame: &mut Frame) {
        // Pre-fill the whole frame with the theme's app background. Cells
        // that no widget paints over (gutters, modal interiors, the strip
        // under the PR banner caps) inherit this color, so light themes
        // actually look light end-to-end. Widgets that explicitly set
        // `Color::Reset` override this and still pass through to the
        // user's terminal default — that path is preserved for PTY cells
        // that emit a "reset background" SGR, so the embedded agent
        // terminal keeps rendering the CLI's own colors unchanged.
        let frame_area = frame.area();
        frame
            .buffer_mut()
            .set_style(frame_area, Style::default().bg(self.theme.app_bg));

        let term_w = frame_area.width as usize;
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
        let right_constraint = if self.right_hidden {
            Constraint::Length(0)
        } else if self.right_collapsed {
            Constraint::Length(3)
        } else {
            Constraint::Percentage(self.right_width_pct)
        };

        let [left, center, right] = if self.left_collapsed {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(4), Constraint::Min(20), right_constraint])
                .areas(body)
        } else {
            let right_pct = if self.right_hidden || self.right_collapsed {
                0
            } else {
                self.right_width_pct
            };
            let center_pct = 100u16
                .saturating_sub(self.left_width_pct + right_pct)
                .max(20);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(self.left_width_pct),
                    Constraint::Percentage(center_pct),
                    right_constraint,
                ])
                .areas(body)
        };
        self.mouse_layout.reset(body, left, center, right);
        self.overlay_layout.reset();
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
            if let Some(session) = self.selected_session()
                && session.branch_name != project.current_branch
            {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                spans.push(Span::styled(
                    "agent: ",
                    Style::default().fg(label_fg).bg(bg),
                ));
                spans.push(Span::styled(
                    session.branch_name.clone(),
                    Style::default().fg(self.theme.branch_fg).bg(bg),
                ));
            }
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "default provider: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.default_provider.as_str().to_string(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            if let Some(session) = self.selected_session()
                && session.provider != project.default_provider
            {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                spans.push(Span::styled(
                    "current provider: ",
                    Style::default().fg(label_fg).bg(bg),
                ));
                spans.push(Span::styled(
                    session.provider.as_str().to_string(),
                    Style::default().fg(self.theme.branch_fg).bg(bg),
                ));
            }
            let running_terminals = self.running_companion_terminal_count();
            if running_terminals > 0 {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                let label = if running_terminals == 1 {
                    "● 1 terminal".to_string()
                } else {
                    format!("● {running_terminals} terminals")
                };
                spans.push(Span::styled(
                    label,
                    Style::default().fg(self.theme.session_active).bg(bg),
                ));
            }
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    fn render_left(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Left;

        if self.left_collapsed {
            self.mouse_layout.left_list = self.themed_block("", focused).inner(area);
            let collapsed_left_items = self.left_items();
            let items = collapsed_left_items
                .iter()
                .enumerate()
                .map(|(i, item)| match item {
                    LeftItem::Project(index) => {
                        let project = &self.projects[*index];
                        if project.path_missing {
                            ListItem::new(Line::from(Span::styled(
                                "⚠",
                                Style::default().fg(self.theme.project_missing_fg),
                            )))
                        } else {
                            let has_sessions =
                                self.sessions.iter().any(|s| s.project_id == project.id);
                            let icon =
                                if !has_sessions || self.collapsed_projects.contains(&project.id) {
                                    "▸"
                                } else {
                                    "▾"
                                };
                            ListItem::new(Line::from(Span::styled(
                                icon,
                                Style::default().fg(self.theme.project_icon),
                            )))
                        }
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

        let terminal_items = self.terminal_items();
        let has_terminals = !terminal_items.is_empty();

        // Split area vertically: projects on top, terminals on bottom (if any).
        let (projects_area, terminals_area) = if has_terminals {
            let pct = self.terminal_pane_height_pct.clamp(10, 80);
            let projects_pct = 100u16.saturating_sub(pct).max(20);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(projects_pct),
                    Constraint::Percentage(pct),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Collect terminal display info for rendering.
        // Show the foreground command if one is running, otherwise the session label.
        let terminal_render_data: Vec<(String, Option<String>)> = terminal_items
            .iter()
            .map(|(_, t)| (t.label.clone(), t.foreground_cmd.clone()))
            .collect();

        let session_counts: HashMap<String, usize> = {
            let mut counts = HashMap::new();
            for session in &self.sessions {
                *counts.entry(session.project_id.clone()).or_insert(0) += 1;
            }
            counts
        };
        let left_items = self.left_items();
        let projects_focused = focused && self.left_section == LeftSection::Projects;
        let items = left_items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                LeftItem::Project(index) => {
                    let project = &self.projects[*index];
                    if project.path_missing {
                        let spans = vec![
                            Span::styled("⚠ ", Style::default().fg(self.theme.project_missing_fg)),
                            Span::styled(
                                project.name.clone(),
                                Style::default().fg(self.theme.project_missing_fg),
                            ),
                        ];
                        ListItem::new(Line::from(spans))
                    } else {
                        let count = session_counts.get(&project.id).copied().unwrap_or(0);
                        let icon = if count == 0 || self.collapsed_projects.contains(&project.id) {
                            "▸ "
                        } else {
                            "▾ "
                        };
                        let mut spans = vec![
                            Span::styled(icon, Style::default().fg(self.theme.project_icon)),
                            Span::styled(
                                project.name.clone(),
                                Style::default()
                                    .fg(self.theme.text_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ];
                        if count > 0 {
                            spans.push(Span::styled(
                                format!(" ({count})"),
                                Style::default().fg(self.theme.provider_label_fg),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    }
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
                    let (dot, dot_color) =
                        if matches!(session.status, crate::model::SessionStatus::Active)
                            && self.is_agent_streaming(&session.id)
                        {
                            let idx = self.spinner_frame_index();
                            (
                                crate::theme::SPINNER_FRAMES[idx].to_string(),
                                self.theme.session_active,
                            )
                        } else {
                            let (d, c) = self.theme.session_dot(&session.status);
                            (d.to_string(), c)
                        };
                    // While a background delete is in flight for this session,
                    // dim the row text and italicize it so the user sees the
                    // transient state. This covers the dot, label, and
                    // provider suffix; the companion-terminal badge chain is
                    // excluded because it renders through a separate helper
                    // and the in-flight window is too brief to justify
                    // threading a deletion flag through it.
                    let deleting = self.pending_deletions.contains(&session.id);
                    let label_color = if deleting {
                        self.theme.session_deleting
                    } else {
                        match self.pr_statuses.get(&session.id).map(|pr| &pr.state) {
                            Some(crate::model::PrState::Merged) => self.theme.pr_merged_label,
                            Some(crate::model::PrState::Closed) => self.theme.pr_closed_label,
                            Some(crate::model::PrState::Open) => self.theme.pr_open_label,
                            None => dot_color,
                        }
                    };
                    let label_style = if deleting {
                        Style::default()
                            .fg(label_color)
                            .add_modifier(Modifier::ITALIC)
                    } else {
                        Style::default().fg(label_color)
                    };
                    let running_provider = self.running_provider_for(session);
                    let provider_label = if running_provider != session.provider {
                        // Swap is pending: agent is still running the old
                        // provider but will spawn with session.provider on
                        // next launch. Show both so the sidebar doesn't lie.
                        format!(
                            " ({} → {})",
                            running_provider.as_str(),
                            session.provider.as_str(),
                        )
                    } else {
                        format!(" ({})", session.provider.as_str())
                    };
                    ListItem::new(Line::from(
                        vec![
                            Span::styled(connector, Style::default().fg(self.theme.project_icon)),
                            Span::styled(format!("{dot} "), label_style),
                            Span::styled(label, label_style),
                            Span::styled(
                                provider_label,
                                if deleting {
                                    label_style
                                } else {
                                    Style::default().fg(self.theme.provider_label_fg)
                                },
                            ),
                        ]
                        .into_iter()
                        .chain(companion_terminal_row_badge(
                            self.companion_terminal_status(&session.id),
                            &self.theme,
                        ))
                        .collect::<Vec<_>>(),
                    ))
                }
            })
            .collect::<Vec<_>>();
        let title = format!("Projects ({})", self.projects.len());
        self.mouse_layout.left_list = self
            .themed_block(&title, projects_focused)
            .inner(projects_area);
        let mut state =
            ListState::default().with_selected(if self.left_section == LeftSection::Projects {
                Some(self.selected_left)
            } else {
                None
            });
        StatefulWidget::render(
            List::new(items)
                .block(self.themed_block(&title, projects_focused))
                .highlight_style(self.theme.selection_style()),
            projects_area,
            frame.buffer_mut(),
            &mut state,
        );

        // Render terminals section if any terminals exist.
        if let Some(term_area) = terminals_area {
            let terminals_focused = focused && self.left_section == LeftSection::Terminals;
            let term_title = format!("Terminals ({})", terminal_render_data.len());
            self.mouse_layout.terminal_list = self
                .themed_block(&term_title, terminals_focused)
                .inner(term_area);
            let term_items: Vec<ListItem> = terminal_render_data
                .iter()
                .map(|(label, fg_cmd)| {
                    let color = self.theme.session_active;
                    let mut spans = vec![Span::styled("● ", Style::default().fg(color))];
                    if let Some(cmd) = fg_cmd {
                        spans.push(Span::styled(cmd.clone(), Style::default().fg(color)));
                        spans.push(Span::styled(
                            format!(" · {label}"),
                            Style::default().fg(self.theme.provider_label_fg),
                        ));
                    } else {
                        spans.push(Span::styled(label.clone(), Style::default().fg(color)));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut term_state = ListState::default().with_selected(
                if self.left_section == LeftSection::Terminals {
                    Some(self.selected_terminal_index)
                } else {
                    None
                },
            );
            StatefulWidget::render(
                List::new(term_items)
                    .block(self.themed_block(&term_title, terminals_focused))
                    .highlight_style(self.theme.selection_style()),
                term_area,
                frame.buffer_mut(),
                &mut term_state,
            );
        } else {
            self.mouse_layout.terminal_list = Rect::default();
        }
    }

    fn render_center(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Center;

        // Determine if a PR banner should be shown above the center pane.
        let is_input = matches!(
            (self.input_target, self.session_surface),
            (InputTarget::Agent, SessionSurface::Agent)
                | (InputTarget::Terminal, SessionSurface::Terminal)
        );
        let pr_info = if !is_input {
            self.selected_session()
                .and_then(|s| self.pr_statuses.get(&s.id))
                .cloned()
        } else {
            None
        };
        let pr_banner_height: u16 = if pr_info.is_some() { 1 } else { 0 };

        let (pr_area, pane_area) = if self.pr_banner_at_bottom {
            let [pane_area, pr_area] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(pr_banner_height)])
                .areas(area);
            (pr_area, pane_area)
        } else {
            let [pr_area, pane_area] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(pr_banner_height), Constraint::Min(1)])
                .areas(area);
            (pr_area, pane_area)
        };

        if let Some(ref pr) = pr_info {
            self.render_pr_banner(frame, pr_area, pr);
        }

        match &self.center_mode {
            CenterMode::Diff { .. } => {
                self.render_diff(frame, pane_area, focused);
            }
            CenterMode::Agent if !matches!(self.fullscreen_overlay, FullscreenOverlay::None) => {
                // Skip agent rendering here — fullscreen overlay handles it.
                // Rendering in both places causes the PTY to be resized twice
                // per frame (once to the small pane, once to the overlay).
                let title = self.center_pane_agent_title();
                self.themed_block(&title, focused)
                    .render(pane_area, frame.buffer_mut());
            }
            CenterMode::Agent => {
                let title = self.center_pane_agent_title();
                // Center pane always renders the agent; terminal is an overlay.
                let saved = self.session_surface;
                self.session_surface = SessionSurface::Agent;
                self.render_agent_terminal(frame, pane_area, &title, focused);
                self.session_surface = saved;
            }
        }
    }

    fn render_diff(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let (lines, scroll, gutter_width) = match &self.center_mode {
            CenterMode::Diff {
                lines,
                scroll,
                gutter_width,
                ..
            } => (Arc::clone(lines), *scroll, *gutter_width),
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
        self.mouse_layout.agent_term = Some(content_area);

        self.last_diff_height = content_area.height;

        let w = content_area.width.max(1) as usize;

        if gutter_width > 0 {
            // Gutter-aware wrapping: continuation lines are indented to align
            // with the content column past the gutter.
            let wrapped = crate::diff::wrap_diff_lines(&lines, w, gutter_width);
            self.last_diff_visual_lines = wrapped.len() as u16;

            let max_scroll = self
                .last_diff_visual_lines
                .saturating_sub(content_area.height);
            let scroll = scroll.min(max_scroll);

            Paragraph::new(wrapped)
                .scroll((scroll, 0))
                .render(content_area, frame.buffer_mut());
        } else {
            // No gutter — fall back to ratatui's built-in wrapping.
            self.last_diff_visual_lines = lines
                .iter()
                .map(|l| {
                    let lw = l.width();
                    if lw <= w { 1u16 } else { lw.div_ceil(w) as u16 }
                })
                .sum();

            let max_scroll = self
                .last_diff_visual_lines
                .saturating_sub(content_area.height);
            let scroll = scroll.min(max_scroll);

            Paragraph::new((*lines).clone())
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .render(content_area, frame.buffer_mut());
        }

        // Hint bar with top border (same style as agent terminal).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line. ", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" to scroll. ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line. ", desc_style));
            }
            spans.extend(self.theme.dim_key_badge_default(&close));
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

    /// Render the ASCII "dux" logo centered in the given area, with an
    /// optional feature tip displayed below.
    fn render_ascii_logo(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < ASCII_LOGO_WIDTH || area.height < ASCII_LOGO_HEIGHT {
            return;
        }

        // Rotate the tip and randomly pick a logo variant when the logo
        // becomes visible again after being hidden, or when the selected
        // left-pane item changes while the logo stays visible.
        if !self.welcome_logo_visible || self.welcome_tip_selection != self.selected_left {
            self.welcome_tip_index = self.welcome_tip_index.wrapping_add(1);
            self.welcome_logo_alt = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() % 2 == 0)
                .unwrap_or(false);
        }
        self.welcome_logo_visible = true;
        self.welcome_tip_selection = self.selected_left;

        // Pick the active logo variant. Fall back to the text logo when the
        // area is too short for the taller duck.
        let use_alt = self.welcome_logo_alt && area.height >= ASCII_LOGO_ALT_HEIGHT;
        let (logo, logo_w, logo_h) = if use_alt {
            (ASCII_LOGO_ALT, ASCII_LOGO_ALT_WIDTH, ASCII_LOGO_ALT_HEIGHT)
        } else {
            (ASCII_LOGO, ASCII_LOGO_WIDTH, ASCII_LOGO_HEIGHT)
        };

        let total_height = logo_h + TIP_GAP + TIP_MAX_LINES;
        let show_tip = area.width >= TIP_MAX_WIDTH && area.height >= total_height;

        let block_height = if show_tip { total_height } else { logo_h };
        let x = area.x + (area.width - logo_w) / 2;
        let y = area.y + (area.height - block_height) / 2;

        // --- logo ---
        let style = Style::default().fg(self.theme.border_normal);
        let lines: Vec<Line> = logo.iter().map(|l| Line::styled(*l, style)).collect();
        Paragraph::new(lines).render(Rect::new(x, y, logo_w, logo_h), frame.buffer_mut());

        // --- tip pill ---
        if show_tip {
            let text_fn = &WELCOME_TIPS[self.welcome_tip_index % WELCOME_TIPS.len()];
            let tip_text = text_fn(&self.bindings);

            let pill_span = Span::styled(
                " Tip ",
                Style::default()
                    .fg(self.theme.tip_pill_fg)
                    .bg(self.theme.tip_pill_bg)
                    .add_modifier(Modifier::BOLD),
            );

            let normal = Style::default().fg(self.theme.tip_text_fg);
            let highlight = Style::default()
                .fg(self.theme.tip_highlight_fg)
                .add_modifier(Modifier::BOLD);

            let mut spans: Vec<Span> = vec![pill_span, Span::raw(" ")];
            let mut inside_backtick = false;
            for segment in tip_text.split('`') {
                if !segment.is_empty() {
                    spans.push(Span::styled(
                        segment.to_owned(),
                        if inside_backtick { highlight } else { normal },
                    ));
                }
                inside_backtick = !inside_backtick;
            }

            let tip_line = Line::from(spans);
            let tip_width = TIP_MAX_WIDTH.min(area.width.saturating_sub(2));
            let tip_x = area.x + (area.width - tip_width) / 2;
            let tip_y = y + logo_h + TIP_GAP;

            Paragraph::new(vec![tip_line])
                .wrap(Wrap { trim: false })
                .alignment(ratatui::layout::Alignment::Center)
                .render(
                    Rect::new(tip_x, tip_y, tip_width, TIP_MAX_LINES),
                    frame.buffer_mut(),
                );
        }
    }

    fn render_terminal_placeholder(
        &self,
        frame: &mut Frame,
        area: Rect,
        status: CompanionTerminalStatus,
        command_name: Option<&str>,
    ) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let (icon, label) = companion_terminal_status_meta(status);
        let command_name = command_name.unwrap_or("terminal");
        let lines = match status {
            CompanionTerminalStatus::NotLaunched => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal not launched"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    format!(
                        "Launch {command_name} explicitly when you need a shell in this worktree."
                    ),
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
            CompanionTerminalStatus::Running => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal {label}"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    "The PTY is alive even when hidden from the center pane.",
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
            CompanionTerminalStatus::Exited => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal exited"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    "Relaunch it explicitly to start a fresh shell.",
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
        };

        let height = lines.len() as u16;
        let y = area.y + area.height.saturating_sub(height) / 2;
        Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .render(
                Rect::new(area.x, y, area.width, height.max(1)),
                frame.buffer_mut(),
            );
    }

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let nudge_active = self.is_nudge_active();
        let outer_block = if nudge_active {
            self.themed_block(title, focused)
                .border_style(Style::default().fg(self.theme.nudge_border))
        } else {
            self.themed_block(title, focused)
        };
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let active_surface = self.session_surface;
        let terminal_status = self.selected_companion_terminal_status();
        let is_input = matches!(
            (self.input_target, active_surface),
            (InputTarget::Agent, SessionSurface::Agent)
                | (InputTarget::Terminal, SessionSurface::Terminal)
        );
        let mut scrollback_offset: usize = 0;
        let mut rendered_content = false;

        // Reserve 2 lines at the bottom for the hint bar (top border + text).
        let hint_height = 2;
        let [term_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);
        self.mouse_layout.agent_term = Some(term_area);

        // Get the selected session's PTY screen.
        let session_id = self.selected_session().map(|s| s.id.clone());
        let session_provider_name = match active_surface {
            SessionSurface::Agent => self
                .selected_session()
                .map(|s| self.running_provider_for(s).as_str().to_owned()),
            SessionSurface::Terminal => Some(
                self.config
                    .terminal
                    .command
                    .rsplit(std::path::MAIN_SEPARATOR)
                    .next()
                    .unwrap_or(self.config.terminal.command.as_str())
                    .to_string(),
            ),
        };
        let session_active = match active_surface {
            SessionSurface::Agent => session_id
                .as_ref()
                .map(|id| self.providers.contains_key(id))
                .unwrap_or(false),
            SessionSurface::Terminal => terminal_status.is_running(),
        };
        let new_size = (term_area.height, term_area.width);
        let should_resize = new_size != self.last_pty_size && new_size.0 > 0 && new_size.1 > 0;
        if should_resize {
            self.last_pty_size = new_size;
        }

        if let Some(provider) = self.selected_terminal_surface_client() {
            rendered_content = true;
            // Resize PTY if needed.
            if should_resize {
                let _ = provider.resize(new_size.0, new_size.1);
            }

            if !provider.has_output() {
                // Show a centered loading card until the PTY produces output.
                let idx = self.spinner_frame_index();
                let spinner = crate::theme::SPINNER_FRAMES[idx];
                let (label_spans, label_len) = match session_provider_name.as_deref() {
                    Some(name) => {
                        let prefix = match active_surface {
                            SessionSurface::Agent => "Starting ",
                            SessionSurface::Terminal => "Launching ",
                        };
                        let text_len = prefix.len() + name.len() + "...".len();
                        let spans = vec![
                            Span::styled(prefix, Style::default().fg(self.theme.hint_desc_fg)),
                            Span::styled(
                                name.to_owned(),
                                Style::default().fg(self.theme.branch_fg),
                            ),
                            Span::styled("...", Style::default().fg(self.theme.hint_desc_fg)),
                        ];
                        (spans, text_len)
                    }
                    None => {
                        let text = match active_surface {
                            SessionSurface::Agent => "Starting agent...",
                            SessionSurface::Terminal => "Launching terminal...",
                        };
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
                // Capture alt-screen state before mutable borrows — we need
                // it below (after `refresh_snapshot_buf` takes &mut self) to
                // decide whether the scrollback indicator applies.
                let is_alt_screen = provider.is_alt_screen();

                // Render the current terminal viewport into the ratatui
                // buffer, reusing the pre-allocated snapshot buffer to
                // avoid per-frame heap allocation.
                self.refresh_snapshot_buf();
                scrollback_offset = self.snapshot_buf.scrollback_offset;

                // When returning from scrollback to the live bottom,
                // clear the PTY area so stale cells don't linger in
                // ratatui's diff buffer.
                if scrollback_offset != self.prev_scrollback_offset {
                    Clear.render(term_area, frame.buffer_mut());
                }
                self.prev_scrollback_offset = scrollback_offset;

                let buf = frame.buffer_mut();
                for cell in &self.snapshot_buf.cells {
                    if cell.row >= self.snapshot_buf.rows
                        || cell.col >= self.snapshot_buf.cols
                        || cell.row >= term_area.height
                        || cell.col >= term_area.width
                    {
                        continue;
                    }
                    let x = term_area.x + cell.col;
                    let y = term_area.y + cell.row;
                    let (fg, bg) = pty_cell_colors(cell.fg, cell.bg, is_input, &self.theme);
                    let mut style = Style::default().fg(fg).add_modifier(cell.modifier);
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    let ratatui_cell = &mut buf[(x, y)];
                    ratatui_cell.set_symbol(&cell.symbol);
                    ratatui_cell.set_style(style);

                    // Overlay selection highlight if this cell is selected.
                    if let Some(sel) = &self.terminal_selection
                        && sel.anchor != sel.end
                        && sel.contains(cell.row, cell.col)
                    {
                        ratatui_cell.set_style(self.theme.selection_style());
                    }
                }

                // Render cursor if in input mode.
                if is_input
                    && let Some(cursor) = self.snapshot_buf.cursor
                    && cursor.row < self.snapshot_buf.rows
                    && cursor.col < self.snapshot_buf.cols
                {
                    let cx = term_area.x + cursor.col;
                    let cy = term_area.y + cursor.row;
                    if cx < term_area.x + term_area.width && cy < term_area.y + term_area.height {
                        let cursor_cell = &mut buf[(cx, cy)];
                        cursor_cell.set_style(
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.prompt_cursor),
                        );
                    }
                }

                // Suppress the scrollback indicator when the child is using
                // the alternate screen buffer — the alt grid has no history,
                // so the label would be misleading even if it somehow rendered.
                if !is_alt_screen
                    && let Some(label) = scrollback_indicator_label(
                        self.snapshot_buf.scrollback_offset,
                        self.snapshot_buf.scrollback_total,
                    )
                {
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

        if rendered_content {
            self.welcome_logo_visible = false;
        } else {
            match active_surface {
                SessionSurface::Agent => self.render_ascii_logo(frame, term_area),
                SessionSurface::Terminal => {
                    self.welcome_logo_visible = false;
                    self.render_terminal_placeholder(
                        frame,
                        term_area,
                        terminal_status,
                        session_provider_name.as_deref(),
                    );
                }
            }
        }

        // Macro bar overlays the hint area when active.
        if self.macro_bar.is_some() {
            self.render_macro_bar(frame, inner);
            return;
        }

        // Hint bar with top border.
        if hint_area.height > 0 {
            // Pre-compute all key labels so they outlive the Span borrows.
            let exit_key = self.bindings.label_for(Action::ExitInteractive);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let focus_agent = self.bindings.labels_for(Action::FocusAgent);
            let reconnect = self.bindings.labels_for(Action::ReconnectAgent);

            let macro_key = self.bindings.label_for(Action::OpenMacroBar);
            let hint_line = if is_input {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.extend(self.theme.dim_key_badge_default(&exit_key));
                spans.push(Span::styled(" return  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                if scrollback_offset > 0 {
                    spans.push(Span::styled(" down  ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                    spans.push(Span::styled(" down one line", desc_style));
                } else {
                    spans.push(Span::styled(" down", desc_style));
                }
                if !self.filtered_macros("").is_empty() && !macro_key.is_empty() {
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&macro_key));
                    spans.push(Span::styled(" macros.", desc_style));
                }
                Line::from(spans)
            } else if scrollback_offset > 0 {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(
                    format!("Scrolled back {scrollback_offset} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line.", desc_style));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if matches!(active_surface, SessionSurface::Terminal) {
                    match terminal_status {
                        CompanionTerminalStatus::Running => {
                            spans.push(Span::styled(
                                "Companion terminal is running. Hidden terminals stay alive in this worktree.",
                                desc_style,
                            ));
                        }
                        CompanionTerminalStatus::Exited => {
                            spans.push(Span::styled(
                                "Companion terminal exited. Relaunch it explicitly to start a fresh shell.",
                                desc_style,
                            ));
                        }
                        CompanionTerminalStatus::NotLaunched => {
                            spans.push(Span::styled(
                                "Companion terminal is not launched yet. Launch it explicitly when needed.",
                                desc_style,
                            ));
                        }
                    }
                } else if session_active && nudge_active {
                    let warn_style = Style::default().fg(self.theme.nudge_border);
                    spans.push(Span::styled(
                        "Read-only \u{2014} agent needs full keyboard control. ",
                        warn_style,
                    ));
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to interact.", desc_style));
                } else if session_active {
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to interact. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                    spans.push(Span::styled(" to scroll. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                    spans.push(Span::styled(" one line.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&reconnect));
                    spans.push(Span::styled(" to relaunch or ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
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
        if self.right_hidden {
            return;
        }

        if self.right_collapsed {
            let focused = self.focus == FocusPane::Files;
            let all_files: Vec<(&str, Color)> = self
                .unstaged_files
                .iter()
                .chain(self.staged_files.iter())
                .map(|f| (f.status.as_str(), self.theme.file_status_fg))
                .collect();
            let items: Vec<ListItem> = all_files
                .iter()
                .map(|(s, color)| {
                    ListItem::new(Line::from(Span::styled(
                        s.to_string(),
                        Style::default().fg(*color),
                    )))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(self.files_index));
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

        let has_staged = !self.staged_files.is_empty();
        let focused = self.focus == FocusPane::Files;

        if has_staged {
            let pct = self.staged_pane_height_pct.clamp(10, 80);
            let unstaged_pct = 100u16.saturating_sub(pct).max(20);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(unstaged_pct), // Changes (unstaged) — always on top
                    Constraint::Percentage(pct),          // Staged Changes (with commit input)
                ])
                .split(area);
            let list_rect = self.render_file_list(
                frame,
                chunks[0],
                "Changes",
                &self.unstaged_files,
                RightSection::Unstaged,
                true,
            );
            self.mouse_layout.unstaged_list = Some(list_rect);
            self.render_staged_with_commit(frame, chunks[1], focused);
        } else {
            let list_rect = self.render_file_list(
                frame,
                area,
                "Changes",
                &self.unstaged_files,
                RightSection::Unstaged,
                true,
            );
            self.mouse_layout.unstaged_list = Some(list_rect);
        }
    }

    /// Render the "Staged Changes" file list and the commit input as two
    /// separate bordered blocks (bubbles).
    fn render_staged_with_commit(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        let commit_pct = self.commit_pane_height_pct.clamp(10, 80);
        let staged_pct = 100u16.saturating_sub(commit_pct).max(20);
        let [files_area, commit_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(staged_pct),
                Constraint::Percentage(commit_pct),
            ])
            .areas(area);

        // Staged files — normal titled block.
        let list_rect = self.render_file_list(
            frame,
            files_area,
            "Staged Changes",
            &self.staged_files,
            RightSection::Staged,
            false,
        );
        self.mouse_layout.staged_list = Some(list_rect);

        // Commit input block.
        self.render_commit_input_inner(frame, commit_area, pane_focused);
    }

    /// Render a file list inside a bordered block and return the inner `Rect`
    /// where file rows were actually placed.  Callers store this in
    /// `mouse_layout` so that mouse-hit detection matches the real rendering.
    fn render_file_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        title_prefix: &str,
        files: &[ChangedFile],
        section: RightSection,
        show_hint: bool,
    ) -> Rect {
        let pane_focused = self.focus == FocusPane::Files;
        let is_active_section = pane_focused && self.right_section == section;
        let title = format!("{title_prefix} ({})", files.len());
        let block = self.themed_block(&title, is_active_section);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        let show_search =
            is_active_section && (self.files_search_active || self.has_files_search());
        let (search_area, list_inner) = if show_search && inner.height >= 4 {
            let [sa, la] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(1)])
                .areas(inner);
            (Some(sa), la)
        } else {
            (None, inner)
        };

        // Optionally reserve 2 lines at the bottom for the hint bar (border + text).
        let (list_area, hint_area) = if show_hint && pane_focused && list_inner.height >= 4 {
            let [la, ha] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(2)])
                .areas(list_inner);
            (la, Some(ha))
        } else {
            (list_inner, None)
        };

        if let Some(search_area) = search_area {
            let query = format!("/ {}", self.files_search.text);
            Paragraph::new(query)
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(search_area, frame.buffer_mut());
        }

        let inner_width = list_area.width as usize;
        let sel_style = self.theme.selection_style();
        let items = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let is_selected = is_active_section && index == self.files_index;

                // Build the right-aligned stats string, e.g. "+12 -3".
                let stats =
                    format_line_stats(file.additions, file.deletions, file.binary, &self.theme);
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
            let search_key = self.bindings.label_for(Action::SearchFiles);
            let next_key = self.bindings.label_for(Action::SearchNext);
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let mut spans: Vec<Span> = Vec::new();
            spans.extend(self.theme.dim_key_badge_default(&stage_key));
            spans.push(Span::styled(" stage/unstage.", desc_style));
            spans.push(Span::raw("  "));
            if self.files_search_active {
                spans.extend(self.theme.dim_key_badge_default("Enter"));
                spans.push(Span::styled(" done  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default("Esc"));
                spans.push(Span::styled(" clear", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&search_key));
                spans.push(Span::styled(" search", desc_style));
                if self.has_files_search() {
                    spans.push(Span::raw("  "));
                    spans.extend(self.theme.dim_key_badge_default(&next_key));
                    spans.push(Span::styled(" next match", desc_style));
                }
            }
            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(ha, frame.buffer_mut());
        }

        list_area
    }

    /// Render the commit input as its own bordered block.
    fn render_commit_input_inner(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        self.mouse_layout.commit_area = Some(area);
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
        self.mouse_layout.commit_text_area = Some(text_area);

        // Update TextInput's display dimensions to match the available area.
        let text_w = text_area.width as usize;
        self.commit_input
            .set_display_width(if text_w > 0 { Some(text_w) } else { None });
        self.commit_input
            .set_visible_lines(text_area.height as usize);

        if let Some(overlay) = self.commit_input.overlay() {
            // Overlay (e.g. "Generating commit message…") with spinner prefix.
            let idx = self.spinner_frame_index();
            let spinner = crate::theme::SPINNER_FRAMES[idx];
            let text = format!("{spinner} {overlay}");
            Paragraph::new(text)
                .style(Style::default().fg(self.theme.hint_desc_fg))
                .render(text_area, frame.buffer_mut());
        } else if self.commit_input.is_empty() && !focused {
            // Show placeholder when unfocused and empty — nothing to render
            // (the placeholder is shown only when focused, below).
        } else {
            // Render visible lines from TextInput (handles wrapping + scroll).
            let visible = self.commit_input.visible_lines();
            let (cursor_row, cursor_col) = self.commit_input.cursor_display_position();
            let is_empty = self.commit_input.is_empty();

            // When empty and focused, show the placeholder.
            if is_empty {
                if let Some(ph) = self.commit_input.placeholder() {
                    Paragraph::new(ph.to_string())
                        .style(Style::default().fg(self.theme.hint_desc_fg))
                        .render(text_area, frame.buffer_mut());
                }
            } else {
                for (i, line_text) in visible.iter().enumerate() {
                    if i >= text_area.height as usize {
                        break;
                    }
                    let y = text_area.y + i as u16;
                    let line_area = Rect::new(text_area.x, y, text_area.width, 1);
                    Paragraph::new(line_text.as_str()).render(line_area, frame.buffer_mut());
                }
            }

            // Position the hardware cursor when focused.
            if focused && !is_empty {
                let cx = text_area.x + cursor_col as u16;
                let cy = text_area.y + cursor_row as u16;
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
                spans.extend(self.theme.dim_key_badge_default(&exit));
                spans.push(Span::styled(" Exit", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&engage));
                spans.push(Span::styled(" Edit  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&ai_msg));
                spans.push(Span::styled(" AI msg  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&commit));
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
            FocusPane::Left if self.left_section == LeftSection::Terminals => {
                HintContext::LeftTerminal
            }
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
            StatusTone::Warning => self.theme.warning_fg,
            StatusTone::Error => self.theme.status_error_fg,
        };
        let status_bg = match tone {
            StatusTone::Info => self.theme.status_info_bg,
            StatusTone::Busy => self.theme.status_busy_bg,
            StatusTone::Warning => self.theme.status_info_bg,
            StatusTone::Error => self.theme.status_error_bg,
        };
        let prefix = format!(" {dot} ");
        let prefix_w = prefix.chars().count();
        let max_status_chars = (status_area.width as usize) * (status_area.height as usize);
        let available = max_status_chars.saturating_sub(prefix_w);
        let truncated = truncate_status_text(&status_text, available);
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
        self.clear_overlay_area(frame, area);

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
        self.overlay_layout.active = OverlayMouseLayout::Help;

        // Build help content lines.
        let mut lines: Vec<Line> = Vec::new();
        let content_width = content_area.width as usize;

        let banner_style = Style::default()
            .fg(self.theme.help_banner_fg)
            .bg(self.theme.help_banner_bg)
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(self.theme.help_body_fg);

        // Helper: push a full-width banner line.
        let push_banner = |lines: &mut Vec<Line>, title: &str, width: usize| {
            let padding = width.saturating_sub(title.chars().count() + 3);
            let text = format!(" {title}{}", " ".repeat(padding));
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(text, banner_style)));
            lines.push(Line::from(""));
        };

        // ── About dux ──────────────────────────────────────────
        push_banner(&mut lines, "About dux", content_width);
        lines.push(Line::from(Span::styled(
            "dux is a terminal UI for orchestrating AI coding agents.",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "Each project maps to a git worktree, and you can spawn",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "unlimited agents — and unlimited companion terminals for",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "each agent — all running side by side.",
            body_style,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Any CLI tool can be a provider: Claude, Codex, Gemini,",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "OpenCode, or anything else you configure.",
            body_style,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Every keybinding shown below is fully rebindable.",
            body_style,
        )));
        lines.push(Line::from(vec![
            Span::styled(
                "Your config file is self-documented — open it and explore: ",
                body_style,
            ),
            Span::styled(
                self.paths.config_path.display().to_string(),
                Style::default()
                    .fg(self.theme.hint_key_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // ── Keybindings ─────────────────────────────────────────
        push_banner(&mut lines, "Keybindings", content_width);

        let help_bindings = self.bindings.help_sections();
        for (i, (section, bindings)) in help_bindings.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                section.to_string(),
                Style::default()
                    .fg(self.theme.help_section_header_fg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for (key, desc) in bindings {
                let padding = 14usize.saturating_sub(key.len() + 2);
                let mut spans = vec![Span::raw("  ")];
                spans.extend(self.theme.key_badge_default(key));
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                lines.push(Line::from(spans));
            }
        }

        // ── Reference ───────────────────────────────────────────
        push_banner(&mut lines, "Reference", content_width);

        // Key notation legend
        lines.push(Line::from(Span::styled(
            "Key notation",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            let key = "Ctrl-x";
            let desc = "Hold Ctrl and press X (e.g. Ctrl-p)";
            let padding = 14usize.saturating_sub(key.len() + 2);
            let mut spans = vec![Span::raw("  ")];
            spans.extend(self.theme.key_badge_default(key));
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
                .fg(self.theme.help_section_header_fg)
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

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Companion terminal states",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for status in [
            CompanionTerminalStatus::NotLaunched,
            CompanionTerminalStatus::Running,
            CompanionTerminalStatus::Exited,
        ] {
            let (icon, label) = companion_terminal_status_meta(status);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    icon,
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                ),
                Span::raw("  "),
                Span::styled(
                    match status {
                        CompanionTerminalStatus::NotLaunched => {
                            format!("{label} — shell has not been started")
                        }
                        CompanionTerminalStatus::Running => {
                            format!("{label} — companion shell is alive")
                        }
                        CompanionTerminalStatus::Exited => {
                            format!("{label} — shell finished and awaits relaunch")
                        }
                    },
                    Style::default().fg(self.theme.hint_desc_fg),
                ),
            ]));
        }

        // GitHub integration status.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "GitHub integration",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            use crate::model::GhStatus;
            let (icon, desc) = if !self.github_integration_enabled {
                (
                    "○",
                    "Disabled — enable via command palette (toggle-github-integration)".to_string(),
                )
            } else {
                match self.gh_status {
                    GhStatus::Unknown => ("◐", "Checking gh CLI availability…".to_string()),
                    GhStatus::NotInstalled => (
                        "⚠",
                        "gh CLI not found — install from https://cli.github.com".to_string(),
                    ),
                    GhStatus::NotAuthenticated => (
                        "⚠",
                        "gh CLI not authenticated — run: gh auth login".to_string(),
                    ),
                    GhStatus::Available => {
                        let count = self.pr_statuses.len();
                        let noun = if count == 1 { "session" } else { "sessions" };
                        ("✓", format!("Active — tracking PRs for {count} {noun}"))
                    }
                }
            };
            let icon_color = match self.gh_status {
                GhStatus::Available if self.github_integration_enabled => self.theme.session_active,
                _ if !self.github_integration_enabled => self.theme.session_exited,
                _ => self.theme.warning_fg,
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(self.theme.hint_desc_fg)),
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
            spans.extend(self.theme.dim_key_badge_default(&move_down));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&move_up));
            spans.push(Span::styled(" or ", desc_style));
            spans.extend(self.theme.dim_key_badge_default("Space"));
            spans.push(Span::styled(" scroll, ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_down));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_up));
            spans.push(Span::styled(" page. ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&close));
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

    fn render_prompt(&mut self, frame: &mut Frame) {
        match &self.prompt {
            PromptState::Command { input, selected } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(72, 40, frame.area());
                self.clear_overlay_area(frame, popup);
                let commands = self.bindings.filtered_palette(&input.text);
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
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            )];
                            let desc_avail = inner_w.saturating_sub(name_col + gap);
                            let desc = binding.palette_description.unwrap_or("");
                            let desc_display =
                                if desc.chars().count() > desc_avail && desc_avail > 1 {
                                    let end: String = desc.chars().take(desc_avail - 1).collect();
                                    format!("  {end}\u{2026}")
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
                let title = "Command Palette";
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " run  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                // Tab autocomplete is text-input behavior, not a rebindable action.
                bottom_spans.extend(self.theme.key_badge_default("Tab"));
                bottom_spans.push(Span::styled(
                    " complete  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                let prompt_prefix = "> ";
                let input_block = self
                    .themed_overlay_block(title)
                    .title_bottom(Line::from(bottom_spans));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    prompt_prefix,
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                ))
                .block(input_block)
                .render(input_area, frame.buffer_mut());
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::Command {
                    input: input_inner,
                    list: list_inner,
                    items: commands.len(),
                    offset: state.offset(),
                };
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
                self.clear_overlay_area(frame, area);
                let visible: Vec<_> = if filter.is_empty() {
                    entries.iter().collect()
                } else {
                    let needle = filter.text.to_lowercase();
                    entries
                        .iter()
                        .filter(|e| e.label.to_lowercase().contains(&needle))
                        .collect()
                };
                let items = if *loading {
                    let idx = self.spinner_frame_index();
                    let spinner = crate::theme::SPINNER_FRAMES[idx];
                    vec![ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{spinner} "),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled("Loading…", Style::default().fg(self.theme.text_fg)),
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
                                Span::styled(
                                    entry.label.clone(),
                                    Style::default().fg(self.theme.text_fg),
                                ),
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
                    let (prefix, text, cursor) = if *editing_path {
                        ("go: ", path_input.text.as_str(), path_input.cursor)
                    } else {
                        ("/ ", filter.text.as_str(), filter.cursor)
                    };
                    let input_block = self.themed_overlay_block(&title);
                    let input_inner = input_block.inner(filter_area);
                    Paragraph::new(render_single_line_cursor_input(
                        prefix,
                        text,
                        cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                    ))
                    .block(input_block)
                    .render(filter_area, frame.buffer_mut());
                    let confirm_key = self.bindings.label_for(Action::Confirm);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    if *editing_path {
                        // Path editor: Tab/Enter/Esc are text-input controls, not rebindable.
                        bottom_spans.extend(self.theme.key_badge_default("Tab"));
                        bottom_spans.push(Span::styled(
                            " complete  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default("Enter"));
                        bottom_spans.push(Span::styled(
                            " go  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default("Esc"));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else if *searching {
                        bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                        bottom_spans.push(Span::styled(
                            " done  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&close_key));
                        bottom_spans.push(Span::styled(
                            " clear",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else {
                        bottom_spans.extend(self.theme.key_badge_default(&search_key));
                        bottom_spans.push(Span::styled(
                            " search  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&open_key));
                        bottom_spans.push(Span::styled(
                            " open  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&goto_key));
                        bottom_spans.push(Span::styled(
                            " go to  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&close_key));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    }
                    let list_block = Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                        .border_style(Style::default().fg(self.theme.overlay_border))
                        .style(Style::default().bg(self.theme.overlay_bg))
                        .title_bottom(Line::from(bottom_spans));
                    let list_inner = list_block.inner(list_render_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::BrowseProjects {
                        input: Some(input_inner),
                        list: list_inner,
                        items: visible.len(),
                        offset: state.offset(),
                    };
                } else {
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let add_key = self.bindings.label_for(Action::AddCurrentDir);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    bottom_spans.extend(self.theme.key_badge_default(&search_key));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&open_key));
                    bottom_spans.push(Span::styled(
                        " open  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&add_key));
                    bottom_spans.push(Span::styled(
                        " add current  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&goto_key));
                    bottom_spans.push(Span::styled(
                        " go to  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&close_key));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    let title = format!("Add Project: {}", current_dir.display());
                    let list_block = self
                        .themed_overlay_block(&title)
                        .title_bottom(Line::from(bottom_spans));
                    let list_inner = list_block.inner(list_render_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::BrowseProjects {
                        input: None,
                        list: list_inner,
                        items: visible.len(),
                        offset: state.offset(),
                    };
                }
            }
            PromptState::ChangeAgentProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 42, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&toggle_key));
                bottom_spans.push(Span::styled(
                    " buttons  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4),
                        Constraint::Min(6),
                        Constraint::Length(3),
                    ])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Agent: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.session_label.as_str(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Path: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.worktree_path.as_str(),
                            Style::default().fg(self.theme.text_fg),
                        ),
                    ]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Agent Provider")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let provider_col = prompt
                    .options
                    .iter()
                    .map(|option| option.provider.as_str().chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let status = if option.is_current {
                            "current"
                        } else if option.resume_available {
                            "a previous session was found; it'll be continued"
                        } else if !option.supports_resume {
                            "this provider doesn't support resume; it'll start fresh every time"
                        } else {
                            "no prior session; will start fresh"
                        };
                        let name =
                            format!("{:width$}", option.provider.as_str(), width = provider_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                name,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {status}"),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                let highlight_style = if matches!(prompt.focus, ChangeAgentProviderFocus::List) {
                    self.theme.selection_style()
                } else {
                    Style::default()
                        .fg(self.theme.help_section_header_fg)
                        .add_modifier(Modifier::BOLD)
                };
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let btn_width = 18u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let apply_enabled = prompt
                    .options
                    .get(prompt.selected)
                    .map(|option| !option.is_current)
                    .unwrap_or(false);

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeAgentProviderCancel,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeAgentProviderFocus::Cancel),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Use Provider")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeAgentProviderApply,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeAgentProviderFocus::Apply),
                        apply_enabled,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ChangeAgentProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                    cancel_button: cancel_area,
                    apply_button: apply_area,
                };
            }
            PromptState::ChangeDefaultProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 42, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&toggle_key));
                bottom_spans.push(Span::styled(
                    " buttons  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4),
                        Constraint::Min(6),
                        Constraint::Length(3),
                    ])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(
                            " Current default: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            prompt.current.as_str().to_string(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![Span::styled(
                        " New sessions will use the chosen provider. Existing agents keep their current provider.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Default Provider")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let provider_col = prompt
                    .options
                    .iter()
                    .map(|option| option.provider.as_str().chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let status = if option.is_current {
                            "current default"
                        } else {
                            "available"
                        };
                        let name =
                            format!("{:width$}", option.provider.as_str(), width = provider_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                name,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {status}"),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                let highlight_style = if matches!(prompt.focus, ChangeDefaultProviderFocus::List) {
                    self.theme.selection_style()
                } else {
                    Style::default()
                        .fg(self.theme.help_section_header_fg)
                        .add_modifier(Modifier::BOLD)
                };
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let btn_width = 18u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let apply_enabled = prompt
                    .options
                    .get(prompt.selected)
                    .map(|option| !option.is_current)
                    .unwrap_or(false);

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeDefaultProviderCancel,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Cancel),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Set Default")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeDefaultProviderApply,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Apply),
                        apply_enabled,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ChangeDefaultProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                    cancel_button: cancel_area,
                    apply_button: apply_area,
                };
            }
            PromptState::ChangeTheme(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 60, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " apply  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(6)])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(
                            " Current theme: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            prompt.current.clone(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![Span::styled(
                        " Selecting a theme applies it instantly and saves it to config.toml.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Theme")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let id_col = prompt
                    .options
                    .iter()
                    .map(|option| option.id.chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let source_label = match option.source {
                            crate::theme::ThemeSource::Bundled => "bundled",
                            crate::theme::ThemeSource::Opaline => "opaline",
                            crate::theme::ThemeSource::User => "user",
                        };
                        let is_current = option.id == prompt.current;
                        let suffix = if is_current {
                            format!("  {source_label} · current")
                        } else {
                            format!("  {source_label}")
                        };
                        let id_padded = format!("{:width$}", option.id, width = id_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                id_padded,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {}", option.display_name),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                            Span::styled(suffix, Style::default().fg(self.theme.hint_dim_desc_fg)),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                self.overlay_layout.active = OverlayMouseLayout::ChangeTheme {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                };
            }
            PromptState::PickEditor {
                session_label,
                worktree_path,
                editors,
                selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(64, 34, frame.area());
                self.clear_overlay_area(frame, area);

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " open  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
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
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Path: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            worktree_path.as_str(),
                            Style::default().fg(self.theme.text_fg),
                        ),
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
                                .fg(self.theme.help_section_header_fg)
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
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::PickEditor {
                    list: list_inner,
                    items: editors.len(),
                    offset: state.offset(),
                };
            }
            PromptState::KillRunning(prompt) => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(78, 72, frame.area());
                self.clear_overlay_area(frame, popup);

                let visible_indices = Self::visible_kill_running_indices(prompt);
                let items = if visible_indices.is_empty() {
                    vec![ListItem::new("No matching running agents or terminals.")]
                } else {
                    let label_col = visible_indices
                        .iter()
                        .filter_map(|index| prompt.runtimes.get(*index))
                        .map(|runtime| runtime.label.chars().count())
                        .max()
                        .unwrap_or(0)
                        .min(28);
                    visible_indices
                        .iter()
                        .filter_map(|index| prompt.runtimes.get(*index))
                        .map(|runtime| {
                            let checkbox = Checkbox::new("")
                                .checked(prompt.selected_ids.contains(&runtime.id))
                                .state(CheckboxState::Normal);
                            let label = if runtime.label.chars().count() > label_col {
                                runtime.label.chars().take(label_col).collect::<String>()
                            } else {
                                runtime.label.clone()
                            };
                            let label_padded = format!("{label:label_col$}");
                            let kind_color = match runtime.kind {
                                KillableRuntimeKind::Agent => self.theme.session_active,
                                KillableRuntimeKind::Terminal => self.theme.session_detached,
                            };
                            let mut spans =
                                checkbox.inline_prefix(Style::default().fg(self.theme.hint_key_fg));
                            spans.extend([
                                Span::styled(
                                    format!("{:>6} ", runtime.kind.badge()),
                                    Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    label_padded,
                                    Style::default().add_modifier(Modifier::BOLD),
                                ),
                            ]);
                            spans.extend(runtime_context_spans(
                                &format!("  {}", runtime.context),
                                Style::default()
                                    .fg(self.theme.hint_dim_desc_fg)
                                    .add_modifier(Modifier::DIM),
                                Style::default().fg(self.theme.runtime_context_value_fg),
                            ));
                            ListItem::new(Line::from(spans))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default().with_selected(Some(
                    prompt
                        .hovered_visible_index
                        .min(visible_indices.len().saturating_sub(1)),
                ));
                let show_top_input = prompt.searching || !prompt.filter.is_empty();
                let (top_area, body_area) = if show_top_input {
                    let [input_area, rest] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(6)])
                        .areas(popup);
                    (Some(input_area), rest)
                } else {
                    (None, popup)
                };
                let [list_area, legend_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(6),
                        Constraint::Length(2),
                        Constraint::Length(3),
                    ])
                    .areas(body_area);

                let search_key = self.bindings.label_for(Action::SearchToggle);
                let toggle_key = self.bindings.label_for(Action::ToggleMarked);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let next_key = self.bindings.label_for(Action::FocusNext);
                let prev_key = self.bindings.label_for(Action::FocusPrev);
                let mut hint_spans = vec![Span::raw(" ")];
                if prompt.searching {
                    hint_spans.extend(self.theme.key_badge_default(&confirm_key));
                    hint_spans.push(Span::styled(
                        " done  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&close_key));
                    hint_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    hint_spans.extend(self.theme.key_badge_default(&toggle_key));
                    hint_spans.push(Span::styled(
                        " select  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&search_key));
                    hint_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&next_key));
                    hint_spans.push(Span::styled(
                        "/",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&prev_key));
                    hint_spans.push(Span::styled(
                        " actions  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&confirm_key));
                    hint_spans.push(Span::styled(
                        " use",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }

                let title = if prompt.searching {
                    "Kill Running (searching)"
                } else {
                    "Kill Running"
                };
                if let Some(input_area) = top_area {
                    let input_block = self.themed_overlay_block(title);
                    let input_inner = input_block.inner(input_area);
                    Paragraph::new(render_single_line_cursor_input(
                        "/ ",
                        &prompt.filter.text,
                        prompt.filter.cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                    ))
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());
                    let list_block = Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::default().fg(self.theme.overlay_border))
                        .style(Style::default().bg(self.theme.overlay_bg))
                        .title_bottom(Line::from(hint_spans));
                    let list_inner = list_block.inner(list_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                        input: Some(input_inner),
                        list: list_inner,
                        items: visible_indices.len(),
                        offset: state.offset(),
                        cancel_button: Rect::default(),
                        hovered_button: Rect::default(),
                        selected_button: Rect::default(),
                        visible_button: Rect::default(),
                    };
                } else {
                    let list_block = self
                        .themed_overlay_block(title)
                        .title_bottom(Line::from(hint_spans));
                    let list_inner = list_block.inner(list_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                        input: None,
                        list: list_inner,
                        items: visible_indices.len(),
                        offset: state.offset(),
                        cancel_button: Rect::default(),
                        hovered_button: Rect::default(),
                        selected_button: Rect::default(),
                        visible_button: Rect::default(),
                    };
                }

                let legend = Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Legend: ", Style::default().fg(self.theme.hint_desc_fg)),
                    Span::styled(
                        "AGENT",
                        Style::default()
                            .fg(self.theme.session_active)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " = running agent CLI  |  ",
                        Style::default().fg(self.theme.hint_dim_desc_fg),
                    ),
                    Span::styled(
                        "TERM",
                        Style::default()
                            .fg(self.theme.session_detached)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " = companion terminal  |  dim text = source context",
                        Style::default().fg(self.theme.hint_dim_desc_fg),
                    ),
                    Span::raw("  "),
                ]);
                Paragraph::new(legend)
                    .wrap(Wrap { trim: false })
                    .render(legend_area, frame.buffer_mut());

                let buttons = [
                    KillRunningFooterAction::Cancel,
                    KillRunningFooterAction::Hovered,
                    KillRunningFooterAction::Selected,
                    KillRunningFooterAction::Visible,
                ];
                let gap = 2u16;
                // This footer pre-dates the Button widget's standard sizing —
                // its labels are long ("Kill Hovered" etc.) and it lays four
                // buttons in a row. The wider per-label width (label_chars + 6)
                // keeps the row visually balanced; Button still handles the
                // colors and bold-when-enabled rules.
                let button_widths = buttons.map(|action| {
                    let label_chars =
                        u16::try_from(action.button_label().chars().count()).unwrap_or(u16::MAX);
                    label_chars.saturating_add(6)
                });
                let total_width = button_widths.iter().sum::<u16>() + gap * 3;
                let start_x = buttons_area.x + buttons_area.width.saturating_sub(total_width) / 2;
                let mut cursor_x = start_x;
                let mut button_rects = [Rect::default(); 4];
                for (index, action) in buttons.iter().enumerate() {
                    let rect = Rect {
                        x: cursor_x,
                        y: buttons_area.y,
                        width: button_widths[index],
                        height: 3,
                    };
                    button_rects[index] = rect;
                    let enabled = Self::kill_running_footer_enabled(prompt, *action);
                    let focused = matches!(
                        prompt.focus,
                        KillRunningFocus::Footer(current) if current == *action
                    );
                    let kind = if matches!(action, KillRunningFooterAction::Cancel) {
                        ButtonKind::Confirm
                    } else {
                        ButtonKind::Danger
                    };
                    let press_target = match action {
                        KillRunningFooterAction::Cancel => ButtonPressedTarget::RuntimeKillCancel,
                        KillRunningFooterAction::Hovered => ButtonPressedTarget::RuntimeKillHovered,
                        KillRunningFooterAction::Selected => {
                            ButtonPressedTarget::RuntimeKillSelected
                        }
                        KillRunningFooterAction::Visible => ButtonPressedTarget::RuntimeKillVisible,
                    };
                    let state =
                        button_state_for(press_target, self.pressed_button, focused, enabled);
                    Button::new(action.button_label())
                        .kind(kind)
                        .state(state)
                        .render(frame, rect, &self.theme);
                    cursor_x += button_widths[index] + gap;
                }
                self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                    input: match self.overlay_layout.active {
                        OverlayMouseLayout::KillRunning { input, .. } => input,
                        _ => None,
                    },
                    list: match self.overlay_layout.active {
                        OverlayMouseLayout::KillRunning { list, .. } => list,
                        _ => Rect::default(),
                    },
                    items: visible_indices.len(),
                    offset: state.offset(),
                    cancel_button: button_rects[0],
                    hovered_button: button_rects[1],
                    selected_button: button_rects[2],
                    visible_button: button_rects[3],
                };
            }
            PromptState::ConfirmKillRunning(confirm_prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 32, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Confirm Kill");
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

                let targets = confirm_prompt.target_ids.len();
                let (agent_count, terminal_count) = confirm_prompt.target_ids.iter().fold(
                    (0usize, 0usize),
                    |(agents, terminals), target_id| match target_id {
                        RuntimeTargetId::Agent(_) => (agents + 1, terminals),
                        RuntimeTargetId::Terminal(_) => (agents, terminals + 1),
                    },
                );
                let mut summary = Vec::new();
                if agent_count > 0 {
                    summary.push(format!(
                        "{agent_count} agent{}",
                        if agent_count == 1 { "" } else { "s" }
                    ));
                }
                if terminal_count > 0 {
                    summary.push(format!(
                        "{terminal_count} terminal{}",
                        if terminal_count == 1 { "" } else { "s" }
                    ));
                }
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(
                            " {} will stop ",
                            confirm_prompt.action.button_label()
                        )),
                        Span::styled(
                            format!(
                                "{targets} running process{}",
                                if targets == 1 { "" } else { "es" }
                            ),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("."),
                    ]),
                    Line::from(format!(" Affected: {}", summary.join(" and "))),
                    Line::from(""),
                    Line::from(Span::styled(
                        " In-progress CLI work will be lost immediately.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(Span::styled(
                        " Worktree files remain on disk for review or relaunch.",
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
                let kill_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmKillCancel,
                        self.pressed_button,
                        !confirm_prompt.confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Kill")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmKillConfirm,
                        self.pressed_button,
                        confirm_prompt.confirm_selected,
                        true,
                    ))
                    .render(frame, kill_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmKillRunning {
                    cancel_button: cancel_area,
                    kill_button: kill_area,
                };
            }
            PromptState::ConfirmDeleteAgent {
                branch_name,
                focus,
                delete_worktree,
                worktree_shared,
                ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 56.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_height = if *worktree_shared {
                    0
                } else {
                    let state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let checkbox = Checkbox::new("Also delete the worktree and branch")
                        .checked(*delete_worktree)
                        .state(state);
                    checkbox
                        .layout(
                            inner_width,
                            checkbox.marker_style(Style::default()),
                            checkbox.label_style(Style::default()),
                        )
                        .height
                };

                // Body: question text + conditional warning/hint/shared note.
                // Long warning text is split into two explicit Lines so it
                // renders correctly even at narrow dialog widths.
                let mut body_lines = vec![
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
                ];
                if *worktree_shared {
                    body_lines.push(Line::from(Span::styled(
                        " Worktree is shared with another agent and will be preserved.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                } else if *delete_worktree {
                    body_lines.push(Line::from(Span::styled(
                        " All uncommitted and unpushed changes in this",
                        Style::default().fg(self.theme.warning_fg),
                    )));
                    body_lines.push(Line::from(Span::styled(
                        " worktree will be permanently lost.",
                        Style::default().fg(self.theme.warning_fg),
                    )));
                } else {
                    body_lines.push(Line::from(Span::styled(
                        " Worktree and branch will be preserved on disk.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let checkbox_spacing = u16::from(!*worktree_shared);
                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + checkbox_spacing + checkbox_height + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Delete Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let checkbox_rect = if !*worktree_shared {
                    let checkbox_state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        "Also delete the worktree and branch",
                        *delete_worktree,
                        checkbox_state,
                        None,
                    );
                    Some(OverlayCheckbox {
                        id: OverlayCheckboxId::DeleteAgentWorktree,
                        rect,
                    })
                } else {
                    None
                };

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

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteCancel,
                        self.pressed_button,
                        *focus == DeleteAgentFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteConfirm,
                        self.pressed_button,
                        *focus == DeleteAgentFocus::Delete,
                        true,
                    ))
                    .render(frame, delete_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteAgent {
                    cancel_button: cancel_area,
                    delete_button: delete_area,
                    checkbox: checkbox_rect,
                };
            }
            PromptState::ConfirmDeleteTerminal {
                terminal_label,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Delete Terminal");
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
                        Span::raw(" Are you sure you want to delete "),
                        Span::styled(
                            terminal_label.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " The running process will be killed.",
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
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteTerminalCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteTerminalConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, delete_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteTerminal {
                    cancel_button: cancel_area,
                    delete_button: delete_area,
                };
            }
            PromptState::ConfirmQuit {
                agent_count,
                terminal_count,
                confirm_selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
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

                let process_desc = quit_process_description(*agent_count, *terminal_count);
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(" {process_desc} will be ")),
                        Span::styled(
                            "killed",
                            Style::default()
                                .fg(self.theme.button_danger_border)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" if you quit."),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Any in-progress work will be lost.",
                        Style::default().fg(self.theme.warning_fg),
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

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmQuitCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Quit")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmQuitConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, quit_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmQuit {
                    cancel_button: cancel_area,
                    quit_button: quit_area,
                };
            }
            PromptState::ConfirmDiscardFile {
                file_path,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
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

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDiscardCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Discard")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDiscardConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, discard_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDiscardFile {
                    cancel_button: cancel_area,
                    discard_button: discard_area,
                };
            }
            PromptState::ConfirmNonDefaultBranch {
                current_branch,
                kind,
                focus,
                checkout_default,
                ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 60u16.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let has_checkbox = matches!(kind, BranchWarningKind::Known { .. });

                // Body: warning text + the "new worktrees branch from …" note,
                // plus a dim info line on the heuristic path explaining why dux
                // won't offer to switch branches automatically.
                let mut body_lines = vec![Line::from("")];
                match kind {
                    BranchWarningKind::Known { default_branch } => {
                        body_lines.push(Line::from(vec![
                            Span::raw(" This repository is on branch "),
                            Span::styled(
                                current_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(", but the"),
                        ]));
                        body_lines.push(Line::from(vec![
                            Span::raw(" remote default branch is "),
                            Span::styled(
                                default_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("."),
                        ]));
                    }
                    BranchWarningKind::Heuristic => {
                        body_lines.push(Line::from(vec![
                            Span::raw(" This repository is on branch "),
                            Span::styled(
                                current_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(","),
                        ]));
                        body_lines.push(Line::from(" which doesn't appear to be the main branch."));
                    }
                }
                body_lines.push(Line::from(""));
                body_lines.push(Line::from(Span::styled(
                    format!(" New worktrees will branch from \"{current_branch}\"."),
                    Style::default().fg(self.theme.warning_fg),
                )));
                if matches!(kind, BranchWarningKind::Heuristic) {
                    body_lines.push(Line::from(""));
                    body_lines.push(Line::from(Span::styled(
                        " Dux can't confidently identify this repo's default",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                    body_lines.push(Line::from(Span::styled(
                        " branch, so it won't change branches for you.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);

                // Checkbox height is measured up-front so the outer rect can
                // be sized exactly — mirrors the Delete Agent modal.
                let checkbox_height = if let BranchWarningKind::Known { default_branch } = kind {
                    let state = if *focus == ConfirmNonDefaultBranchFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let label = format!("Check out \"{default_branch}\" before adding");
                    let checkbox = Checkbox::new(&label)
                        .checked(*checkout_default)
                        .state(state);
                    checkbox
                        .layout(
                            inner_width,
                            checkbox.marker_style(Style::default()),
                            checkbox.label_style(Style::default()),
                        )
                        .height
                } else {
                    0
                };
                let checkbox_spacing = u16::from(has_checkbox);

                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + checkbox_spacing + checkbox_height + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Non-Default Branch");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let checkbox_rect = if let BranchWarningKind::Known { default_branch } = kind {
                    let checkbox_state = if *focus == ConfirmNonDefaultBranchFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let label = format!("Check out \"{default_branch}\" before adding");
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        &label,
                        *checkout_default,
                        checkbox_state,
                        None,
                    );
                    Some(OverlayCheckbox {
                        id: OverlayCheckboxId::NonDefaultBranchCheckoutDefault,
                        rect,
                    })
                } else {
                    None
                };

                // Both buttons share a single width derived from the longest
                // label that could appear in either slot. Including
                // "Check Out & Add" and "Add Anyway" in the calculation keeps
                // the layout stable when the user toggles the checkbox —
                // otherwise the buttons would resize mid-modal.
                let btn_width = shared_button_width(&["Cancel", "Add Anyway", "Check Out & Add"]);
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let add_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                // Swap the confirm button label so the user sees exactly what
                // pressing it will do. When the checkbox is on and we know the
                // default branch, the action is a two-step (switch + add),
                // otherwise it's the original "Add Anyway" add-as-is.
                let add_label = if has_checkbox && *checkout_default {
                    "Check Out & Add"
                } else {
                    "Add Anyway"
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmNonDefaultBranchCancel,
                        self.pressed_button,
                        *focus == ConfirmNonDefaultBranchFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new(add_label)
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmNonDefaultBranchAdd,
                        self.pressed_button,
                        *focus == ConfirmNonDefaultBranchFocus::Add,
                        true,
                    ))
                    .render(frame, add_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmNonDefaultBranch {
                    cancel_button: cancel_area,
                    add_button: add_area,
                    checkbox: checkbox_rect,
                };
            }
            PromptState::ConfirmUseExistingBranch {
                branch_name,
                location,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Branch Already Exists");
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

                let location_label = match location {
                    crate::git::BranchLocation::Local => "local",
                    crate::git::BranchLocation::Remote => "remote",
                };
                let mut lines = vec![Line::from("")];
                lines.push(Line::from(vec![
                    Span::raw(" A "),
                    Span::raw(location_label),
                    Span::raw(" branch named "),
                    Span::styled(
                        branch_name.as_str(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(" already exists."));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " A new worktree will be created using this branch,",
                    Style::default().fg(self.theme.warning_fg),
                )));
                lines.push(Line::from(Span::styled(
                    " allowing you to continue working on it.",
                    Style::default().fg(self.theme.warning_fg),
                )));
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
                let use_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmUseExistingBranchCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                // "Use Existing" reuses a branch that already exists — not
                // destructive, so it shares the Confirm kind with Cancel.
                Button::new("Use Existing")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmUseExistingBranchUse,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, use_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmUseExistingBranch {
                    cancel_button: cancel_area,
                    use_button: use_area,
                };
            }
            PromptState::RenameSession {
                input,
                rename_branch,
                ..
            } => {
                self.render_dim_overlay(frame);
                let checkbox = Checkbox::new("Also rename the git branch")
                    .checked(*rename_branch)
                    .state(CheckboxState::Normal);
                let dialog_width = 62.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_height = checkbox
                    .layout(
                        inner_width,
                        checkbox.marker_style(Style::default()),
                        checkbox.label_style(Style::default()),
                    )
                    .height
                    .saturating_add(1);
                let checkbox_spacing = 1;
                let area = centered_rect_exact(
                    dialog_width,
                    9 + checkbox_spacing + checkbox_height,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Rename Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, _, checkbox_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                Paragraph::new(Line::from(Span::styled(
                    " Enter a new name (empty to reset):",
                    Style::default().fg(self.theme.input_label_fg),
                )))
                .render(label_area, frame.buffer_mut());

                // Show the input with a cursor indicator.
                let display = if input.cursor < input.text.len() {
                    let (before, after) = input.text.split_at(input.cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", &input.text)),
                        Span::styled(
                            " ",
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                    ])
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(display)
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());

                let (checkbox_rect, _) = self.render_overlay_checkbox(
                    frame,
                    checkbox_area,
                    "Also rename the git branch",
                    *rename_branch,
                    CheckboxState::Normal,
                    Some(Line::from(Span::styled(
                        format!(
                            "{}Open PRs will still reference the old branch name",
                            Checkbox::indent()
                        ),
                        Style::default().fg(self.theme.hint_desc_fg),
                    ))),
                );

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&toggle_key));
                hints.push(Span::styled(
                    " toggle  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&close_key));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::RenameSession {
                    input: input_inner,
                    checkbox: Some(OverlayCheckbox {
                        id: OverlayCheckboxId::RenameSessionBranch,
                        rect: checkbox_rect,
                    }),
                };
            }
            PromptState::EditMacros { .. } => {
                // Full rendering implemented in Task #5.
                self.render_edit_macros(frame);
            }
            PromptState::ResourceMonitor {
                rows,
                scroll_offset,
                selected_row,
                expanded,
                first_sample,
                ..
            } => {
                let rows = rows.clone();
                let scroll_offset = *scroll_offset;
                let selected_row = *selected_row;
                let expanded = expanded.clone();
                let first_sample = *first_sample;
                self.render_resource_monitor(
                    frame,
                    &rows,
                    scroll_offset,
                    selected_row,
                    &expanded,
                    first_sample,
                );
            }
            PromptState::DebugInput {
                lines,
                scroll_offset,
            } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(80, 70, frame.area());
                self.clear_overlay_area(frame, popup);

                // Split: content area + 1-line footer hint.
                let chunks =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(popup);
                let content_area = chunks[0];
                let hint_area = chunks[1];

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .title(" Input Debugger ")
                    .title_style(
                        Style::default()
                            .fg(self.theme.help_section_header_fg)
                            .add_modifier(Modifier::BOLD),
                    );
                let inner = block.inner(content_area);
                block.render(content_area, frame.buffer_mut());

                // Compute the visible window.
                let visible_h = inner.height as usize;
                let total = lines.len();
                let max_offset = total.saturating_sub(visible_h);
                let offset = (*scroll_offset as usize).min(max_offset);

                // When scroll_offset exceeds max (auto-scroll sentinel), pin to bottom.
                let start = if *scroll_offset as usize >= total {
                    max_offset
                } else {
                    offset
                };

                let visible: Vec<Line> =
                    lines.iter().skip(start).take(visible_h).cloned().collect();

                let paragraph = Paragraph::new(visible);
                paragraph.render(inner, frame.buffer_mut());

                // Footer hint.
                let hint = Line::from(vec![
                    Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" close  "),
                    Span::styled("Scroll", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" navigate"),
                ]);
                let hint_para = Paragraph::new(hint)
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(self.theme.hint_desc_fg)
                            .add_modifier(Modifier::DIM),
                    );
                hint_para.render(hint_area, frame.buffer_mut());
            }
            PromptState::NameNewAgent {
                input,
                randomize_name,
                focus,
                ..
            } => {
                self.render_dim_overlay(frame);
                let checkbox = Checkbox::new("Use randomized pet name")
                    .checked(*randomize_name)
                    .state(if *focus == NameNewAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    });
                let dialog_width = 60.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_height = checkbox
                    .layout(
                        inner_width,
                        checkbox.marker_style(Style::default()),
                        checkbox.label_style(Style::default()),
                    )
                    .height
                    .saturating_add(1);
                let checkbox_spacing = 1;
                let area = centered_rect_exact(
                    dialog_width,
                    8 + checkbox_spacing + checkbox_height,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Name New Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, _, checkbox_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                Paragraph::new(Line::from(Span::styled(
                    " Enter a name for the new agent (used as branch name):",
                    Style::default().fg(self.theme.input_label_fg),
                )))
                .render(label_area, frame.buffer_mut());

                // Input field with cursor indicator.
                let display = if input.cursor < input.text.len() {
                    let (before, after) = input.text.split_at(input.cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", &input.text)),
                        Span::styled(
                            " ",
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                    ])
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(display)
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());

                let (checkbox_rect, _) = self.render_overlay_checkbox(
                    frame,
                    checkbox_area,
                    "Use randomized pet name",
                    *randomize_name,
                    if *focus == NameNewAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    },
                    Some(Line::from(Span::styled(
                        format!(
                            "{}Fills this prompt with a fresh pet-tool name",
                            Checkbox::indent()
                        ),
                        Style::default().fg(self.theme.hint_desc_fg),
                    ))),
                );

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&toggle_key));
                hints.push(Span::styled(
                    " randomize  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&close_key));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::NameNewAgent {
                    input: input_inner,
                    checkbox: Some(OverlayCheckbox {
                        id: OverlayCheckboxId::NameNewAgentRandomizedPetName,
                        rect: checkbox_rect,
                    }),
                };
            }
            PromptState::None => {}
        }
    }

    fn render_edit_macros(&mut self, frame: &mut Frame) {
        use super::MacroEditStage;

        // Pre-compute the popup layout so we can set the display width for
        // soft-wrapping before taking the immutable borrow on self.prompt.
        let popup = centered_rect_exact(64, 20, frame.area());
        {
            // Temporarily borrow prompt mutably to set the text input's
            // display width to match the available inner area.
            if let PromptState::EditMacros {
                editing: Some(edit_state),
                ..
            } = &mut self.prompt
                && edit_state.stage == MacroEditStage::EditText
            {
                // Replicate the layout chain to compute actual text width:
                // popup → outer border inner → layout (label + text + hints)
                // → text border inner → minus leading space(1).
                let outer_block = Block::bordered();
                let outer_inner = outer_block.inner(popup);
                let text_bordered = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(outer_inner)[1];
                let inner_block = Block::bordered();
                let text_inner = inner_block.inner(text_bordered);
                // Subtract 1 for the leading space prefix on each rendered line.
                let wrap_w = text_inner.width.saturating_sub(1) as usize;
                edit_state.text_input.set_display_width(if wrap_w > 0 {
                    Some(wrap_w)
                } else {
                    None
                });
            }
        }

        let PromptState::EditMacros {
            entries,
            selected,
            editing,
            pending_delete,
        } = &self.prompt
        else {
            return;
        };

        self.render_dim_overlay(frame);
        self.clear_overlay_area(frame, popup);

        if let Some(edit_state) = editing {
            // ── Edit view ──
            let title = match &edit_state.id {
                Some(name) => format!("Edit Macro — {name}"),
                None => "New Macro".to_string(),
            };
            let outer = self.themed_overlay_block(&title);
            let inner = outer.inner(popup);
            outer.render(popup, frame.buffer_mut());

            match edit_state.stage {
                MacroEditStage::EditName => {
                    let [label_area, input_area, _, surface_area, _, hint_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Length(3),
                            Constraint::Length(1),
                            Constraint::Length(1),
                            Constraint::Min(1),
                            Constraint::Length(1),
                        ])
                        .areas(inner);

                    Paragraph::new(Line::from(Span::styled(
                        " Name (identifies this macro):",
                        Style::default().fg(self.theme.input_label_fg),
                    )))
                    .render(label_area, frame.buffer_mut());

                    self.render_single_line_input(&edit_state.name_input, input_area, frame);

                    // Surface radio buttons
                    let current = edit_state.surface;
                    let options = [
                        (MacroSurface::Agent, "Agent"),
                        (MacroSurface::Terminal, "Terminal"),
                        (MacroSurface::Both, "Both"),
                    ];
                    let mut radio_spans: Vec<Span> = vec![Span::styled(
                        " Surface:  ",
                        Style::default().fg(self.theme.input_label_fg),
                    )];
                    for (i, (variant, label)) in options.iter().enumerate() {
                        if i > 0 {
                            radio_spans.push(Span::styled("    ", Style::default()));
                        }
                        let bullet = if *variant == current { "● " } else { "○ " };
                        let style = if *variant == current {
                            Style::default().fg(self.theme.input_label_fg)
                        } else {
                            Style::default().fg(self.theme.hint_desc_fg)
                        };
                        radio_spans.push(Span::styled(bullet, style));
                        radio_spans.push(Span::styled(*label, style));
                    }
                    Paragraph::new(Line::from(radio_spans))
                        .render(surface_area, frame.buffer_mut());

                    let hints = self.edit_macro_hints(&[
                        ("Enter", "next"),
                        ("Tab/Shift-Tab", "surface"),
                        ("Esc", "cancel"),
                    ]);
                    Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                }
                MacroEditStage::EditText => {
                    let [label_area, bordered_area, hint_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(3),
                            Constraint::Length(1),
                        ])
                        .areas(inner);

                    let surface_desc = match edit_state.surface {
                        MacroSurface::Agent => "agent macro",
                        MacroSurface::Terminal => "terminal macro",
                        MacroSurface::Both => "agent + terminal macro",
                    };
                    Paragraph::new(Line::from(Span::styled(
                        format!(" Text for the {surface_desc}:"),
                        Style::default().fg(self.theme.input_label_fg),
                    )))
                    .render(label_area, frame.buffer_mut());

                    // Draw border around the text area; pass inner rect to renderer.
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(self.theme.overlay_border));
                    let text_inner = block.inner(bordered_area);
                    block.render(bordered_area, frame.buffer_mut());

                    self.render_multiline_input(&edit_state.text_input, text_inner, frame);

                    let hints =
                        self.edit_macro_hints(&[("Enter", "newline"), ("Esc", "save & close")]);
                    Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                }
            }
        } else {
            // ── List view ──
            let outer = self.themed_overlay_block("Text Macros");
            let inner = outer.inner(popup);
            outer.render(popup, frame.buffer_mut());

            if entries.is_empty() {
                let [msg_area, _, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(1),
                        Constraint::Length(1),
                    ])
                    .areas(inner);

                Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        " No macros defined. Press n to create one.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ])
                .render(msg_area, frame.buffer_mut());

                let hints = self.edit_macro_hints(&[("n", "new"), ("Esc", "close")]);
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
            } else {
                let [list_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(1)])
                    .areas(inner);

                let items: Vec<ListItem> = entries
                    .iter()
                    .map(|(name, text, surface)| {
                        let surface_label = format!(" ({})", surface.label());
                        let mut spans = vec![
                            Span::styled(
                                format!(" {name}"),
                                Style::default().fg(self.theme.input_label_fg),
                            ),
                            Span::styled(
                                surface_label.clone(),
                                Style::default().fg(self.theme.hint_dim_desc_fg),
                            ),
                            Span::styled(" — ", Style::default().fg(self.theme.input_label_fg)),
                        ];
                        let text_preview = text.replace('\n', "↵");
                        // " " + name + " (label)" + " — "
                        let prefix_len = 1 + name.len() + surface_label.len() + 3;
                        let max_len = (list_area.width as usize).saturating_sub(prefix_len + 2);
                        let truncated = if text_preview.len() > max_len {
                            format!("{}…", &text_preview[..max_len.saturating_sub(1)])
                        } else {
                            text_preview
                        };
                        spans.push(Span::styled(
                            truncated,
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let list = List::new(items)
                    .highlight_style(self.theme.selection_style())
                    .highlight_symbol("");
                let mut state = ratatui::widgets::ListState::default();
                state.select(Some(*selected));
                ratatui::prelude::StatefulWidget::render(
                    list,
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let hints = self.edit_macro_hints(&[
                    ("Enter", "edit"),
                    ("n", "new"),
                    ("d", "delete"),
                    ("Esc", "close"),
                ]);
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
            }
        }

        let pending_delete_snapshot = pending_delete
            .as_ref()
            .map(|p| (p.name.clone(), p.confirm_selected));
        if let Some((name, confirm_selected)) = pending_delete_snapshot {
            self.render_confirm_delete_macro(frame, &name, confirm_selected);
        }
    }

    fn render_confirm_delete_macro(
        &mut self,
        frame: &mut Frame,
        name: &str,
        confirm_selected: bool,
    ) {
        self.render_dim_overlay(frame);
        let area = centered_rect(56, 30, frame.area());
        self.clear_overlay_area(frame, area);
        let outer = self.themed_overlay_block("Delete Macro");
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
                Span::raw(" Are you sure you want to delete "),
                Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("?"),
            ]),
            Line::from(""),
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
        let delete_area = Rect {
            x: cancel_area.x + btn_width + gap,
            y: buttons_area.y,
            width: btn_width,
            height: 3,
        };

        Button::new("Cancel")
            .kind(ButtonKind::Confirm)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteMacroCancel,
                self.pressed_button,
                !confirm_selected,
                true,
            ))
            .render(frame, cancel_area, &self.theme);

        Button::new("Delete")
            .kind(ButtonKind::Danger)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteMacroConfirm,
                self.pressed_button,
                confirm_selected,
                true,
            ))
            .render(frame, delete_area, &self.theme);

        self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteMacro {
            cancel_button: cancel_area,
            delete_button: delete_area,
        };
    }

    /// Render a single-line TextInput with cursor in a bordered box.
    /// Uses the terminal's hardware cursor for a blinking caret.
    fn render_single_line_input(&self, input: &TextInput, area: Rect, frame: &mut Frame) {
        let display = Line::from(Span::raw(format!(" {}", &input.text)));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border));
        let inner = block.inner(area);
        Paragraph::new(display)
            .block(block)
            .render(area, frame.buffer_mut());

        // Position the hardware cursor (blinking caret).
        // Cursor column in chars + 1 for the leading space padding.
        let cursor_col = input.text[..input.cursor.min(input.text.len())]
            .chars()
            .count();
        let cx = inner.x + cursor_col as u16 + 1;
        let cy = inner.y;
        if cx < inner.x + inner.width && cy < inner.y + inner.height {
            frame.set_cursor_position((cx, cy));
        }
    }

    /// Render a multiline TextInput into the given area.
    ///
    /// The caller is responsible for drawing any border — this method renders
    /// text directly into `area`. Uses the terminal's hardware cursor for a
    /// blinking caret.
    fn render_multiline_input(&self, input: &TextInput, area: Rect, frame: &mut Frame) {
        let visible = input.visible_lines();
        let (cursor_row, cursor_col) = input.cursor_display_position();

        for (i, line_text) in visible.iter().enumerate() {
            if i >= area.height as usize {
                break;
            }
            let y = area.y + i as u16;
            let line_area = Rect::new(area.x, y, area.width, 1);
            let line = Line::from(Span::raw(format!(" {line_text}")));
            Paragraph::new(line).render(line_area, frame.buffer_mut());
        }

        // Position the hardware cursor (blinking caret).
        // +1 for the leading space padding on each line.
        let cx = area.x + cursor_col as u16 + 1;
        let cy = area.y + cursor_row as u16;
        if cx < area.x + area.width && cy < area.y + area.height {
            frame.set_cursor_position((cx, cy));
        }
    }

    /// Build hint spans from alternating key/description pairs.
    /// Each pair is (key_label, description). Spans are fully owned.
    fn edit_macro_hints(&self, pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        for (key, desc) in pairs {
            // key_badge ties lifetime to &str, so we convert to owned spans.
            let badge = self.theme.key_badge_default(key);
            spans.extend(
                badge
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            spans.push(Span::styled(
                format!(" {desc}  "),
                Style::default().fg(self.theme.hint_desc_fg),
            ));
        }
        spans
    }

    fn render_overlay(&mut self, frame: &mut Frame) {
        match self.fullscreen_overlay {
            FullscreenOverlay::Agent => {
                self.render_fullscreen_agent(frame);
                return;
            }
            FullscreenOverlay::Terminal => {
                self.render_fullscreen_terminal(frame);
                return;
            }
            FullscreenOverlay::None => {}
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
        // Clear leaves cells at Color::Reset (terminal default). Repaint
        // with app_bg so the fullscreen agent surface — borders, the
        // loading card area, the gap above the hint bar — tracks the
        // active theme instead of falling through to the user's terminal
        // default.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));
        let title = match self.selected_session() {
            Some(session) => {
                let provider = capitalize(self.running_provider_for(session).as_str());
                let name = session.title.as_deref().unwrap_or(&session.branch_name);
                let pr_suffix = self
                    .pr_statuses
                    .get(&session.id)
                    .map(|pr| format!(" · {}#{}", pr.owner_repo, pr.number))
                    .unwrap_or_default();
                format!(" {provider} agent · {name}{pr_suffix} ")
            }
            None => " Agent ".to_string(),
        };
        let saved = self.session_surface;
        self.session_surface = SessionSurface::Agent;
        self.render_agent_terminal(frame, area, &title, true);
        self.session_surface = saved;
    }

    fn render_fullscreen_terminal(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(96, 94, frame.area());
        Clear.render(area, frame.buffer_mut());
        // Same reasoning as render_fullscreen_agent — fill with app_bg so
        // the fullscreen terminal surface follows the active theme.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));
        let saved = self.session_surface;
        self.session_surface = SessionSurface::Terminal;
        self.render_agent_terminal(frame, area, " Terminal ", true);
        self.session_surface = saved;
    }

    fn render_macro_bar(&mut self, frame: &mut Frame, area: Rect) {
        let (query, selected, cursor, cursor_fg, cursor_bg) = {
            let Some(bar) = &self.macro_bar else {
                return;
            };
            (
                bar.input.text.clone(),
                bar.selected,
                bar.input.cursor.min(bar.input.text.len()),
                self.theme.input_cursor_fg,
                self.theme.input_cursor_bg,
            )
        };

        let filtered = self.filtered_macros(&query);

        // Compute total height: input block (3) + list block (variable).
        // The list block shares borders with the input block (no top border).
        let list_content_h = (filtered.len() as u16).clamp(1, 8);
        // list block = content + bottom border (1). Left/right borders are sides.
        let list_block_h = list_content_h + 1; // +1 for bottom border
        let input_block_h: u16 = 3; // top border + input + bottom border (shared with list top)
        let total_h = (input_block_h + list_block_h).min(area.height);

        if area.height < 4 {
            return;
        }

        // Bottom-anchor the bar.
        let bar_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(total_h),
            area.width,
            total_h,
        );
        self.clear_overlay_area(frame, bar_area);

        // Split into input area (top) and list area (bottom).
        let [input_area, list_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(input_block_h), Constraint::Min(1)])
            .areas(bar_area);

        // ── Input block (top, with title and hint badges) ──
        let mut bottom_spans = vec![Span::raw(" ")];
        for (key, desc) in &[("Enter", "send"), ("Tab", "complete"), ("Esc", "cancel")] {
            let badge = self.theme.key_badge_default(key);
            bottom_spans.extend(
                badge
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            bottom_spans.push(Span::styled(
                format!(" {desc}  "),
                Style::default().fg(self.theme.hint_desc_fg),
            ));
        }

        let input_block = self
            .themed_overlay_block("Macros")
            .title_bottom(Line::from(bottom_spans));
        let input_inner = input_block.inner(input_area);
        Paragraph::new(render_single_line_cursor_input(
            "", &query, cursor, cursor_fg, cursor_bg,
        ))
        .block(input_block)
        .render(input_area, frame.buffer_mut());

        // Place hardware cursor inside the input.
        let cursor_col = query[..cursor].chars().count();
        let cx = input_inner.x + cursor_col as u16;
        let cy = input_inner.y;
        if cx < input_inner.x + input_inner.width && cy < input_inner.y + input_inner.height {
            frame.set_cursor_position((cx, cy));
        }

        // ── List block (bottom, connected borders) ──
        let name_col = filtered
            .iter()
            .map(|&(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        let inner_w = list_area.width.saturating_sub(3) as usize; // borders + padding
        let gap = 2usize;

        let items: Vec<ListItem> = if filtered.is_empty() {
            let msg = "No matching macros.";
            vec![ListItem::new(Span::styled(
                msg,
                Style::default().fg(self.theme.hint_desc_fg),
            ))]
        } else {
            filtered
                .iter()
                .map(|&(name, text)| {
                    let name_padded = format!("{name:name_col$}");
                    let mut spans = vec![Span::styled(
                        name_padded,
                        Style::default()
                            .fg(self.theme.help_section_header_fg)
                            .add_modifier(Modifier::BOLD),
                    )];
                    let text_preview = text.replace('\n', "↵");
                    let desc_avail = inner_w.saturating_sub(name_col + gap);
                    let desc_display =
                        if text_preview.chars().count() > desc_avail && desc_avail > 1 {
                            let end = text_preview
                                .char_indices()
                                .nth(desc_avail - 1)
                                .map(|(i, _)| i)
                                .unwrap_or(text_preview.len());
                            format!("  {}\u{2026}", &text_preview[..end])
                        } else {
                            format!("  {text_preview:desc_avail$}")
                        };
                    spans.push(Span::styled(
                        desc_display,
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    ListItem::new(Line::from(spans))
                })
                .collect()
        };

        let list_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.overlay_border))
            .style(Style::default().bg(self.theme.overlay_bg));
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(selected));
        }
        StatefulWidget::render(
            List::new(items)
                .block(list_block)
                .highlight_style(self.theme.selection_style()),
            list_area,
            frame.buffer_mut(),
            &mut list_state,
        );
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

    /// Reset `area` to a blank surface and fill it with the modal overlay
    /// background. Use this in place of a bare `Clear.render(area, ..)` for
    /// every modal popup so the entire popup — including any strip outside
    /// the inner `themed_overlay_block` (footer hints, gaps between stacked
    /// blocks, etc.) — tracks the active theme rather than reading whatever
    /// `Color::Reset` falls through to in the user's terminal.
    ///
    /// Fullscreen surfaces (the agent and terminal fullscreen overlays) want
    /// `app_bg` instead of `overlay_bg` and stay open-coded.
    fn clear_overlay_area(&self, frame: &mut Frame, area: Rect) {
        Clear.render(area, frame.buffer_mut());
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.overlay_bg));
    }

    fn themed_overlay_block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.input_label_fg)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border))
            // Modals are presented after a `Clear.render(..)` which resets the
            // popup cells to `Color::Reset`. Filling the block with overlay_bg
            // means the modal interior — borders, surrounding chrome, the gap
            // around the inner widgets — tracks the active theme instead of
            // reading terminal-default behind the border ring.
            .style(Style::default().bg(self.theme.overlay_bg))
    }

    fn center_pane_agent_title(&self) -> String {
        if let Some(session) = self.selected_session() {
            let provider = capitalize(self.running_provider_for(session).as_str());
            let base = format!("{provider} agent");
            let count = self.session_terminal_count(&session.id);
            if count == 1 {
                return format!("{base} (+ 1 terminal)");
            } else if count > 1 {
                return format!("{base} (+ {count} terminals)");
            }
            return base;
        }
        "Agent".to_string()
    }

    fn render_resource_monitor(
        &mut self,
        frame: &mut Frame,
        rows: &[ResourceStats],
        scroll_offset: u16,
        selected_row: usize,
        expanded: &HashSet<u32>,
        first_sample: bool,
    ) {
        use ratatui::widgets::{Cell, Row, Table, TableState};

        self.render_dim_overlay(frame);
        let popup = centered_rect(85, 78, frame.area());
        self.clear_overlay_area(frame, popup);

        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(popup);
        let content_area = chunks[0];
        let hint_area = chunks[1];

        let block = self.themed_overlay_block(" Resource Monitor ");
        let inner = block.inner(content_area);
        block.render(content_area, frame.buffer_mut());

        let header_style = Style::default()
            .fg(self.theme.help_section_header_fg)
            .add_modifier(Modifier::BOLD);

        let header = Row::new(vec![
            Cell::from("Name").style(header_style),
            Cell::from("PID").style(header_style),
            Cell::from("Procs").style(header_style),
            Cell::from("CPU %").style(header_style),
            Cell::from("RSS").style(header_style),
        ]);

        let visual = build_visual_rows(rows, expanded);
        let dim_style = Style::default().fg(self.theme.hint_dim_desc_fg);

        let table_rows: Vec<Row> = visual
            .iter()
            .map(|vr| match vr {
                VisualRow::Parent(idx) => {
                    let stat = &rows[*idx];
                    let pid_str = stat.pid.map(|p| p.to_string()).unwrap_or_default();
                    let cpu_str = if first_sample {
                        format!("~{:.1}%", stat.cpu_percent)
                    } else {
                        format!("{:.1}%", stat.cpu_percent)
                    };
                    let rss_str = format_bytes(stat.rss_bytes);
                    let procs_str = stat.process_count.to_string();

                    let is_total = stat.label == "TOTAL";
                    let style = if is_total {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    // Expand indicator: ▶/▼ for expandable rows, space for others.
                    let indicator = if !stat.children.is_empty() {
                        if let Some(pid) = stat.pid {
                            if expanded.contains(&pid) {
                                "▼ "
                            } else {
                                "▶ "
                            }
                        } else {
                            "  "
                        }
                    } else {
                        "  "
                    };
                    let label = format!("{indicator}{}", stat.label);

                    Row::new(vec![
                        Cell::from(label),
                        Cell::from(pid_str),
                        Cell::from(procs_str),
                        Cell::from(cpu_str),
                        Cell::from(rss_str),
                    ])
                    .style(style)
                }
                VisualRow::Child(parent_idx, child_idx) => {
                    let child = &rows[*parent_idx].children[*child_idx];
                    let is_last = *child_idx == rows[*parent_idx].children.len().saturating_sub(1);
                    let connector = if is_last { "└" } else { "├" };
                    let label = format!("    {connector} {}", child.name);
                    let cpu_str = if first_sample {
                        format!("~{:.1}%", child.cpu_percent)
                    } else {
                        format!("{:.1}%", child.cpu_percent)
                    };
                    let rss_str = format_bytes(child.rss_bytes);

                    Row::new(vec![
                        Cell::from(label),
                        Cell::from(child.pid.to_string()),
                        Cell::from(""),
                        Cell::from(cpu_str),
                        Cell::from(rss_str),
                    ])
                    .style(dim_style)
                }
            })
            .collect();

        let widths = [
            Constraint::Min(30),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(12),
        ];

        let highlight_style = self.theme.selection_style();
        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(highlight_style);

        let mut table_state = TableState::default()
            .with_offset(scroll_offset as usize)
            .with_selected(Some(selected_row));
        StatefulWidget::render(table, inner, frame.buffer_mut(), &mut table_state);

        let row_area = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.overlay_layout.active = OverlayMouseLayout::ResourceMonitor {
            list: row_area,
            items: visual.len(),
            offset: table_state.offset(),
        };

        // Footer hint.
        let close_key = self.bindings.label_for(Action::CloseOverlay);
        let desc_style = Style::default().fg(self.theme.hint_desc_fg);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(self.theme.key_badge_default(&close_key));
        spans.push(Span::styled(" close  ", desc_style));
        spans.extend(self.theme.key_badge_default("Enter"));
        spans.push(Span::styled(" expand/collapse  ", desc_style));
        spans.extend(self.theme.key_badge_default("Scroll"));
        spans.push(Span::styled(" navigate  ", desc_style));
        spans.push(Span::styled(
            "refreshes every ~2s",
            Style::default().fg(self.theme.hint_dim_desc_fg),
        ));
        let hint_para =
            Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Center);
        hint_para.render(hint_area, frame.buffer_mut());
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
                cell.set_fg(self.theme.overlay_dim_fg);
                cell.set_bg(self.theme.overlay_dim_bg);
            }
        }
    }
}

/// Convert a [`Color`] to its grayscale equivalent using ITU-R BT.601
/// luminance weights (0.299 R + 0.587 G + 0.114 B).
///
/// - `Color::Rgb` values are converted directly.
/// - `Color::Indexed` values are resolved through the standard xterm-256
///   palette before conversion.
/// - All other variants (`Reset`, named ANSI colors, etc.) are returned as-is.
fn to_grayscale(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let l = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64).round() as u8;
            Color::Rgb(l, l, l)
        }
        Color::Indexed(idx) => {
            let (r, g, b) = xterm256_to_rgb(idx);
            let l = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64).round() as u8;
            Color::Rgb(l, l, l)
        }
        other => other,
    }
}

/// Resolve a xterm-256 palette index to (R, G, B).
fn xterm256_to_rgb(idx: u8) -> (u8, u8, u8) {
    #[rustfmt::skip]
    const BASIC: [(u8, u8, u8); 16] = [
        (0,0,0),       (128,0,0),     (0,128,0),     (128,128,0),
        (0,0,128),     (128,0,128),   (0,128,128),   (192,192,192),
        (128,128,128), (255,0,0),     (0,255,0),     (255,255,0),
        (0,0,255),     (255,0,255),   (0,255,255),   (255,255,255),
    ];

    match idx {
        0..=15 => BASIC[idx as usize],
        16..=231 => {
            let val = idx - 16;
            let r_idx = val / 36;
            let g_idx = (val % 36) / 6;
            let b_idx = val % 6;
            let to_level = |i: u8| if i == 0 { 0 } else { 55 + 40 * i };
            (to_level(r_idx), to_level(g_idx), to_level(b_idx))
        }
        232..=255 => {
            let level = 8 + 10 * (idx - 232);
            (level, level, level)
        }
    }
}

/// Choose foreground/background colors for a PTY cell.
///
/// In interactive mode (`is_input == true`) the cell's original colors are
/// returned. In non-interactive mode the foreground is replaced with the
/// theme's dim color and the background is converted to grayscale, giving
/// the pane a muted appearance that signals it is read-only.
///
/// Resolve the foreground/background a single PTY cell should render with.
///
/// The bg is `Option<Color>` to give the caller a way to say "leave the
/// underlying buffer background alone": the rendering loop only applies bg
/// when this is `Some`, so `None` lets whatever was already in the buffer
/// (the frame-wide `app_bg` pre-fill, the dim overlay, the parent block
/// fill, …) show through.
///
/// Interactive mode (fullscreen agent or focused agent/terminal) always
/// passes the CLI's colors through unchanged so the user's configured
/// palette renders end-to-end without dux fighting it.
///
/// Non-interactive (minimized / read-only) mode dims the foreground to a
/// single muted shade. Cells the CLI explicitly colored have their
/// background grayscaled, which is what produces the recognizable "this
/// pane is read-only" look. Cells the CLI emitted with `Color::Reset` are
/// returned with `bg = None` so the dim overlay / app surface continues to
/// show through them — only the solid CLI-painted backgrounds are
/// grayscaled, not the ambient surface around them.
fn pty_cell_colors(fg: Color, bg: Color, is_input: bool, theme: &Theme) -> (Color, Option<Color>) {
    if is_input {
        (fg, Some(bg))
    } else if bg == Color::Reset {
        (theme.overlay_dim_fg, None)
    } else {
        (theme.overlay_dim_fg, Some(to_grayscale(bg)))
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn quit_process_description(agents: usize, terminals: usize) -> String {
    match (agents, terminals) {
        (0, 1) => "1 running terminal".to_string(),
        (0, t) => format!("{t} running terminals"),
        (1, 0) => "1 running agent".to_string(),
        (a, 0) => format!("{a} running agents"),
        (a, t) => {
            let agent_word = if a == 1 { "agent" } else { "agents" };
            let term_word = if t == 1 { "terminal" } else { "terminals" };
            format!("{a} running {agent_word} and {t} {term_word}")
        }
    }
}

fn companion_terminal_status_meta(status: CompanionTerminalStatus) -> (&'static str, &'static str) {
    match status {
        CompanionTerminalStatus::NotLaunched => ("○", "not launched"),
        CompanionTerminalStatus::Running => ("●", "running"),
        CompanionTerminalStatus::Exited => ("◐", "exited"),
    }
}

fn companion_terminal_status_color(theme: &Theme, status: CompanionTerminalStatus) -> Color {
    match status {
        CompanionTerminalStatus::NotLaunched => theme.terminal_hint_fg,
        CompanionTerminalStatus::Running => theme.session_active,
        CompanionTerminalStatus::Exited => theme.session_detached,
    }
}

fn companion_terminal_row_badge(
    status: CompanionTerminalStatus,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if matches!(status, CompanionTerminalStatus::NotLaunched) {
        return Vec::new();
    }
    let (icon, label) = companion_terminal_status_meta(status);
    vec![
        Span::raw(" "),
        Span::styled("[", Style::default().fg(theme.provider_label_fg)),
        Span::styled(
            format!("{icon} term {label}"),
            Style::default().fg(companion_terminal_status_color(theme, status)),
        ),
        Span::styled("]", Style::default().fg(theme.provider_label_fg)),
    ]
}

/// Format additions/deletions as right-aligned colored spans.
/// Returns an empty vec when both counts are zero for text files.
pub(crate) fn format_line_stats(
    additions: usize,
    deletions: usize,
    binary: bool,
    theme: &crate::theme::Theme,
) -> Vec<Span<'static>> {
    if binary {
        return vec![Span::styled(
            "bin",
            Style::default().fg(theme.diff_binary_fg),
        )];
    }
    if additions == 0 && deletions == 0 {
        return Vec::new();
    }
    let mut spans = Vec::new();
    if additions > 0 {
        spans.push(Span::styled(
            format!("+{additions}"),
            Style::default().fg(theme.diff_stat_add_fg),
        ));
    }
    if additions > 0 && deletions > 0 {
        spans.push(Span::raw(" "));
    }
    if deletions > 0 {
        spans.push(Span::styled(
            format!("-{deletions}"),
            Style::default().fg(theme.diff_stat_remove_fg),
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

pub(crate) fn centered_rect_exact(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.max(1));
    let height = height.min(area.height.max(1));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn render_single_line_cursor_input(
    prefix: &str,
    text: &str,
    cursor: usize,
    cursor_fg: Color,
    cursor_bg: Color,
) -> Line<'static> {
    let cursor = cursor.min(text.len());
    if cursor < text.len() {
        let (before, after) = text.split_at(cursor);
        let cursor_char = after.chars().next().expect("cursor within text");
        let cursor_len = cursor_char.len_utf8();
        let rest = &after[cursor_len..];
        Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::raw(before.to_string()),
            Span::styled(
                cursor_char.to_string(),
                Style::default().fg(cursor_fg).bg(cursor_bg),
            ),
            Span::raw(rest.to_string()),
        ])
    } else {
        Line::from(vec![
            Span::raw(format!("{prefix}{text}")),
            Span::styled(" ", Style::default().fg(cursor_fg).bg(cursor_bg)),
        ])
    }
}

fn runtime_context_spans(
    context: &str,
    prose_style: Style,
    quoted_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;

    for ch in context.chars() {
        if ch == '"' {
            if in_quotes {
                buf.push(ch);
                spans.push(Span::styled(std::mem::take(&mut buf), quoted_style));
                in_quotes = false;
            } else {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), prose_style));
                }
                buf.push(ch);
                in_quotes = true;
            }
        } else {
            buf.push(ch);
        }
    }

    if !buf.is_empty() {
        let style = if in_quotes { quoted_style } else { prose_style };
        spans.push(Span::styled(buf, style));
    }

    spans
}

fn scrollback_indicator_label(scrolled: usize, total: usize) -> Option<String> {
    if scrolled == 0 {
        return None;
    }

    let total = total.max(scrolled);
    let noun = if total == 1 { "line" } else { "lines" };
    Some(format!(" {scrolled}/{total} {noun} "))
}

impl App {
    /// Render the GitHub PR pill as a single-line pill using Unicode
    /// half-block characters for the caps and a solid background:
    /// `▐ owner/repo#1234 │ PR title ellipsized… ▌`
    ///
    /// The left cap `▐` (U+2590) paints the right half of the cell in the
    /// state color; the right cap `▌` (U+258C) paints the left half. This
    /// creates a pill-like shape without requiring Powerline/Nerd Fonts.
    /// The `│` divider uses terminal default colors so it blends with the
    /// user's background.
    fn render_pr_banner(&self, frame: &mut Frame, area: Rect, pr: &crate::model::PrInfo) {
        use crate::model::PrState;

        if area.height < 1 || area.width < 6 {
            return;
        }

        // Paint the row with the theme's app background first so the
        // half-block caps blend into a surface that actually changes with
        // the theme. Without this the caps render on top of whatever
        // cell colors the agent PTY left behind, which made the pill look
        // detached on light themes.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));

        let bg = match pr.state {
            PrState::Open => self.theme.pr_open_bg,
            PrState::Merged => self.theme.pr_merged_bg,
            PrState::Closed => self.theme.pr_closed_bg,
        };
        let fg = self.theme.pr_banner_fg;
        // Half-block caps: fg is the pill color, bg is the freshly-painted
        // app surface so the pill arc sits on a theme-driven backdrop
        // instead of the terminal default.
        let cap_style = Style::default().fg(bg).bg(self.theme.app_bg);
        // Inner content: white text on colored background.
        let text_style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
        let title_style = Style::default()
            .fg(fg)
            .bg(bg)
            .add_modifier(Modifier::ITALIC);
        let fill_style = Style::default().fg(fg).bg(bg);

        // Half-block caps (universally supported Unicode).
        let left_cap = "\u{2590}"; // ▐ — right half block
        let right_cap = "\u{258c}"; // ▌ — left half block

        // Build the pill content as a single string.
        // With title:    " ⎇ owner/repo#1234 ▸ PR title here… "
        // Without title: " ⎇ owner/repo#1234 "
        let prefix = format!(" \u{2387} {}#{}", pr.owner_repo, pr.number);
        let title_trimmed = pr.title.trim();
        let has_title = !title_trimmed.is_empty();

        // Available content width inside the pill (between the two caps).
        let avail = area.width as usize;
        let inner_w = avail.saturating_sub(2); // 2 = left cap + right cap
        if inner_w < 4 {
            return;
        }

        let prefix_w = prefix.chars().count();
        let buf = frame.buffer_mut();
        let y = area.y;
        let sx = area.x;
        let mut x = sx;

        // Left cap.
        set_cell(buf, x, y, left_cap, cap_style);
        x += 1;

        if !has_title || inner_w <= prefix_w + 4 {
            // No title or not enough room — render just the prefix, padded.
            // " ⎇ owner/repo#1234 "
            let content = format!("{prefix} ");
            for ch in content.chars() {
                if (x - sx) as usize > inner_w {
                    break;
                }
                set_cell(buf, x, y, &ch.to_string(), text_style);
                x += 1;
            }
            // Fill remaining space.
            while (x - sx) as usize <= inner_w {
                set_cell(buf, x, y, " ", fill_style);
                x += 1;
            }
        } else {
            // Render prefix + arrow + title.
            // " ⎇ owner/repo#1234 ▸ PR title here "
            let arrow = " \u{2192} "; // " ▸ "
            let arrow_w = arrow.chars().count();

            // Write prefix.
            for ch in prefix.chars() {
                set_cell(buf, x, y, &ch.to_string(), text_style);
                x += 1;
            }

            // Write arrow separator.
            for ch in arrow.chars() {
                set_cell(buf, x, y, &ch.to_string(), fill_style);
                x += 1;
            }

            // Remaining space for the title + trailing space.
            let used = prefix_w + arrow_w;
            let title_budget = inner_w.saturating_sub(used + 1); // +1 for trailing space

            // Write title, ellipsized if needed.
            let title_w = title_trimmed.chars().count();
            if title_w > title_budget {
                for (i, ch) in title_trimmed.chars().enumerate() {
                    if i + 1 >= title_budget {
                        set_cell(buf, x, y, "…", title_style);
                        x += 1;
                        break;
                    }
                    set_cell(buf, x, y, &ch.to_string(), title_style);
                    x += 1;
                }
            } else {
                for ch in title_trimmed.chars() {
                    set_cell(buf, x, y, &ch.to_string(), title_style);
                    x += 1;
                }
            }

            // Fill remaining space to the right cap.
            while (x - sx) as usize <= inner_w {
                set_cell(buf, x, y, " ", fill_style);
                x += 1;
            }
        }

        // Right cap.
        set_cell(buf, x, y, right_cap, cap_style);
    }
}

/// Set a single cell in the buffer, bounds-checked.
fn set_cell(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    let area = buf.area();
    if x >= area.x + area.width || y >= area.y + area.height {
        return;
    }
    buf[(x, y)].set_symbol(symbol).set_style(style);
}

/// Truncate `text` to at most `available` **characters**, appending `…` when
/// trimmed. Using char-based counting avoids panics when the text contains
/// multi-byte UTF-8 (e.g. box-drawing or block characters).
fn truncate_status_text(text: &str, available: usize) -> String {
    if text.chars().count() > available && available > 1 {
        let mut truncated: String = text.chars().take(available - 1).collect();
        truncated.push('…');
        truncated
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn companion_terminal_status_meta_covers_v1_states() {
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::NotLaunched),
            ("○", "not launched")
        );
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::Running),
            ("●", "running")
        );
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::Exited),
            ("◐", "exited")
        );
    }

    #[test]
    fn row_badge_hidden_until_terminal_is_launched() {
        let theme = Theme::default_dark();
        assert!(
            companion_terminal_row_badge(CompanionTerminalStatus::NotLaunched, &theme).is_empty()
        );
        assert!(!companion_terminal_row_badge(CompanionTerminalStatus::Running, &theme).is_empty());
    }

    #[test]
    fn runtime_context_spans_highlight_quoted_values() {
        let prose = Style::default().fg(Color::DarkGray);
        let quoted = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let spans = runtime_context_spans(
            "on agent \"foxy-basilisk\" under project \"http-server\"",
            prose,
            quoted,
        );

        assert_eq!(spans.len(), 4);
        assert_eq!(spans[0].content.as_ref(), "on agent ");
        assert_eq!(spans[1].content.as_ref(), "\"foxy-basilisk\"");
        assert_eq!(spans[2].content.as_ref(), " under project ");
        assert_eq!(spans[3].content.as_ref(), "\"http-server\"");
        assert_eq!(spans[0].style, prose);
        assert_eq!(spans[1].style, quoted);
        assert_eq!(spans[3].style, quoted);
    }

    #[test]
    fn render_single_line_cursor_input_supports_empty_prefix() {
        let line = render_single_line_cursor_input("", "macro", 2, Color::White, Color::Black);

        assert_eq!(line.spans.len(), 4);
        assert_eq!(line.spans[0].content.as_ref(), "");
        assert_eq!(line.spans[1].content.as_ref(), "ma");
        assert_eq!(line.spans[2].content.as_ref(), "c");
        assert_eq!(line.spans[3].content.as_ref(), "ro");
    }

    #[test]
    fn centered_rect_exact_centers_requested_size() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(centered_rect_exact(56, 9, area), Rect::new(22, 15, 56, 9));
    }

    #[test]
    fn centered_rect_exact_clamps_to_available_area() {
        let area = Rect::new(0, 0, 40, 6);
        assert_eq!(centered_rect_exact(56, 9, area), area);
    }

    #[test]
    fn wrapped_line_count_counts_unwrapped_lines() {
        let lines = vec![Line::from(" short line"), Line::from(" another short line")];

        assert_eq!(wrapped_line_count(&lines, 40, false), 2);
    }

    #[test]
    fn wrapped_line_count_grows_for_narrow_widths() {
        let lines = vec![Line::from(vec![
            Span::raw(" Are you sure you want to delete "),
            Span::styled(
                "very-long-branch-name",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ])];

        assert!(wrapped_line_count(&lines, 20, false) > 1);
    }

    // ── Unit tests for capitalize ─────────────────────────────────

    #[test]
    fn capitalize_normal_string() {
        assert_eq!(capitalize("claude"), "Claude");
    }

    #[test]
    fn capitalize_already_capitalized() {
        assert_eq!(capitalize("Claude"), "Claude");
    }

    #[test]
    fn capitalize_single_char() {
        assert_eq!(capitalize("c"), "C");
    }

    #[test]
    fn capitalize_empty_string() {
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn capitalize_all_uppercase() {
        assert_eq!(capitalize("CODEX"), "CODEX");
    }

    // ── Unit tests for pty_cell_colors ────────────────────────────

    #[test]
    fn pty_cell_colors_passes_through_in_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        let bg = Color::Rgb(10, 20, 30);
        assert_eq!(pty_cell_colors(fg, bg, true, &theme), (fg, Some(bg)));
    }

    #[test]
    fn pty_cell_colors_preserves_default_bg_in_interactive_mode() {
        // Color::Reset must pass through so the agent/terminal pane shows
        // whatever the user's CLI configured rather than fighting it with
        // a dux-themed surface.
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        assert_eq!(
            pty_cell_colors(fg, Color::Reset, true, &theme),
            (fg, Some(Color::Reset))
        );
    }

    #[test]
    fn pty_cell_colors_grayscales_solid_bg_in_non_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        let bg = Color::Rgb(10, 20, 30);
        let (result_fg, result_bg) = pty_cell_colors(fg, bg, false, &theme);
        assert_eq!(result_fg, theme.overlay_dim_fg);
        // Solid CLI-painted backgrounds get grayscaled, marking them as
        // read-only.
        assert_eq!(result_bg, Some(to_grayscale(bg)));
        // Sanity: the grayscale value is a uniform grey, not near-black.
        let expected_l = (0.299 * 10.0_f64 + 0.587 * 20.0 + 0.114 * 30.0).round() as u8;
        assert_eq!(
            result_bg,
            Some(Color::Rgb(expected_l, expected_l, expected_l))
        );
    }

    #[test]
    fn pty_cell_colors_lets_default_bg_show_through_in_non_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        // Cells the CLI emitted with no explicit background return `None`
        // for the bg so the rendering loop leaves the underlying buffer
        // bg untouched — that lets the dim overlay / app surface show
        // through the minimized PTY view, only grayscaling the cells the
        // CLI actually painted.
        assert_eq!(
            pty_cell_colors(fg, Color::Reset, false, &theme),
            (theme.overlay_dim_fg, None)
        );
    }

    // ── Unit tests for to_grayscale / xterm256_to_rgb ──────────────

    #[test]
    fn to_grayscale_rgb() {
        // Pure red (255,0,0) → luminance ≈ 76
        assert_eq!(to_grayscale(Color::Rgb(255, 0, 0)), Color::Rgb(76, 76, 76));
        // Pure green (0,255,0) → luminance ≈ 150
        assert_eq!(
            to_grayscale(Color::Rgb(0, 255, 0)),
            Color::Rgb(150, 150, 150)
        );
        // Pure white → stays white
        assert_eq!(
            to_grayscale(Color::Rgb(255, 255, 255)),
            Color::Rgb(255, 255, 255)
        );
        // Pure black → stays black
        assert_eq!(to_grayscale(Color::Rgb(0, 0, 0)), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn to_grayscale_indexed() {
        // Index 1 = red (128,0,0) → luminance ≈ 38
        let result = to_grayscale(Color::Indexed(1));
        assert_eq!(result, Color::Rgb(38, 38, 38));
        // Index 244 = grayscale ramp entry → 8 + 10*(244-232) = 128
        assert_eq!(to_grayscale(Color::Indexed(244)), Color::Rgb(128, 128, 128));
    }

    #[test]
    fn to_grayscale_reset_passthrough() {
        assert_eq!(to_grayscale(Color::Reset), Color::Reset);
    }

    #[test]
    fn xterm256_basic_colors() {
        assert_eq!(xterm256_to_rgb(0), (0, 0, 0));
        assert_eq!(xterm256_to_rgb(7), (192, 192, 192));
        assert_eq!(xterm256_to_rgb(15), (255, 255, 255));
    }

    #[test]
    fn xterm256_cube_colors() {
        // Index 16 = (0,0,0)
        assert_eq!(xterm256_to_rgb(16), (0, 0, 0));
        // Index 196 = (255,0,0): (196-16)=180, r=180/36=5, g=0, b=0 → (255,0,0)
        assert_eq!(xterm256_to_rgb(196), (255, 0, 0));
    }

    #[test]
    fn xterm256_grayscale_ramp() {
        assert_eq!(xterm256_to_rgb(232), (8, 8, 8));
        assert_eq!(xterm256_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_below_kib() {
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_exactly_one_kib() {
        assert_eq!(format_bytes(1024), "1 KiB");
    }

    #[test]
    fn format_bytes_mib_range() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 512), "512.0 MiB");
    }

    #[test]
    fn format_bytes_gib_range() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3.0 GiB");
    }

    #[test]
    fn truncate_status_text_ascii_short_enough() {
        assert_eq!(truncate_status_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_status_text_ascii_exact_fit() {
        assert_eq!(truncate_status_text("hello", 5), "hello");
    }

    #[test]
    fn truncate_status_text_ascii_truncated() {
        assert_eq!(truncate_status_text("hello world", 6), "hello…");
    }

    #[test]
    fn truncate_status_text_multibyte_no_panic() {
        // Box-drawing char ─ is 3 bytes but 1 char.
        let text = "Copied: ─────end";
        let result = truncate_status_text(text, 10);
        assert_eq!(result.chars().count(), 10);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_status_text_block_characters() {
        // Block characters like ██▛▘ are multi-byte; slicing by byte would panic.
        let text = "██▛▘ Opus 4.6 (1M context) · Claude Max";
        let result = truncate_status_text(text, 12);
        assert_eq!(result.chars().count(), 12);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_status_text_available_zero() {
        // Edge case: zero available should not panic.
        assert_eq!(truncate_status_text("hello", 0), "hello");
    }

    #[test]
    fn truncate_status_text_available_one() {
        // With only 1 char available, no room for truncation marker.
        assert_eq!(truncate_status_text("hello", 1), "hello");
    }

    #[test]
    fn truncate_status_text_empty_input() {
        assert_eq!(truncate_status_text("", 10), "");
    }
}
